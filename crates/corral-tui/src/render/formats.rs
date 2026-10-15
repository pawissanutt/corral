//! Highlighters for JSON, TOML, YAML, diffs, CSV and generic code. Pure text -> styled lines.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::wrap::{self, Seg};
use crate::theme::{CYAN, DIM, FG, GREEN, MAGENTA, ORANGE, RED};

pub(crate) fn expand_tabs(s: &str) -> String {
    s.replace('\t', "    ")
}

fn st(c: ratatui::style::Color) -> Style {
    Style::new().fg(c)
}

/// Lines for a fenced code block: known data formats are highlighted as such, anything else as code.
pub(crate) fn render_lang(lang: &str, text: &str) -> Vec<Line<'static>> {
    match lang.to_ascii_lowercase().as_str() {
        "json" | "jsonc" => text.lines().map(json_line).collect(),
        "toml" => toml_lines(text),
        "yaml" | "yml" => yaml_lines(text),
        "diff" | "patch" => diff_lines(text),
        l => code_lines(l, text),
    }
}

fn is_json_number(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E')
}

pub(crate) fn json_line(l: &str) -> Line<'static> {
    let chars: Vec<char> = expand_tabs(l).chars().collect();
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let mut j = i + 1;
        let style = if c == '"' {
            while j < chars.len() && chars[j] != '"' {
                j += if chars[j] == '\\' { 2 } else { 1 };
            }
            j = (j + 1).min(chars.len());
            let key = chars[j..].iter().find(|c| !c.is_whitespace()) == Some(&':');
            st(if key { CYAN } else { GREEN })
        } else if is_json_number(c) {
            while j < chars.len() && is_json_number(chars[j]) {
                j += 1;
            }
            st(ORANGE)
        } else if c.is_alphabetic() {
            while j < chars.len() && chars[j].is_alphabetic() {
                j += 1;
            }
            st(MAGENTA)
        } else if c.is_whitespace() {
            st(FG)
        } else {
            st(DIM)
        };
        out.push(Span::styled(chars[i..j].iter().collect::<String>(), style));
        i = j;
    }
    Line::from(out)
}

/// Pretty-printed, highlighted JSON; text that does not parse is highlighted as written.
pub(crate) fn json_lines(text: &str) -> Vec<Line<'static>> {
    match serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
    {
        Some(p) => p.lines().map(json_line).collect(),
        None => text.lines().map(json_line).collect(),
    }
}

fn value_spans(v: &str) -> Vec<Span<'static>> {
    let t = v.trim();
    let lead = &v[..v.len() - v.trim_start().len()];
    let mut out = vec![Span::raw(lead.to_string())];
    if t.starts_with('#') {
        out.push(Span::styled(t.to_string(), st(DIM)));
    } else if t.starts_with('"') || t.starts_with('\'') {
        out.push(Span::styled(t.to_string(), st(GREEN)));
    } else if t.starts_with('[') || t.starts_with('{') {
        out.extend(json_line(t).spans);
    } else if matches!(t, "true" | "false" | "null" | "~") {
        out.push(Span::styled(t.to_string(), st(MAGENTA)));
    } else if !t.is_empty() && t.parse::<f64>().is_ok() {
        out.push(Span::styled(t.to_string(), st(ORANGE)));
    } else {
        out.push(Span::styled(t.to_string(), st(FG)));
    }
    out
}

pub(crate) fn toml_line(raw: &str) -> Line<'static> {
    let l = expand_tabs(raw);
    let t = l.trim_start();
    let indent = Span::raw(l[..l.len() - t.len()].to_string());
    if t.starts_with('#') {
        return Line::from(vec![indent, Span::styled(t.to_string(), st(DIM))]);
    }
    if t.starts_with('[') {
        let s = st(CYAN).add_modifier(Modifier::BOLD);
        return Line::from(vec![indent, Span::styled(t.to_string(), s)]);
    }
    match t.split_once('=') {
        Some((k, v)) => {
            let key = k.trim_end();
            let mut spans = vec![indent, Span::styled(key.to_string(), st(CYAN))];
            spans.push(Span::styled(format!("{}=", &k[key.len()..]), st(DIM)));
            spans.extend(value_spans(v));
            Line::from(spans)
        }
        None => Line::from(vec![indent, Span::styled(t.to_string(), st(FG))]),
    }
}

fn toml_lines(text: &str) -> Vec<Line<'static>> {
    text.lines().map(toml_line).collect()
}

pub(crate) fn yaml_line(raw: &str) -> Line<'static> {
    let l = expand_tabs(raw);
    let t = l.trim_start();
    let mut spans = vec![Span::raw(l[..l.len() - t.len()].to_string())];
    if t.starts_with('#') || t.starts_with("---") {
        spans.push(Span::styled(t.to_string(), st(DIM)));
        return Line::from(spans);
    }
    let mut rest = t;
    while let Some(r) = rest.strip_prefix("- ") {
        spans.push(Span::styled("- ", st(DIM)));
        rest = r;
    }
    let split = rest
        .find(": ")
        .map(|i| (i, 2))
        .or_else(|| rest.strip_suffix(':').map(|r| (r.len(), 1)));
    match split {
        Some((i, n)) if !rest.starts_with('"') => {
            spans.push(Span::styled(rest[..i].to_string(), st(CYAN)));
            spans.push(Span::styled(rest[i..i + n].to_string(), st(DIM)));
            spans.extend(value_spans(&rest[i + n..]));
        }
        _ => spans.extend(value_spans(rest)),
    }
    Line::from(spans)
}

fn yaml_lines(text: &str) -> Vec<Line<'static>> {
    text.lines().map(yaml_line).collect()
}

/// Unified diff text: file headers bold, `+` green, `-` red, `@@` cyan, metadata dim.
pub(crate) fn diff_line(l: &str) -> Line<'static> {
    let style = if l.starts_with("+++") || l.starts_with("---") {
        st(FG).add_modifier(Modifier::BOLD)
    } else if l.starts_with("diff ") || l.starts_with("index ") || l.starts_with("...") {
        st(DIM)
    } else if l.starts_with('+') {
        st(GREEN)
    } else if l.starts_with('-') {
        st(RED)
    } else if l.starts_with("@@") {
        st(CYAN)
    } else {
        st(FG)
    };
    Line::from(Span::styled(expand_tabs(l), style))
}

pub(crate) fn diff_lines(text: &str) -> Vec<Line<'static>> {
    text.lines().map(diff_line).collect()
}

const KEYWORDS: &[&str] = &[
    "fn",
    "let",
    "mut",
    "pub",
    "use",
    "mod",
    "struct",
    "enum",
    "impl",
    "trait",
    "match",
    "if",
    "else",
    "for",
    "while",
    "loop",
    "return",
    "const",
    "static",
    "self",
    "Self",
    "async",
    "await",
    "def",
    "class",
    "import",
    "from",
    "as",
    "in",
    "not",
    "and",
    "or",
    "lambda",
    "function",
    "var",
    "export",
    "type",
    "interface",
    "func",
    "package",
    "go",
    "defer",
    "select",
    "case",
    "switch",
    "break",
    "continue",
    "true",
    "false",
    "null",
    "None",
    "True",
    "False",
    "nil",
    "where",
    "ref",
    "move",
];

fn comment_marker(lang: &str) -> &'static str {
    match lang {
        "py" | "python" | "sh" | "bash" | "zsh" | "rb" | "ruby" | "yaml" | "toml" | "conf" => "#",
        "sql" | "lua" => "--",
        _ => "//",
    }
}

pub(crate) fn code_line(lang: &str, raw: &str) -> Line<'static> {
    let line = expand_tabs(raw);
    let chars: Vec<char> = line.chars().collect();
    let marker: Vec<char> = comment_marker(lang).chars().collect();
    let quotes: &[char] = if matches!(lang, "rs" | "rust") {
        &['"']
    } else {
        &['"', '\'']
    };
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let mut j = i + 1;
        let style = if chars[i..].starts_with(&marker) {
            j = chars.len();
            st(DIM).add_modifier(Modifier::ITALIC)
        } else if quotes.contains(&c) {
            while j < chars.len() && chars[j] != c {
                j += if chars[j] == '\\' { 2 } else { 1 };
            }
            j = (j + 1).min(chars.len());
            st(GREEN)
        } else if c.is_ascii_digit() {
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '.') {
                j += 1;
            }
            st(ORANGE)
        } else if c.is_alphabetic() || c == '_' {
            while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            let word: String = chars[i..j].iter().collect();
            if KEYWORDS.contains(&word.as_str()) {
                st(MAGENTA)
            } else {
                st(FG)
            }
        } else {
            st(FG)
        };
        out.push(Span::styled(chars[i..j].iter().collect::<String>(), style));
        i = j;
    }
    Line::from(out)
}

/// Light, line-based highlighting for source files and unlabeled code blocks.
pub(crate) fn code_lines(lang: &str, text: &str) -> Vec<Line<'static>> {
    text.lines().map(|l| code_line(lang, l)).collect()
}

fn csv_row(line: &str) -> Vec<String> {
    let mut cells = vec![String::new()];
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
                if let Some(cell) = cells.last_mut() {
                    cell.push('"');
                }
            }
            '"' => quoted = !quoted,
            ',' if !quoted => cells.push(String::new()),
            _ => {
                if let Some(cell) = cells.last_mut() {
                    cell.push(c);
                }
            }
        }
    }
    cells
}

/// CSV as a box table fitted to `width` (first row is the header).
pub(crate) fn csv_table(text: &str, width: usize) -> Vec<Line<'static>> {
    let rows: Vec<Vec<Vec<Seg>>> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| csv_row(l).into_iter().map(|c| vec![(c, st(FG))]).collect())
        .collect();
    wrap::table(&rows, width, true)
}

pub(crate) fn plain_lines(text: &str) -> Vec<Line<'static>> {
    text.lines().map(|l| Line::from(expand_tabs(l))).collect()
}
