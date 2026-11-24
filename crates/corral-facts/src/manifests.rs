use std::collections::{BTreeMap, BTreeSet, VecDeque};

use regex::Regex;
use serde_json::Value as Json;
use toml::Value as Toml;

use crate::model::{ContainerFact, ContainerKind, DependencyFact, WorkspaceStatus};

const CARGO_SECTIONS: &[&str] = &[
    "dependencies",
    "dev-dependencies",
    "build-dependencies",
    "dev_dependencies",
    "build_dependencies",
];

struct Manifest<'a> {
    file: &'a str,
    dir: &'a str,
    kind: ContainerKind,
    name: Option<String>,
    entry: Option<String>,
    workspace: Option<Workspace>,
    cargo_workspace_path: Option<String>,
    declarations: Vec<Declaration>,
}

struct Declaration {
    section: String,
    alias: String,
    package: String,
    requirement: String,
    options: Option<String>,
    source: Source,
}

#[derive(PartialEq, Eq)]
enum Source {
    Name,
    Path(String),
    Remote,
    Inherited,
    WorkspaceName,
}

#[derive(Default)]
struct Workspace {
    members: Vec<Pattern>,
    exclude: Vec<Pattern>,
}

#[derive(Default, Clone, Copy)]
struct Membership {
    owner: Option<usize>,
    member: bool,
    excluded: bool,
}

type ManifestIndex<'a> = BTreeMap<(ContainerKind, &'a str), usize>;

pub(crate) type EntryPoints<'a> = BTreeMap<(ContainerKind, &'a str), String>;

pub(crate) struct Extraction<'a> {
    pub containers: Vec<ContainerFact>,
    pub dependencies: Vec<DependencyFact>,
    pub parsed: BTreeSet<String>,
    pub entries: EntryPoints<'a>,
}

pub(crate) fn extract<'a>(blobs: &'a BTreeMap<String, String>, paths: &[String]) -> Extraction<'a> {
    let manifests: Vec<_> = blobs
        .iter()
        .filter_map(|(file, text)| parse(file, text))
        .collect();
    let parsed = manifests.iter().map(|item| item.file.to_owned()).collect();
    let by_dir: ManifestIndex<'_> = manifests
        .iter()
        .enumerate()
        .map(|(id, item)| ((item.kind, item.dir), id))
        .collect();
    let mut by_name = BTreeMap::<_, Vec<_>>::new();
    let membership = memberships(&manifests, &by_dir);
    let mut containers = Vec::new();
    let mut entries = EntryPoints::new();
    let mut claimed = BTreeSet::new();
    for (id, item) in manifests.iter().enumerate() {
        let Some(name) = &item.name else { continue };
        if let Some(entry) = &item.entry {
            entries.insert((item.kind, item.dir), entry.clone());
        }
        by_name
            .entry((item.kind, name_key(item.kind, name)))
            .or_default()
            .push(id);
        claimed.insert(item.dir);
        let group = membership[id];
        let workspace_status = if group.owner == Some(id) && !item.dir.is_empty() {
            WorkspaceStatus::SeparateWorkspace
        } else if group.member {
            WorkspaceStatus::Member
        } else if group.owner.is_some() {
            WorkspaceStatus::Excluded
        } else {
            WorkspaceStatus::SeparateWorkspace
        };
        containers.push(ContainerFact {
            path: item.dir.to_owned(),
            name: name.clone(),
            kind: item.kind,
            workspace_status,
            other_packages: Vec::new(),
        });
    }

    // Compute fallbacks from real packages only: a root fallback must not claim
    // otherwise unclaimed top-level folders.
    let fallbacks: BTreeSet<_> = paths
        .iter()
        .filter(|path| nearest(parent(path), &claimed).is_none())
        .map(|path| path.split_once('/').map_or("", |(top, _)| top))
        .collect();
    for dir in fallbacks {
        claimed.insert(dir);
        containers.push(ContainerFact {
            path: dir.to_owned(),
            name: if dir.is_empty() { "(root files)" } else { dir }.to_owned(),
            kind: ContainerKind::Folder,
            workspace_status: WorkspaceStatus::SeparateWorkspace,
            other_packages: Vec::new(),
        });
    }

    let mut dependencies = Vec::new();
    for (id, item) in manifests.iter().enumerate() {
        let container = nearest(item.dir, &claimed).unwrap_or(item.dir);
        for declared in &item.declarations {
            let (origin, effective) = effective_declaration(id, declared, &manifests, &membership);
            let target = match &effective.source {
                Source::Path(path) => local_path(manifests[origin].dir, path)
                    .and_then(|path| package_at(item.kind, &path, &by_dir, &manifests)),
                Source::Name | Source::WorkspaceName => by_name
                    .get(&(item.kind, name_key(item.kind, &effective.package)))
                    .and_then(|candidates| {
                        let mut matches = candidates.iter().copied().filter(|&candidate| {
                            effective.source != Source::WorkspaceName
                                || (membership[id].member
                                    && membership[candidate].member
                                    && membership[id].owner == membership[candidate].owner)
                        });
                        let first = matches.next()?;
                        matches.next().is_none().then_some(first)
                    }),
                Source::Remote | Source::Inherited => None,
            };
            dependencies.push(DependencyFact {
                container: container.to_owned(),
                manifest: item.file.to_owned(),
                section: declared.section.clone(),
                alias: declared.alias.clone(),
                package: effective.package.clone(),
                requirement: effective.requirement.clone(),
                options: declared.options.clone(),
                target: target.map(|target| manifests[target].dir.to_owned()),
            });
        }
    }
    containers.sort();
    dependencies.sort();
    dependencies.dedup();
    Extraction {
        containers,
        dependencies,
        parsed,
        entries,
    }
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    std::iter::successors(Some(path), |&p| (!p.is_empty()).then(|| parent(p)))
}

fn nearest<'a>(dir: &'a str, paths: &BTreeSet<&str>) -> Option<&'a str> {
    ancestors(dir).find(|path| paths.contains(path))
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

pub(crate) fn local_path(dir: &str, path: &str) -> Option<String> {
    if path.is_empty() || path.starts_with('/') || path.contains([':', '\\']) {
        return None;
    }
    let mut pieces: Vec<_> = dir.split('/').filter(|part| !part.is_empty()).collect();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                pieces.pop()?;
            }
            _ => pieces.push(part),
        }
    }
    Some(pieces.join("/"))
}

fn package_at(
    kind: ContainerKind,
    path: &str,
    by_dir: &ManifestIndex<'_>,
    manifests: &[Manifest<'_>],
) -> Option<usize> {
    let file = match kind {
        ContainerKind::Cargo => "Cargo.toml",
        ContainerKind::Npm => "package.json",
        ContainerKind::Python => "pyproject.toml",
        ContainerKind::Folder => return None,
    };
    let at_dir = |dir| {
        by_dir
            .get(&(kind, dir))
            .copied()
            .filter(|&id| manifests[id].name.is_some())
    };
    at_dir(path).or_else(|| {
        if basename(path) == file {
            at_dir(parent(path))
        } else {
            None
        }
    })
}

fn memberships(manifests: &[Manifest<'_>], by_dir: &ManifestIndex<'_>) -> Vec<Membership> {
    let mut groups = Vec::new();
    for (id, item) in manifests.iter().enumerate() {
        let owner = if item.workspace.is_some() {
            Some(id)
        } else if let Some(path) = &item.cargo_workspace_path {
            local_path(item.dir, path)
                .and_then(|path| by_dir.get(&(item.kind, path.as_str())).copied())
                .filter(|&other| manifests[other].workspace.is_some())
        } else {
            ancestors(item.dir)
                .filter_map(|dir| by_dir.get(&(item.kind, dir)).copied())
                .find(|&other| manifests[other].workspace.is_some())
        };
        let mut group = Membership {
            owner,
            ..Membership::default()
        };
        if let Some(owner) = owner
            && let Some(workspace) = &manifests[owner].workspace
        {
            let root = manifests[owner].dir;
            let relative = if root.is_empty() {
                Some(item.dir)
            } else {
                item.dir
                    .strip_prefix(root)
                    .and_then(|rest| rest.strip_prefix('/'))
            };
            group.excluded = owner != id
                && relative.is_some_and(|dir| {
                    workspace.exclude.iter().any(|pattern| {
                        if item.kind == ContainerKind::Cargo {
                            ancestors(dir).any(|dir| pattern.matches(dir))
                        } else {
                            pattern.matches(dir)
                        }
                    })
                });
            group.member = !group.excluded
                && (owner == id
                    || item.cargo_workspace_path.is_some()
                    || relative
                        .is_some_and(|dir| workspace.members.iter().any(|p| p.matches(dir))));
        }
        groups.push(group);
    }

    // Cargo also includes in-tree path dependencies of members, transitively.
    // Do not cross an exclusion or a more deeply nested workspace boundary.
    let mut queue: VecDeque<_> = groups
        .iter()
        .enumerate()
        .filter(|&(id, group)| group.member && manifests[id].kind == ContainerKind::Cargo)
        .map(|(id, _)| id)
        .collect();
    while let Some(id) = queue.pop_front() {
        for declared in &manifests[id].declarations {
            if declared.section == "workspace.dependencies" {
                continue;
            }
            let (origin, effective) = effective_declaration(id, declared, manifests, &groups);
            let Source::Path(path) = &effective.source else {
                continue;
            };
            let Some(target) = local_path(manifests[origin].dir, path)
                .and_then(|path| package_at(ContainerKind::Cargo, &path, by_dir, manifests))
            else {
                continue;
            };
            if groups[target].owner == groups[id].owner
                && !groups[target].excluded
                && !groups[target].member
            {
                groups[target].member = true;
                queue.push_back(target);
            }
        }
    }
    groups
}

fn effective_declaration<'a>(
    id: usize,
    declared: &'a Declaration,
    manifests: &'a [Manifest<'_>],
    groups: &[Membership],
) -> (usize, &'a Declaration) {
    if declared.source == Source::Inherited
        && groups[id].member
        && let Some(owner) = groups[id].owner
        && let Some(definition) = manifests[owner]
            .declarations
            .iter()
            .find(|d| d.section == "workspace.dependencies" && d.alias == declared.alias)
    {
        (owner, definition)
    } else {
        (id, declared)
    }
}

fn parse<'a>(file: &'a str, text: &str) -> Option<Manifest<'a>> {
    let kind = match basename(file) {
        "Cargo.toml" => ContainerKind::Cargo,
        "package.json" => ContainerKind::Npm,
        "pyproject.toml" => ContainerKind::Python,
        _ => return None,
    };
    let mut item = Manifest {
        file,
        dir: parent(file),
        kind,
        name: None,
        entry: None,
        workspace: None,
        cargo_workspace_path: None,
        declarations: Vec::new(),
    };
    match kind {
        ContainerKind::Cargo => cargo(&mut item, &text.parse().ok()?),
        ContainerKind::Npm => npm(&mut item, &serde_json::from_str(text).ok()?)?,
        ContainerKind::Python => python(&mut item, &text.parse().ok()?),
        ContainerKind::Folder => return None,
    }
    Some(item)
}

fn string(value: Option<&Toml>) -> Option<&str> {
    value.and_then(Toml::as_str)
}

fn toml_strings(value: Option<&Toml>) -> impl Iterator<Item = &str> {
    value
        .and_then(Toml::as_array)
        .into_iter()
        .flatten()
        .filter_map(Toml::as_str)
}

fn subtable<'a>(table: &'a toml::Table, key: &str) -> Option<&'a toml::Table> {
    table.get(key)?.as_table()
}

fn cargo(item: &mut Manifest<'_>, table: &toml::Table) {
    if let Some(package) = subtable(table, "package") {
        item.name = string(package.get("name"))
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        item.cargo_workspace_path = string(package.get("workspace")).map(str::to_owned);
    }
    item.entry = subtable(table, "lib")
        .and_then(|lib| string(lib.get("path")))
        .and_then(|path| local_path(item.dir, path));
    if let Some(workspace) = subtable(table, "workspace") {
        item.workspace = Some(toml_workspace(workspace));
        cargo_declarations(
            &mut item.declarations,
            workspace.get("dependencies"),
            "workspace.dependencies",
        );
    }
    for &section in CARGO_SECTIONS {
        cargo_declarations(&mut item.declarations, table.get(section), section);
    }
    if let Some(targets) = subtable(table, "target") {
        for (target, config) in targets {
            for &section in CARGO_SECTIONS {
                cargo_declarations(
                    &mut item.declarations,
                    config.get(section),
                    &format!("target.{target}.{section}"),
                );
            }
        }
    }
}

fn cargo_declarations(out: &mut Vec<Declaration>, value: Option<&Toml>, section: &str) {
    let Some(table) = value.and_then(Toml::as_table) else {
        return;
    };
    for (alias, entry) in table {
        if !entry.is_str() && !entry.is_table() {
            continue;
        }
        let field = |key| string(entry.get(key));
        let path = field("path");
        let git = field("git");
        let inherited = entry.get("workspace").and_then(Toml::as_bool) == Some(true);
        let source = if inherited {
            Source::Inherited
        } else if let Some(path) = path {
            Source::Path(path.to_owned())
        } else {
            // A registry or git dependency is not a sibling package merely because
            // their names match. Local Cargo dependencies need an explicit path.
            Source::Remote
        };
        let requirement = entry
            .as_str()
            .or_else(|| field("version"))
            .or(path)
            .or(git)
            .unwrap_or(if inherited { "workspace" } else { "" });
        out.push(Declaration {
            section: section.to_owned(),
            alias: alias.clone(),
            package: field("package").unwrap_or(alias).to_owned(),
            requirement: requirement.to_owned(),
            options: entry.as_table().map(|_| entry.to_string()),
            source,
        });
    }
}

fn json_strings(value: Option<&Json>) -> impl Iterator<Item = &str> {
    value
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
}

fn npm(item: &mut Manifest<'_>, json: &Json) -> Option<()> {
    let table = json.as_object()?;
    item.entry = table
        .get("main")
        .and_then(Json::as_str)
        .and_then(|path| local_path(item.dir, path));
    item.name = table
        .get("name")
        .and_then(Json::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    if let Some(workspaces) = table
        .get("workspaces")
        .filter(|value| value.is_array() || value.is_object())
    {
        let mut workspace = Workspace::default();
        for pattern in json_strings(workspaces.get("packages").or(Some(workspaces))) {
            if let Some(excluded) = pattern.strip_prefix('!') {
                workspace.exclude.extend(Pattern::new(excluded));
            } else {
                workspace.members.extend(Pattern::new(pattern));
            }
        }
        workspace
            .exclude
            .extend(json_strings(workspaces.get("exclude")).filter_map(Pattern::new));
        item.workspace = Some(workspace);
    }
    for section in [
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
    ] {
        let Some(deps) = table.get(section).and_then(Json::as_object) else {
            continue;
        };
        for (alias, entry) in deps {
            let Some(requirement) = entry.as_str() else {
                continue;
            };
            let (package, source) = npm_source(alias, requirement);
            item.declarations.push(Declaration {
                section: section.to_owned(),
                alias: alias.clone(),
                package: package.to_owned(),
                requirement: requirement.to_owned(),
                options: None,
                source,
            });
        }
    }
    Some(())
}

fn has_relative_prefix(path: &str) -> bool {
    path.starts_with("./") || path.starts_with("../") || path == "." || path == ".."
}

fn npm_source<'a>(alias: &'a str, requirement: &'a str) -> (&'a str, Source) {
    // For scoped packages skip the leading @ when splitting off a version.
    let name = |spec: &'a str| {
        let start = usize::from(spec.starts_with('@'));
        let package = spec[start..]
            .find('@')
            .map_or(spec, |end| &spec[..start + end]);
        if package.is_empty() { alias } else { package }
    };
    if let Some(spec) = requirement.strip_prefix("npm:") {
        (name(spec), Source::Name)
    } else if let Some(spec) = requirement.strip_prefix("workspace:") {
        if has_relative_prefix(spec) {
            (alias, Source::Path(spec.to_owned()))
        } else if spec == "*"
            || spec.starts_with(['^', '~', '<', '>', '='])
            || spec.starts_with(|c: char| c.is_ascii_digit())
            || spec.is_empty()
        {
            (alias, Source::WorkspaceName)
        } else {
            (name(spec), Source::WorkspaceName)
        }
    } else if let Some(path) = requirement
        .strip_prefix("file:")
        .or_else(|| requirement.strip_prefix("link:"))
    {
        (alias, Source::Path(path.to_owned()))
    } else if has_relative_prefix(requirement) {
        (alias, Source::Path(requirement.to_owned()))
    } else if requirement.contains([':', '/']) {
        (alias, Source::Remote)
    } else {
        (alias, Source::Name)
    }
}

fn python(item: &mut Manifest<'_>, table: &toml::Table) {
    let project = subtable(table, "project");
    let tool = subtable(table, "tool");
    let poetry = tool.and_then(|tool| subtable(tool, "poetry"));
    let uv = tool.and_then(|tool| subtable(tool, "uv"));
    item.name = project
        .and_then(|p| string(p.get("name")))
        .or_else(|| poetry.and_then(|p| string(p.get("name"))))
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let sources = uv.and_then(|tool| subtable(tool, "sources"));
    item.workspace = uv
        .and_then(|uv| subtable(uv, "workspace"))
        .map(toml_workspace);
    if let Some(project) = project {
        python_list(
            &mut item.declarations,
            project.get("dependencies"),
            "project.dependencies",
            sources,
        );
        if let Some(optional) = subtable(project, "optional-dependencies") {
            for (group, deps) in optional {
                python_list(
                    &mut item.declarations,
                    Some(deps),
                    &format!("project.optional-dependencies.{group}"),
                    sources,
                );
            }
        }
    }
    if let Some(groups) = subtable(table, "dependency-groups") {
        for (group, deps) in groups {
            python_list(
                &mut item.declarations,
                Some(deps),
                &format!("dependency-groups.{group}"),
                sources,
            );
        }
    }
    if let Some(build) = subtable(table, "build-system") {
        python_list(
            &mut item.declarations,
            build.get("requires"),
            "build-system.requires",
            sources,
        );
    }
    if let Some(uv) = uv {
        python_list(
            &mut item.declarations,
            uv.get("dev-dependencies"),
            "tool.uv.dev-dependencies",
            sources,
        );
    }
    if let Some(poetry) = poetry {
        for section in ["dependencies", "dev-dependencies"] {
            poetry_declarations(
                &mut item.declarations,
                poetry.get(section),
                &format!("tool.poetry.{section}"),
            );
        }
        if let Some(groups) = subtable(poetry, "group") {
            for (group, config) in groups {
                poetry_declarations(
                    &mut item.declarations,
                    config.get("dependencies"),
                    &format!("tool.poetry.group.{group}.dependencies"),
                );
            }
        }
    }
}

fn python_list(
    out: &mut Vec<Declaration>,
    value: Option<&Toml>,
    section: &str,
    sources: Option<&toml::Table>,
) {
    for entry in toml_strings(value) {
        let entry = entry.trim();
        let end = entry
            .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
            .unwrap_or(entry.len());
        let (name, requirement) = entry.split_at(end);
        if !name.starts_with(|c: char| c.is_ascii_alphanumeric()) {
            continue;
        }
        let requirement = requirement.trim();
        let source_options = sources.and_then(|sources| {
            sources
                .get(name)
                .or_else(|| sources.get(&name_key(ContainerKind::Python, name)))
        });
        let source = if let Some((_, location)) = requirement
            .split(';')
            .next()
            .and_then(|r| r.split_once('@'))
        {
            let location = location.trim();
            if let Some(path) = location.strip_prefix("file:") {
                Source::Path(path.to_owned())
            } else if has_relative_prefix(location) {
                Source::Path(location.to_owned())
            } else {
                Source::Remote
            }
        } else {
            source_options.map_or(Source::Name, python_source)
        };
        out.push(Declaration {
            section: section.to_owned(),
            alias: name.to_owned(),
            package: name.to_owned(),
            requirement: requirement.to_owned(),
            options: source_options.map(Toml::to_string),
            source,
        });
    }
}

fn python_source(spec: &Toml) -> Source {
    if let Some(path) = string(spec.get("path")) {
        Source::Path(path.to_owned())
    } else if spec.get("workspace").and_then(Toml::as_bool) == Some(true) {
        Source::WorkspaceName
    } else if ["git", "url", "index", "source"]
        .iter()
        .any(|key| spec.get(key).is_some())
    {
        Source::Remote
    } else {
        Source::Name
    }
}

fn poetry_declarations(out: &mut Vec<Declaration>, value: Option<&Toml>, section: &str) {
    let Some(deps) = value.and_then(Toml::as_table) else {
        return;
    };
    for (name, spec) in deps {
        if name == "python" {
            // The interpreter constraint is not a package dependency.
            continue;
        }
        let entries = spec
            .as_array()
            .map_or(std::slice::from_ref(spec), Vec::as_slice);
        for entry in entries {
            if !entry.is_str() && !entry.is_table() {
                continue;
            }
            let requirement = entry.as_str().or_else(|| {
                ["version", "path", "git", "url"]
                    .iter()
                    .find_map(|key| string(entry.get(key)))
            });
            out.push(Declaration {
                section: section.to_owned(),
                alias: name.clone(),
                package: name.clone(),
                requirement: requirement.unwrap_or("").to_owned(),
                options: entry.as_table().map(|_| entry.to_string()),
                source: python_source(entry),
            });
        }
    }
}

fn name_key(kind: ContainerKind, name: &str) -> String {
    if kind != ContainerKind::Python {
        return name.to_owned();
    }
    // Python distribution names normalize case and collapse runs of -_.
    let mut key = String::new();
    for c in name.chars() {
        if matches!(c, '-' | '_' | '.') {
            if !key.ends_with('-') {
                key.push('-');
            }
        } else {
            key.push(c.to_ascii_lowercase());
        }
    }
    key
}

fn toml_workspace(table: &toml::Table) -> Workspace {
    Workspace {
        members: toml_strings(table.get("members"))
            .filter_map(Pattern::new)
            .collect(),
        exclude: toml_strings(table.get("exclude"))
            .filter_map(Pattern::new)
            .collect(),
    }
}

enum Pattern {
    Literal(String),
    Glob(Regex),
}

impl Pattern {
    fn new(input: &str) -> Option<Self> {
        let input = input.trim_end_matches('/');
        let mut input = input;
        while let Some(relative) = input.strip_prefix("./") {
            input = relative;
        }
        if !input.contains(['*', '?', '[', '{']) {
            return Some(Self::Literal(if input == "." {
                String::new()
            } else {
                input.to_owned()
            }));
        }
        let mut regex = String::from(r"\A");
        let mut chars = input.chars().peekable();
        let mut segment_start = true;
        let mut braces = 0;
        while let Some(c) = chars.next() {
            match c {
                '*' => {
                    let double = chars.next_if_eq(&'*').is_some();
                    if double && segment_start && chars.next_if_eq(&'/').is_some() {
                        regex.push_str("(?:[^/]+/)*");
                        continue;
                    }
                    if double && segment_start && chars.peek().is_none() {
                        regex.push_str("(?s:.*)");
                    } else {
                        regex.push_str("[^/]*");
                    }
                }
                '?' => regex.push_str("[^/]"),
                '[' => {
                    let mut class = String::new();
                    let negative = chars.next_if_eq(&'!').is_some();
                    while let Some(&next) = chars.peek() {
                        if next == ']' || next == '/' {
                            break;
                        }
                        chars.next();
                        if next != '-' {
                            class.push_str(&regex::escape(&next.to_string()));
                        } else {
                            class.push('-');
                        }
                    }
                    if !class.is_empty() && chars.next_if_eq(&']').is_some() {
                        // Set intersection keeps negated classes from crossing a slash.
                        regex.push_str(if negative { "[[^" } else { "[[" });
                        regex.push_str(&class);
                        regex.push_str("]&&[^/]]");
                    } else {
                        regex.push_str(r"\[");
                        if negative {
                            regex.push('!');
                        }
                        regex.push_str(&class);
                    }
                }
                '{' => {
                    regex.push_str("(?:");
                    braces += 1;
                }
                '}' if braces > 0 => {
                    regex.push(')');
                    braces -= 1;
                }
                ',' if braces > 0 => regex.push('|'),
                _ => regex.push_str(&regex::escape(&c.to_string())),
            }
            segment_start = c == '/';
        }
        regex.push_str(r"\z");
        Regex::new(&regex).ok().map(Self::Glob)
    }

    fn matches(&self, path: &str) -> bool {
        match self {
            Self::Literal(pattern) => pattern == path,
            Self::Glob(pattern) => pattern.is_match(path),
        }
    }
}
