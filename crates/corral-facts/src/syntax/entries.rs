//! Source-only entry-point probes. The shared lexer keeps comments and literals
//! from supplying code tokens to these probes or to the reference/process scanner.

use std::sync::LazyLock;

use heck::{ToKebabCase, ToLowerCamelCase, ToShoutySnakeCase, ToSnakeCase, ToUpperCamelCase};
use regex::Regex;

use super::{Scan, eat, take_id};
use crate::model::{EntryDetector, EntryKind};

const ID: &str = r"(?:\p{XID_Start}|[_$])(?:\p{XID_Continue}|[$])*";
const DOUBLE: &str = r#""(?:\\(?:\r\n|.)|[^"\\\r\n])*""#;
const SINGLE: &str = r"'(?:\\(?:\r\n|.)|[^'\\\r\n])*'";
const LINE_COMMENT: &str = r"//[^\r\n]*";

static RUST: LazyLock<Regex> = LazyLock::new(|| {
    lexer(&[
        LINE_COMMENT,
        r"/\*",
        r#"[bc]?r#{0,255}""#,
        r#""(?:\\.|[^"\\])*""#,
        r"'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\r\n])'",
        r"r#\p{XID_Continue}+",
        ID,
        "::|->",
    ])
});
static JS: LazyLock<Regex> = LazyLock::new(|| {
    lexer(&[
        LINE_COMMENT,
        r"/\*.*?\*/|/\*",
        DOUBLE,
        SINGLE,
        "`",
        ID,
        r"\?\.|=>|&&|\|\|",
    ])
});
static PYTHON: LazyLock<Regex> = LazyLock::new(|| {
    lexer(&[
        r"\#[^\r\n]*",
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

#[derive(Clone, Copy)]
pub(super) enum Lexer {
    Rust,
    JavaScript,
    Python,
}

pub(super) fn tokens(language: Lexer, text: &str) -> Vec<&str> {
    let regex = match language {
        Lexer::Rust => &*RUST,
        Lexer::JavaScript => &*JS,
        Lexer::Python => &*PYTHON,
    };
    let mut result = Vec::new();
    let mut matches = regex.find_iter(text);
    let mut base = 0;
    while let Some(m) = matches.next() {
        let token = m.as_str();
        let suffix = &text[base + m.end()..];
        let rust = matches!(language, Lexer::Rust);
        let js = matches!(language, Lexer::JavaScript);
        let extra = match token {
            "/*" if rust => block_comment_len(suffix),
            "/*" if js => suffix.len(), // Unterminated; a closed JS comment was matched whole.
            "`" if js => template_end(suffix, 0)
                .or_else(|| flat_template_end(suffix))
                .unwrap_or(0),
            "/" if js && allows_regex(result.last().copied()) => regex_end(suffix).unwrap_or(0),
            opener if rust && opener.starts_with(['r', 'b', 'c']) => {
                raw_string_end(opener, suffix).unwrap_or(0)
            }
            _ => 0,
        };
        let token = if extra == 0 {
            token
        } else {
            let start = base + m.start();
            base += m.end() + extra;
            matches = regex.find_iter(&text[base..]);
            &text[start..base]
        };
        if !token.starts_with("//")
            && !token.starts_with("/*")
            && (rust || !token.starts_with('#'))
            && !token.trim().is_empty()
        {
            result.push(token);
        }
    }
    result
}

fn raw_string_end(opener: &str, suffix: &str) -> Option<usize> {
    let opener = opener.strip_suffix('"')?;
    let hashes = opener
        .strip_prefix('r')
        .or_else(|| opener.strip_prefix("br"))
        .or_else(|| opener.strip_prefix("cr"))?;
    if !hashes.bytes().all(|b| b == b'#') {
        return None;
    }
    let end = format!("\"{hashes}");
    Some(suffix.find(&end).map_or(suffix.len(), |n| n + end.len()))
}

fn block_comment_len(body: &str) -> usize {
    let mut depth = 1;
    let mut i = 0;
    let bytes = body.as_bytes();
    while i < bytes.len() {
        match bytes.get(i..i + 2) {
            Some(b"/*") => depth += 1,
            Some(b"*/") => depth -= 1,
            _ => {
                i += 1;
                continue;
            }
        }
        i += 2;
        if depth == 0 {
            return i;
        }
    }
    body.len()
}

// Common expression boundaries distinguish regex literals from division without a JS parser.
fn allows_regex(previous: Option<&str>) -> bool {
    matches!(
        previous,
        None | Some(
            "=" | "return"
                | "throw"
                | "("
                | "["
                | "{"
                | ","
                | ":"
                | ";"
                | "?"
                | "!"
                | "=>"
                | "&&"
                | "||"
        )
    )
}

fn regex_end(suffix: &str) -> Option<usize> {
    let mut escaped = false;
    let mut class = false;
    for (i, c) in suffix.char_indices() {
        if matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}') {
            break;
        }
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '[' => class = true,
            ']' => class = false,
            '/' if !class => {
                let flags = suffix[i + 1..]
                    .bytes()
                    .take_while(u8::is_ascii_alphabetic)
                    .count();
                return Some(i + 1 + flags);
            }
            _ => {}
        }
    }
    None
}

// If a complex interpolation exceeds the heuristic, at least recover the old
// first-unescaped-backtick behavior rather than hiding the rest of the file.
fn flat_template_end(body: &str) -> Option<usize> {
    let mut escaped = false;
    for (i, b) in body.bytes().enumerate() {
        if escaped {
            escaped = false;
        } else if b == b'\\' {
            escaped = true;
        } else if b == b'`' {
            return Some(i + 1);
        }
    }
    None
}

// Treat templates as opaque, including nested templates inside interpolation.
fn template_end(body: &str, depth: usize) -> Option<usize> {
    if depth == 64 {
        return None;
    }
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'`' => return Some(i + 1),
            b'\\' => i += 2,
            b'$' if bytes.get(i + 1) == Some(&b'{') => {
                i += 2;
                let mut braces = 1;
                let mut previous = None;
                while braces != 0 {
                    let token = JS.find_at(body, i)?;
                    i = token.end();
                    let token = token.as_str();
                    match token {
                        "{" => braces += 1,
                        "}" => braces -= 1,
                        "`" => i += template_end(&body[i..], depth + 1)?,
                        "/*" => return None,
                        "/" if allows_regex(previous) => i += regex_end(&body[i..]).unwrap_or(0),
                        token
                            if token.starts_with("//")
                                || token.starts_with("/*")
                                || token.trim().is_empty() =>
                        {
                            continue;
                        }
                        _ => {}
                    }
                    previous = Some(token);
                }
            }
            _ => i += 1,
        }
    }
    None
}

pub(super) fn literal(token: &str, template: bool) -> Option<&str> {
    let quote = token.chars().next()?;
    let content = match quote {
        '"' | '\'' => token.strip_prefix(quote)?.strip_suffix(quote)?,
        '`' if template && !token.contains("${") => token.strip_prefix('`')?.strip_suffix('`')?,
        'r' => {
            let (prefix, body) = token.strip_prefix('r')?.split_once('"')?;
            if !prefix.bytes().all(|byte| byte == b'#') {
                return None;
            }
            body.strip_suffix(prefix)?.strip_suffix('"')?
        }
        _ => return None,
    };
    (!content.is_empty() && !content.contains(['\\', '\n', '\r', '\0'])).then_some(content)
}

pub(super) fn literal_arg<'a>(tokens: &[&'a str], template: bool) -> Option<&'a str> {
    match tokens {
        ["(", source, ")" | ",", ..] => literal(source, template),
        _ => None,
    }
}

impl Scan {
    fn entry(&mut self, kind: EntryKind, name: String, token: &str, start: usize) {
        self.entries
            .push((kind, name, token.as_ptr() as usize - start));
    }

    fn route(&mut self, method: &str, path: &str, token: &str, start: usize) {
        self.entry(
            EntryKind::Route,
            format!("{} {path}", method.to_ascii_uppercase()),
            token,
            start,
        );
    }
}

pub(super) fn rust(
    tokens: &[&str],
    i: usize,
    start: usize,
    operation_enums: &[String],
    scan: &mut Scan,
) {
    match tokens[i] {
        "enum" => rust_enum(&tokens[..i], &tokens[i + 1..], start, operation_enums, scan),
        "route" if tokens[..i].ends_with(&["."]) => {
            if let Some((method, source)) = axum(&tokens[i + 1..]) {
                scan.entry_detectors.insert(EntryDetector::Axum);
                if let [token] = source
                    && let Some(path) = rust_string(token).filter(|path| path.starts_with('/'))
                {
                    scan.route(method, path, tokens[i], start);
                }
            }
        }
        _ => {}
    }
}

fn http_method(token: &str) -> bool {
    matches!(
        token,
        "connect" | "delete" | "get" | "head" | "options" | "patch" | "post" | "put" | "trace"
    )
}

fn route_args<'a, 't>(mut tokens: &'t [&'a str]) -> Option<(&'t [&'a str], &'t [&'a str])> {
    if !eat(&mut tokens, "(") {
        return None;
    }
    // Inspect only the first argument; a handler body can contain more route calls.
    let source = until_comma(&mut tokens)?;
    if source.is_empty()
        || !eat(&mut tokens, ",")
        || matches!(tokens.first(), None | Some(&("," | ")" | "]" | "}" | ";")))
    {
        return None;
    }
    Some((source, tokens))
}

fn axum<'a, 't>(tokens: &'t [&'a str]) -> Option<(&'a str, &'t [&'a str])> {
    let (source, mut handler) = route_args(tokens)?;
    let method = path_last(&mut handler)?;
    if !http_method(method) && method != "any" {
        return None;
    }
    if take_group(&mut handler, "(")?.is_empty() {
        return None;
    }
    eat(&mut handler, ",");
    handler.starts_with(&[")"]).then_some((method, source))
}

pub(super) fn express(tokens: &[&str], i: usize, start: usize, scan: &mut Scan) {
    let method = tokens[i];
    if !(http_method(method) || method == "all") {
        return;
    }
    let [head @ .., "app", "."] = &tokens[..i] else {
        return;
    };
    if matches!(head.last(), Some(&("." | "?."))) {
        return;
    }
    if let Some((source, _)) = route_args(&tokens[i + 1..]) {
        scan.entry_detectors.insert(EntryDetector::Express);
        if let [token] = source
            && let Some(path) = literal(token, true).filter(|path| path.starts_with('/'))
        {
            scan.route(method, path, tokens[i], start);
        }
    }
}

fn rust_string(token: &str) -> Option<&str> {
    token
        .starts_with(['"', 'r'])
        .then(|| literal(token, false))
        .flatten()
}

fn path_last<'a>(tokens: &mut &[&'a str]) -> Option<&'a str> {
    eat(tokens, "::");
    loop {
        let name = take_id(tokens)?;
        if !eat(tokens, "::") {
            return Some(name);
        }
    }
}

fn rust_enum(
    before: &[&str],
    mut after: &[&str],
    start: usize,
    operation_enums: &[String],
    scan: &mut Scan,
) {
    let Some(name) = take_id(&mut after) else {
        return;
    };
    let mut clap = Clap::default();
    for attr in item_attributes(before).into_iter().rev() {
        clap.attribute(attr);
    }
    let operation = operation_enums.iter().any(|configured| configured == name);
    if !clap.derive && !operation {
        return;
    }
    let Some(mut body) = enum_body(after) else {
        return;
    };
    if clap.derive {
        scan.entry_detectors.insert(EntryDetector::Clap);
    }
    if operation {
        scan.entry_detectors.insert(EntryDetector::OperationEnum);
    }
    while !body.is_empty() {
        let mut variant = Clap::default();
        while eat(&mut body, "#") {
            let Some(attr) = take_group(&mut body, "[") else {
                return;
            };
            if clap.derive {
                variant.attribute(attr);
            }
        }
        let Some(&token) = body.first() else {
            break;
        };
        let Some(name) = take_id(&mut body) else {
            return;
        };
        if !variant_tail(&mut body) {
            return;
        }
        if clap.derive
            && !variant.unnamed
            && let Some(name) = variant.command_name(clap.casing, name)
        {
            scan.entry(EntryKind::Command, name, token, start);
        }
        if operation {
            scan.entry(EntryKind::Operation, name.to_owned(), token, start);
        }
    }
}

pub(super) fn enum_body<'a, 't>(mut rest: &'t [&'a str]) -> Option<&'t [&'a str]> {
    if rest.starts_with(&["<"]) {
        skip_angles(&mut rest)?;
    }
    if eat(&mut rest, "where") {
        while let Some(&token) = rest.first() {
            match token {
                "{" => break,
                ";" | "}" | "enum" => return None,
                "(" | "[" => {
                    take_group(&mut rest, token)?;
                }
                "<" => skip_angles(&mut rest)?,
                _ => rest = &rest[1..],
            }
        }
    }
    take_group(&mut rest, "{")
}

pub(super) fn variant_tail(rest: &mut &[&str]) -> bool {
    if let Some(&token @ ("(" | "{")) = rest.first()
        && take_group(rest, token).is_none()
    {
        return false;
    }
    if eat(rest, "=") {
        // A qualified path may start a discriminant: <Type as Trait<A, B>>::VALUE.
        if rest.starts_with(&["<"]) && skip_angles(rest).is_none() {
            return false;
        }
        if until_comma(rest).is_none() {
            return false;
        }
    }
    rest.is_empty() || eat(rest, ",")
}

// Take a token group without descending into nested field or expression syntax.
pub(super) fn take_group<'a, 't>(rest: &mut &'t [&'a str], opening: &str) -> Option<&'t [&'a str]> {
    if rest.first() != Some(&opening) {
        return None;
    }
    let mut stack = Vec::new();
    for (i, &token) in rest.iter().enumerate() {
        match token {
            "(" => stack.push(")"),
            "[" => stack.push("]"),
            "{" => stack.push("}"),
            ")" | "]" | "}" if stack.pop() != Some(token) => return None,
            _ => {}
        }
        if stack.is_empty() {
            let inside = &rest[1..i];
            *rest = &rest[i + 1..];
            return Some(inside);
        }
    }
    None
}

fn skip_angles(rest: &mut &[&str]) -> Option<()> {
    if !eat(rest, "<") {
        return None;
    }
    let mut depth = 1;
    while let Some((&token, tail)) = rest.split_first() {
        match token {
            "(" | "[" | "{" => {
                take_group(rest, token)?;
            }
            "<" => {
                *rest = tail;
                depth += 1;
            }
            ">" => {
                *rest = tail;
                depth -= 1;
                if depth == 0 {
                    return Some(());
                }
            }
            ")" | "]" | "}" => return None,
            _ => *rest = tail,
        }
    }
    None
}

fn until_comma<'a, 't>(rest: &mut &'t [&'a str]) -> Option<&'t [&'a str]> {
    let original = *rest;
    while let Some((&token, tail)) = rest.split_first() {
        match token {
            "," | ")" | "]" | "}" | ";" => break,
            "(" | "[" | "{" => {
                take_group(rest, token)?;
            }
            "::" if tail.starts_with(&["<"]) => {
                *rest = tail;
                skip_angles(rest)?;
            }
            _ => *rest = tail,
        }
    }
    Some(&original[..original.len() - rest.len()])
}

fn item_attributes<'a, 't>(before: &'t [&'a str]) -> Vec<&'t [&'a str]> {
    let mut before = before.strip_suffix(&["pub"]).unwrap_or(before);
    if let Some((prefix, _)) = trailing_group(before, "(", ")")
        && let Some(prefix) = prefix.strip_suffix(&["pub"])
    {
        before = prefix;
    }
    let mut attrs = Vec::new();
    while let Some((prefix, group)) = trailing_group(before, "[", "]") {
        let Some(prefix) = prefix.strip_suffix(&["#"]) else {
            break;
        };
        attrs.push(group);
        before = prefix;
    }
    attrs
}

fn trailing_group<'a, 't>(
    tokens: &'t [&'a str],
    open: &str,
    close: &str,
) -> Option<(&'t [&'a str], &'t [&'a str])> {
    let (&last, inside) = tokens.split_last()?;
    if last != close {
        return None;
    }
    let mut depth = 1;
    for (i, &token) in inside.iter().enumerate().rev() {
        if token == close {
            depth += 1;
        } else if token == open {
            depth -= 1;
            if depth == 0 {
                return Some((&inside[..i], &inside[i + 1..]));
            }
        }
    }
    None
}

#[derive(Default)]
struct Clap<'a, 't> {
    derive: bool,
    unnamed: bool,
    name: Option<&'t [&'a str]>,
    casing: Option<&'t [&'a str]>,
}

impl<'a, 't> Clap<'a, 't> {
    fn attribute(&mut self, attr: &'t [&'a str]) {
        match attr {
            ["derive", "(", paths @ .., ")"] => {
                self.derive |= paths
                    .split(|token| *token == ",")
                    .any(|mut path| path_last(&mut path) == Some("Subcommand") && path.is_empty());
            }
            ["command" | "clap", "(", args @ .., ")"] => {
                let mut args = args;
                while let Some((name, value)) =
                    until_comma(&mut args).and_then(|arg| arg.split_first())
                {
                    match *name {
                        "skip" | "flatten" | "external_subcommand" => self.unnamed = true,
                        "name" => self.name = Some(value),
                        "rename_all" => self.casing = Some(value),
                        _ => {}
                    }
                    if !eat(&mut args, ",") {
                        break;
                    }
                }
            }
            _ => {}
        }
    }

    fn command_name(&self, inherited_casing: Option<&[&str]>, variant: &str) -> Option<String> {
        if let Some(name) = self.name {
            return attribute_string(name).map(str::to_owned);
        }
        let casing = match self.casing.or(inherited_casing) {
            Some(tokens) => attribute_string(tokens)?,
            None => "kebab-case",
        };
        rename(variant, casing)
    }
}

fn attribute_string<'a>(value: &[&'a str]) -> Option<&'a str> {
    match value {
        ["=", token] | ["(", token, ")"] => rust_string(token),
        _ => None,
    }
}

fn rename(name: &str, style: &str) -> Option<String> {
    // Use the same case conversions as clap_derive, including acronym and Unicode handling.
    let style = style.to_upper_camel_case().to_lowercase();
    let output = match style.strip_suffix("case").unwrap_or(&style) {
        "kebab" => name.to_kebab_case(),
        "snake" => name.to_snake_case(),
        "screamingsnake" => name.to_shouty_snake_case(),
        "camel" => name.to_lower_camel_case(),
        "pascal" => name.to_upper_camel_case(),
        "lower" => name.to_snake_case().replace('_', ""),
        "upper" => name.to_shouty_snake_case().replace('_', ""),
        "verbatim" => name.to_owned(),
        _ => return None,
    };
    (!output.is_empty()).then_some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rust_scan(text: &str) -> Scan {
        let tokens = tokens(Lexer::Rust, text);
        let mut scan = Scan::default();
        for i in 0..tokens.len() {
            rust(
                &tokens,
                i,
                text.as_ptr() as usize,
                &["Op".into()],
                &mut scan,
            );
        }
        scan
    }

    #[test]
    fn enum_entries_follow_variant_boundaries_and_clap_naming() {
        let text = r####"
            // A line comment ending with a quote: "
            #[derive(Debug, ::clap::Subcommand,)]
            #[allow(dead_code)]
            #[command(rename_all = "SCREAMING_SNAKE_CASE")]
            pub(crate) enum Op<T> where T: Into<Vec<(u8, u8)>> {
                HTTPServer { item: Result<T, Error>, other: [T; 2] },
                #[command(name(r##"real-name"##), hide = true)]
                r#type(T),
                #[clap(skip)] Skipped,
                #[command(flatten)] Placeholder(Nested),
                Shifted = 1 << 2,
            }
            #[derive(Subcommand)] enum Defaults { PiStartup, }
        "####;
        let scan = rust_scan(text);
        assert_eq!(
            scan.entry_detectors,
            [EntryDetector::Clap, EntryDetector::OperationEnum].into()
        );
        let entries: Vec<_> = scan
            .entries
            .iter()
            .map(|(kind, name, offset)| {
                let token = text[*offset..].split([' ', '(', ',', '\n']).next().unwrap();
                (*kind, name.as_str(), token)
            })
            .collect();
        assert_eq!(
            entries,
            [
                (EntryKind::Command, "HTTP_SERVER", "HTTPServer"),
                (EntryKind::Operation, "HTTPServer", "HTTPServer"),
                (EntryKind::Command, "real-name", "r#type"),
                (EntryKind::Operation, "type", "r#type"),
                (EntryKind::Operation, "Skipped", "Skipped"),
                (EntryKind::Operation, "Placeholder", "Placeholder"),
                (EntryKind::Command, "SHIFTED", "Shifted"),
                (EntryKind::Operation, "Shifted", "Shifted"),
                (EntryKind::Command, "pi-startup", "PiStartup"),
            ]
        );
        let empty = rust_scan("#[derive(clap::Subcommand)] enum Op {}");
        assert!(empty.entries.is_empty());
        assert_eq!(empty.entry_detectors, scan.entry_detectors);
    }

    #[test]
    fn routes_ignore_code_shaped_strings_and_comments() {
        let rust = rust_scan(
            r####"
            /* nested /* comment */ router.route("/comment", get(handler)) */
            let _ = r###" fake " router.route("/raw", delete(handler)) "###;
            let _ = "
            #[derive(Subcommand)] enum Fake { Quoted }
            ";
            router
                .route(
                    r##"/rust/{id}"##,
                    axum::routing::get(handler),
                )
                .route("/unsupported", choose(handler));
        "####,
        );
        assert_eq!(rust.entry_detectors, [EntryDetector::Axum].into());
        assert_eq!(rust.entries.len(), 1);
        assert_eq!(rust.entries[0].1, "GET /rust/{id}");

        let source = r#"
            const text = `${`app.get("/template", handler)`}`;
            // app.post("/comment", handler);
            service.app.post("/member", handler);
            app.get(`/dynamic/${id}`, handler);
            const regexInTemplate = `${/{/.test('')}`;
            const fake = /[app.get("/", handler)]/;
            const markdownQuote = /`/;
            app.patch("/js", handler);
        "#;
        let tokens = tokens(Lexer::JavaScript, source);
        let mut js = Scan::default();
        for i in 0..tokens.len() {
            express(&tokens, i, source.as_ptr() as usize, &mut js);
        }
        assert_eq!(js.entry_detectors, [EntryDetector::Express].into());
        assert_eq!(js.entries.len(), 1);
        assert_eq!(js.entries[0].1, "PATCH /js");
    }
}
