//! Best-effort use/import and process-site facts from committed source, without a compiler.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::manifests::EntryPoints;
use crate::model::{
    ContainerFact, ContainerKind, DependencyFact, FileFact, ReferenceKind, WorkspaceStatus,
};

type Aliases<'a> = HashMap<&'a str, HashMap<String, Option<&'a str>>>;

pub(crate) struct Scanner<'a> {
    files: HashMap<&'a str, &'a FileFact>,
    entries: &'a EntryPoints<'a>,
    cargo: Aliases<'a>,
    npm: Aliases<'a>,
    cargo_self_names: HashMap<&'a str, String>,
    cargo_names: HashMap<String, Option<&'a str>>,
    npm_names: HashMap<&'a str, Option<&'a str>>,
}

#[derive(Default)]
pub(crate) struct Scan {
    pub references: Vec<(String, ReferenceKind)>,
    pub unresolved: u64,
    pub spawns: Vec<String>,
}

impl Scan {
    fn reference(&mut self, target: Option<&str>, kind: ReferenceKind) {
        if let Some(target) = target {
            self.references.push((target.to_owned(), kind));
        } else {
            self.unresolved += 1;
        }
    }
}

impl<'a> Scanner<'a> {
    pub(crate) fn new(
        files: &'a [FileFact],
        containers: &'a [ContainerFact],
        dependencies: &'a [DependencyFact],
        entries: &'a EntryPoints<'a>,
    ) -> Self {
        let mut scanner = Self {
            files: files
                .iter()
                .map(|file| (file.path.as_str(), file))
                .collect(),
            entries,
            cargo: HashMap::new(),
            npm: HashMap::new(),
            cargo_self_names: HashMap::new(),
            cargo_names: HashMap::new(),
            npm_names: HashMap::new(),
        };
        for container in containers {
            match container.kind {
                ContainerKind::Cargo => {
                    let name = rust_name(&container.name);
                    if container.workspace_status == WorkspaceStatus::Member {
                        insert_unique(
                            &mut scanner.cargo_names,
                            name.clone(),
                            Some(&container.path),
                        );
                    }
                    scanner.cargo_self_names.insert(&container.path, name);
                }
                ContainerKind::Npm if container.workspace_status == WorkspaceStatus::Member => {
                    insert_unique(
                        &mut scanner.npm_names,
                        container.name.as_str(),
                        Some(&container.path),
                    );
                }
                _ => {}
            }
        }
        for dependency in dependencies
            .iter()
            .filter(|dep| dep.section != "workspace.dependencies")
        {
            let (aliases, key) = match basename(&dependency.manifest) {
                "Cargo.toml" => (&mut scanner.cargo, rust_name(&dependency.alias)),
                "package.json" => (&mut scanner.npm, dependency.alias.clone()),
                _ => continue,
            };
            insert_unique(
                aliases.entry(&dependency.container).or_default(),
                key,
                dependency.target.as_deref(),
            );
        }
        scanner
    }

    pub(crate) fn scan(&self, file: &FileFact, text: &str) -> Scan {
        let mut scan = Scan::default();
        match file.language.as_str() {
            "Rust" => self.scan_rust(file, &tokens(&RUST, text), &mut scan),
            "TypeScript" | "JavaScript" => self.scan_js(file, &tokens(&JS, text), &mut scan),
            "Python" => scan_python(&tokens(&PYTHON, text), &mut scan),
            _ => {}
        }
        scan
    }

    fn scan_rust(&self, file: &FileFact, tokens: &[&str], scan: &mut Scan) {
        let (root, entry) = self.rust_scope(file);
        let stem = file.path.strip_suffix(".rs").unwrap_or(&file.path);
        let current = if entry == Some(file.path.as_str()) || basename(stem) == "mod" {
            dirname(stem)
        } else {
            stem
        };
        let mut bindings = ProcessBindings::default();
        for (i, _) in tokens
            .iter()
            .enumerate()
            .filter(|(_, token)| **token == "use")
        {
            let mut rest = &tokens[i + 1..];
            let mut leaves = Vec::new();
            let absolute = eat(&mut rest, "::");
            if !rust_tree(&mut rest, &mut Vec::new(), &mut leaves, 0) || !eat(&mut rest, ";") {
                continue;
            }
            for (path, alias) in leaves {
                scan.reference(
                    self.rust_use(file, &root, entry, current, &path, absolute),
                    ReferenceKind::Use,
                );
                match path.as_slice() {
                    ["std" | "tokio", "process"] => {
                        bindings.namespaces.insert(alias.unwrap_or("process"));
                    }
                    ["std" | "tokio", "process", "Command" | "*"] => {
                        bindings.functions.insert(alias.unwrap_or("Command"), "new");
                    }
                    _ => {}
                }
            }
        }
        for (i, token) in tokens.iter().enumerate() {
            if *token != "new" {
                continue;
            }
            let prefix = &tokens[..i];
            let process_constructor = match prefix {
                [.., "std" | "tokio", "::", "process", "::", "Command", "::"] => true,
                [.., namespace, "::", "Command", "::"] if !preceded_by(prefix, 4, "::") => {
                    bindings.namespaces.contains(namespace)
                }
                [.., command, "::"] if !preceded_by(prefix, 2, "::") => {
                    bindings.functions.contains_key(command)
                }
                _ => false,
            };
            if process_constructor
                && !tokens
                    .get(i + 2)
                    .is_some_and(|token| token.starts_with('\''))
                && let Some(program) = literal_arg(&tokens[i + 1..], false)
            {
                scan.spawns.push(program.to_owned());
            }
        }
    }

    fn rust_scope(&self, file: &FileFact) -> (String, Option<&'a str>) {
        let container = &file.container;
        let src = join(container, "src");
        let rel = file
            .path
            .strip_prefix(&src)
            .and_then(|p| p.strip_prefix('/'));
        let root = if rel.is_some()
            || self
                .conventional_rust_entry(container)
                .is_some_and(|p| dirname(p) == src)
        {
            src
        } else {
            container.clone()
        };
        let main = join(&root, "main.rs");
        let entry = if main == file.path {
            self.rust_file(&main, container)
        } else {
            self.rust_file(&join(&root, "lib.rs"), container)
                .or_else(|| self.rust_file(&main, container))
        };
        (root, entry)
    }

    fn rust_entry(&self, container: &str) -> Option<&'a str> {
        if let Some(path) = self.entries.get(&(ContainerKind::Cargo, container)) {
            return self.rust_file(path, container);
        }
        self.conventional_rust_entry(container)
    }

    fn conventional_rust_entry(&self, container: &str) -> Option<&'a str> {
        ["src/lib.rs", "src/main.rs", "lib.rs", "main.rs"]
            .into_iter()
            .find_map(|path| self.rust_file(&join(container, path), container))
    }

    fn rust_file(&self, path: &str, container: &str) -> Option<&'a str> {
        let file = self.files.get(path)?;
        (file.language == "Rust" && file.container == container).then_some(&file.path)
    }

    fn rust_use(
        &self,
        file: &FileFact,
        root: &str,
        entry: Option<&'a str>,
        current: &str,
        path: &[&str],
        absolute: bool,
    ) -> Option<&'a str> {
        let (first, tail) = path.split_first()?;
        let container = file.container.as_str();
        match *first {
            "crate" if !absolute => return self.rust_module(container, root, entry, tail),
            "self" | "super" if !absolute => {
                let (mut base, mut tail) = (current, path);
                if *first == "self" {
                    tail = &tail[1..];
                }
                while let Some((&"super", rest)) = tail.split_first() {
                    if base == root || base.len() <= root.len() {
                        return None;
                    }
                    base = dirname(base);
                    tail = rest;
                }
                let base_file = if base == root {
                    entry
                } else {
                    self.rust_file_at(container, base)
                };
                return self.rust_module(container, base, base_file, tail);
            }
            "std" | "core" | "alloc" => return None,
            _ => {}
        }
        if let Some(target) = self
            .cargo
            .get(container)
            .and_then(|aliases| aliases.get(*first))
        {
            let target = (*target)?;
            let entry = self.rust_entry(target)?;
            return self.rust_module(target, dirname(entry), Some(entry), tail);
        }
        if self
            .cargo_self_names
            .get(container)
            .is_some_and(|name| name == first)
        {
            let entry = self.rust_entry(container)?;
            return self.rust_module(container, dirname(entry), Some(entry), tail);
        }
        if !absolute
            && let Some(target) = self
                .rust_module(container, current, None, path)
                .or_else(|| self.rust_module(container, root, None, path))
        {
            return Some(target);
        }
        let target = (*self.cargo_names.get(*first)?)?;
        let entry = self.rust_entry(target)?;
        self.rust_module(target, dirname(entry), Some(entry), tail)
    }

    fn rust_module(
        &self,
        container: &str,
        base: &str,
        base_file: Option<&'a str>,
        path: &[&str],
    ) -> Option<&'a str> {
        let mut candidate = join(base, &path.join("/"));
        // A use path may end with a symbol rather than a module. Use the deepest file
        // known to the tree; a package entry file is only eligible for root-level names.
        for _ in path {
            if let Some(file) = self.rust_file_at(container, &candidate) {
                return Some(file);
            }
            candidate.truncate(dirname(&candidate).len());
        }
        base_file.filter(|_| path.len() <= 1)
    }

    fn rust_file_at(&self, container: &str, stem: &str) -> Option<&'a str> {
        self.rust_file(&format!("{stem}.rs"), container)
            .or_else(|| self.rust_file(&join(stem, "mod.rs"), container))
    }

    fn scan_js(&self, file: &FileFact, tokens: &[&str], scan: &mut Scan) {
        let mut bindings = ProcessBindings::default();
        for (i, token) in tokens.iter().enumerate() {
            let after = &tokens[i + 1..];
            let before = &tokens[..i];
            let (specifier, clause) = match (*token, after) {
                ("require" | "import", ["(", ..]) if !js_member(before) => {
                    (literal_arg(after, true), cjs_bindings(before))
                }
                ("import", [source, ..])
                    if !js_member(before) && literal(source, false).is_some() =>
                {
                    (literal(source, false), &[][..])
                }
                ("import" | "export", _) if !js_member(before) => js_from(token, after),
                _ => continue,
            };
            if let Some(specifier) = specifier {
                scan.reference(self.js_import(file, specifier), ReferenceKind::Import);
                if matches!(specifier, "node:child_process" | "child_process") && *token != "export"
                {
                    bindings.js_clause(clause);
                }
            }
        }
        for (i, token) in tokens.iter().enumerate() {
            let before = &tokens[..i];
            let api = if !js_member(before) {
                bindings.functions.get(token).copied()
            } else {
                match before {
                    [.., ns, "." | "?."]
                        if !preceded_by(before, 2, ".") && bindings.namespaces.contains(ns) =>
                    {
                        Some(*token)
                    }
                    [.., "require", "(", source, ")", "." | "?."]
                        if !js_member(&before[..before.len() - 5])
                            && matches!(
                                literal(source, false),
                                Some("node:child_process" | "child_process")
                            ) =>
                    {
                        Some(*token)
                    }
                    _ => None,
                }
            };
            if let Some(api) = api.filter(|api| js_process_api(api))
                && let Some(arg) = literal_arg(&tokens[i + 1..], true)
                && let Some(program) = js_program(api, arg)
            {
                scan.spawns.push(program.to_owned());
            }
        }
    }

    fn js_import(&self, file: &FileFact, specifier: &str) -> Option<&'a str> {
        if matches!(specifier, "." | "..")
            || specifier.starts_with("./")
            || specifier.starts_with("../")
        {
            let path = normalize(dirname(&file.path), specifier)?;
            return self.js_path(&path);
        }
        if specifier.starts_with(['/', '#']) || specifier.contains(':') {
            return None;
        }
        let (name, subpath) = package_specifier(specifier)?;
        let target = match self
            .npm
            .get(file.container.as_str())
            .and_then(|aliases| aliases.get(name))
        {
            Some(target) => (*target)?,
            None => (*self.npm_names.get(name)?)?,
        };
        // A package name is not a path alias for arbitrary files outside that package.
        if subpath.split('/').any(|part| matches!(part, "." | "..")) {
            return None;
        }
        if subpath.is_empty()
            && let Some(main) = self.entries.get(&(ContainerKind::Npm, target))
            && let Some(path) = self.js_path(main)
            && self.files[path].container == target
        {
            return Some(path);
        }
        self.js_path(&join(target, subpath))
            .or_else(|| self.js_path(&join(&join(target, "src"), subpath)))
    }

    fn js_path(&self, path: &str) -> Option<&'a str> {
        const EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs", "json"];
        let found = |path: &str| self.files.get(path).map(|file| file.path.as_str());
        if let Some(file) = found(path) {
            return Some(file);
        }
        if let Some((stem, ext)) = path.rsplit_once('.') {
            let extensions: &[&str] = match ext {
                "js" => &["ts", "tsx"],
                "jsx" => &["tsx"],
                "mjs" => &["mts"],
                "cjs" => &["cts"],
                _ => &[],
            };
            for ext in extensions {
                if let Some(file) = found(&format!("{stem}.{ext}")) {
                    return Some(file);
                }
            }
        }
        for ext in EXTENSIONS {
            if !path.is_empty()
                && let Some(file) = found(&format!("{path}.{ext}"))
            {
                return Some(file);
            }
        }
        let index = join(path, "index");
        EXTENSIONS
            .iter()
            .find_map(|ext| found(&format!("{index}.{ext}")))
    }
}

fn rust_name(name: &str) -> String {
    name.replace('-', "_")
}
fn dirname(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}
fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn join(base: &str, path: &str) -> String {
    if base.is_empty() {
        path.to_owned()
    } else if path.is_empty() {
        base.to_owned()
    } else {
        format!("{base}/{path}")
    }
}

fn normalize(base: &str, relative: &str) -> Option<String> {
    let mut parts: Vec<_> = base.split('/').filter(|p| !p.is_empty()).collect();
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

fn package_specifier(value: &str) -> Option<(&str, &str)> {
    let mut separators = value.match_indices('/');
    if value.starts_with('@') {
        separators.next()?;
    }
    let (name, subpath) = separators
        .next()
        .map_or((value, ""), |(i, _)| (&value[..i], &value[i + 1..]));
    (!name.is_empty()).then_some((name, subpath))
}

fn insert_unique<'a, K: Eq + std::hash::Hash>(
    map: &mut HashMap<K, Option<&'a str>>,
    key: K,
    target: Option<&'a str>,
) {
    map.entry(key)
        .and_modify(|old| {
            if *old != target {
                *old = None;
            }
        })
        .or_insert(target);
}

// Separate string and comment tokens so that a keyword in a quoted string is not treated as code.
const ID: &str = r"(?:\p{XID_Start}|[_$])(?:\p{XID_Continue}|[$])*";
const DOUBLE: &str = r#""(?:\\.|[^"\\\r\n])*""#;
const SINGLE: &str = r"'(?:\\.|[^'\\\r\n])*'";
const C_COMMENTS: &str = r"//[^\r\n]*|/\*.*?\*/";
const PY_COMMENTS: &str = r"\#[^\r\n]*";

static RUST: LazyLock<Regex> = LazyLock::new(|| {
    lexer(&[
        C_COMMENTS,
        r###"r##".*?"##|r#".*?"#|r"[^"]*""###,
        DOUBLE,
        r"'(?:\\.|[^'\\\r\n])'",
        r"r#\p{XID_Continue}+",
        ID,
        "::",
    ])
});
static JS: LazyLock<Regex> = LazyLock::new(|| {
    lexer(&[
        C_COMMENTS,
        DOUBLE,
        SINGLE,
        r"`(?:\\.|[^`\\])*`",
        ID,
        r"\?\.",
    ])
});
static PYTHON: LazyLock<Regex> = LazyLock::new(|| {
    lexer(&[
        PY_COMMENTS,
        r#"\x22{3}.*?\x22{3}|'{3}.*?'{3}"#,
        DOUBLE,
        SINGLE,
        ID,
    ])
});

fn lexer(alternatives: &[&str]) -> Regex {
    match Regex::new(&format!(r"(?s)(?:{}|\s+|.)", alternatives.join("|"))) {
        Ok(regex) => regex,
        Err(error) => panic!("invalid built-in syntax scanner expression: {error}"),
    }
}

fn tokens<'a>(regex: &Regex, text: &'a str) -> Vec<&'a str> {
    regex
        .find_iter(text)
        .map(|m| m.as_str())
        .filter(|token| {
            !["//", "/*", "#"]
                .iter()
                .any(|prefix| token.starts_with(prefix))
                && !token.trim().is_empty()
        })
        .collect()
}

fn eat(tokens: &mut &[&str], token: &str) -> bool {
    if tokens.first() != Some(&token) {
        return false;
    }
    *tokens = &tokens[1..];
    true
}

fn is_ident(token: &str) -> bool {
    token.starts_with(|c: char| c.is_alphabetic() || matches!(c, '_' | '$'))
        && !token.contains(['"', '\'', '`'])
}

fn take_id<'a>(tokens: &mut &[&'a str]) -> Option<&'a str> {
    let (first, rest) = tokens.split_first()?;
    let name = first.strip_prefix("r#").unwrap_or(first);
    if !is_ident(name) {
        return None;
    }
    *tokens = rest;
    Some(name)
}

fn preceded_by(tokens: &[&str], count: usize, token: &str) -> bool {
    tokens
        .len()
        .checked_sub(count + 1)
        .and_then(|i| tokens.get(i))
        == Some(&token)
}

type UseLeaf<'a> = (Vec<&'a str>, Option<&'a str>);

fn rust_tree<'a>(
    tokens: &mut &[&'a str],
    prefix: &mut Vec<&'a str>,
    leaves: &mut Vec<UseLeaf<'a>>,
    depth: usize,
) -> bool {
    if depth == 64 {
        return false;
    }
    let len = prefix.len();
    loop {
        if prefix.len() == 64 {
            return false;
        }
        if eat(tokens, "{") {
            while !eat(tokens, "}") {
                if !rust_tree(tokens, prefix, leaves, depth + 1) {
                    return false;
                }
                if !eat(tokens, ",") {
                    if !eat(tokens, "}") {
                        return false;
                    }
                    break;
                }
            }
            break;
        }
        let wildcard = eat(tokens, "*");
        let name = if wildcard {
            "*"
        } else if let Some(name) = take_id(tokens) {
            name
        } else {
            return false;
        };
        if !wildcard && eat(tokens, "::") {
            prefix.push(name);
            continue;
        }
        if name != "self" || prefix.is_empty() {
            prefix.push(name);
        }
        let alias = if eat(tokens, "as") {
            let Some(alias) = take_id(tokens) else {
                return false;
            };
            Some(alias)
        } else {
            None
        };
        leaves.push((prefix.clone(), alias));
        break;
    }
    prefix.truncate(len);
    true
}

fn literal(token: &str, template: bool) -> Option<&str> {
    let quote = token.chars().next()?;
    let content = match quote {
        '"' | '\'' => token.strip_prefix(quote)?.strip_suffix(quote)?,
        '`' if template && !token.contains("${") => token.strip_prefix('`')?.strip_suffix('`')?,
        'r' => token
            .strip_prefix("r#\"")
            .and_then(|s| s.strip_suffix("\"#"))
            .or_else(|| token.strip_prefix("r\"").and_then(|s| s.strip_suffix('"')))?,
        _ => return None,
    };
    (!content.is_empty() && !content.contains(['\\', '\n', '\r', '\0'])).then_some(content)
}

fn literal_arg<'a>(tokens: &[&'a str], template: bool) -> Option<&'a str> {
    match tokens {
        ["(", source, ")" | ",", ..] => literal(source, template),
        _ => None,
    }
}

fn js_member(before: &[&str]) -> bool {
    matches!(before.last(), Some(&("." | "?.")))
}

fn js_from<'a, 't>(keyword: &str, tokens: &'t [&'a str]) -> (Option<&'a str>, &'t [&'a str]) {
    // The binding clause ends at `from`, not somewhere later in an unrelated statement.
    let mut rest = tokens;
    if matches!(rest, ["type", next, ..] if *next != "from" && *next != ",") {
        eat(&mut rest, "type");
    }
    if keyword == "import" && take_id(&mut rest).is_some() && !eat(&mut rest, ",") {
        return match rest {
            ["from", source, ..] => (literal(source, false), &tokens[..tokens.len() - rest.len()]),
            _ => (None, &[]),
        };
    }
    if eat(&mut rest, "*") {
        if eat(&mut rest, "as") && take_id(&mut rest).is_none() {
            return (None, &[]);
        }
    } else if eat(&mut rest, "{") {
        let Some(end) = rest
            .iter()
            .position(|token| matches!(*token, "}" | ";" | "{"))
        else {
            return (None, &[]);
        };
        rest = &rest[end..];
        if !eat(&mut rest, "}") {
            return (None, &[]);
        }
    } else {
        return (None, &[]);
    }
    match rest {
        ["from", source, ..] => (literal(source, false), &tokens[..tokens.len() - rest.len()]),
        _ => (None, &[]),
    }
}

fn cjs_bindings<'a, 't>(before: &'t [&'a str]) -> &'t [&'a str] {
    match before {
        [.., "const" | "let" | "var", binding, "="] if is_ident(binding) => {
            &before[before.len() - 2..before.len() - 1]
        }
        [.., "}", "="] => {
            let Some(start) = before.iter().rposition(|token| matches!(*token, "{" | ";")) else {
                return &[];
            };
            if before[start] == "{"
                && matches!(
                    before.get(start.wrapping_sub(1)),
                    Some(&("const" | "let" | "var"))
                )
            {
                &before[start..before.len() - 1]
            } else {
                &[]
            }
        }
        _ => &[],
    }
}

#[derive(Default)]
struct ProcessBindings<'a> {
    functions: HashMap<&'a str, &'a str>,
    namespaces: HashSet<&'a str>,
}

impl<'a> ProcessBindings<'a> {
    fn js_clause(&mut self, mut clause: &[&'a str]) {
        if matches!(clause, ["type", _, ..]) {
            return;
        }
        if let Some(default) = take_id(&mut clause) {
            self.namespaces.insert(default);
            eat(&mut clause, ",");
        }
        match clause {
            ["*", "as", alias] => {
                self.namespaces.insert(alias);
            }
            ["{", entries @ .., "}"] => {
                for entry in entries.split(|token| *token == ",") {
                    match entry {
                        [name] if js_process_api(name) => {
                            self.functions.insert(name, name);
                        }
                        [name, "as" | ":", alias] if js_process_api(name) => {
                            self.functions.insert(alias, name);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

fn js_process_api(name: &str) -> bool {
    matches!(
        name,
        "spawn" | "spawnSync" | "execFile" | "execFileSync" | "exec" | "execSync"
    )
}

fn js_program<'a>(api: &str, arg: &'a str) -> Option<&'a str> {
    if !matches!(api, "exec" | "execSync") {
        return Some(arg);
    }
    // Shell parsing is outside syntax coverage. Only accept a plain initial command word.
    let first = arg.split_whitespace().next()?;
    first
        .chars()
        .all(|c| c.is_alphanumeric() || "_./+-".contains(c))
        .then_some(first)
}

fn scan_python(tokens: &[&str], scan: &mut Scan) {
    let mut bindings = ProcessBindings::default();
    for (i, token) in tokens.iter().enumerate() {
        if *token != "import" {
            continue;
        }
        let mut rest = &tokens[i + 1..];
        if tokens[..i].ends_with(&["from", "subprocess"]) {
            eat(&mut rest, "(");
            while let Some(name) = take_id(&mut rest) {
                let alias = python_alias(&mut rest, name);
                if python_api(name) {
                    bindings.functions.insert(alias, name);
                }
                if !eat(&mut rest, ",") {
                    break;
                }
            }
        } else if !preceded_by(&tokens[..i], 1, "from") {
            while let Some(name) = take_id(&mut rest) {
                let alias = python_alias(&mut rest, name);
                if name == "subprocess" && rest.first() != Some(&".") {
                    bindings.namespaces.insert(alias);
                }
                if !eat(&mut rest, ",") {
                    break;
                }
            }
        }
    }
    for (i, token) in tokens.iter().enumerate() {
        let before = &tokens[..i];
        let process_call = match before {
            [.., ns, "."] if !preceded_by(before, 2, ".") => {
                bindings.namespaces.contains(ns) && python_api(token)
            }
            _ => !before.ends_with(&["."]) && bindings.functions.contains_key(token),
        };
        if process_call && let ["(", rest @ ..] = &tokens[i + 1..] {
            let rest = rest.strip_prefix(&["args", "="]).unwrap_or(rest);
            let program = match rest {
                ["[" | "(", arg, "," | "]" | ")", ..] | [arg, "," | ")", ..] => literal(arg, false),
                _ => None,
            };
            if let Some(program) = program {
                scan.spawns.push(program.to_owned());
            }
        }
    }
}

fn python_alias<'a>(rest: &mut &[&'a str], default: &'a str) -> &'a str {
    if eat(rest, "as") {
        take_id(rest).unwrap_or(default)
    } else {
        default
    }
}

fn python_api(name: &str) -> bool {
    matches!(
        name,
        "Popen" | "run" | "call" | "check_call" | "check_output"
    )
}
