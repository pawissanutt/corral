//! A Mermaid fence in an artifact's Markdown, read into the diagram the app already draws for a
//! lens (`model::system::lens::Diagram`): a flowchart or state diagram as boxes and arrows laid
//! out by the layered layout, a sequence diagram as participants and numbered messages. The app
//! reads the source itself; no script runs, nothing is fetched and no markup reaches a page, so
//! a diagram cannot reach the app's credentials. Anything it does not read (another diagram
//! type, a line it does not understand, a diagram past its bounds) is an error the view shows
//! beside the source.

use corral_types::lenses::{LensKind, LensStatus};

use crate::model::system::lens::{Diagram, Element};

/// The fence's language that names a Mermaid diagram.
pub const LANGUAGE: &str = "mermaid";
/// Source bytes read at most.
pub const MAX_SOURCE: usize = 16 * 1024;
/// Boxes or participants drawn at most.
pub const MAX_NODES: usize = 80;
/// Arrows or messages drawn at most.
pub const MAX_EDGES: usize = 160;

/// Whether a fence's language names a Mermaid diagram.
pub fn is_mermaid(language: Option<&str>) -> bool {
    language.is_some_and(|language| language.trim().eq_ignore_ascii_case(LANGUAGE))
}

/// The diagram `source` describes, or why it is not drawn.
pub fn parse(source: &str) -> Result<Diagram, String> {
    if source.len() > MAX_SOURCE {
        return Err(format!(
            "it is longer than the {} KiB the app draws",
            MAX_SOURCE / 1024
        ));
    }
    let mut lines = statements(source);
    let Some((_, header)) = lines.next() else {
        return Err("it is empty".into());
    };
    let kind = header
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mut built = Built::default();
    let diagram = match kind.as_str() {
        "graph" | "flowchart" => {
            for (number, line) in lines {
                flow_line(&mut built, line).map_err(|why| at(number, &why))?;
            }
            LensKind::State
        }
        "statediagram" | "statediagram-v2" => {
            let mut in_note = false;
            for (number, line) in lines {
                state_line(&mut built, line, &mut in_note).map_err(|why| at(number, &why))?;
            }
            LensKind::State
        }
        "sequencediagram" => {
            for (number, line) in lines {
                sequence_line(&mut built, line).map_err(|why| at(number, &why))?;
            }
            LensKind::Sequence
        }
        _ => return Err(format!("“{kind}” diagrams are not drawn here")),
    };
    if built.nodes.is_empty() {
        return Err("it names nothing to draw".into());
    }
    Ok(built.diagram(diagram))
}

fn at(number: usize, why: &str) -> String {
    format!("line {number}: {why}")
}

/// The source's lines that say something, numbered from one: without front matter, comments
/// and blank lines.
fn statements(source: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut front = false;
    source.lines().enumerate().filter_map(move |(at, line)| {
        let line = line.trim();
        if at == 0 && line == "---" {
            front = true;
            return None;
        }
        if front {
            front = line != "---";
            return None;
        }
        (!line.is_empty() && !line.starts_with("%%")).then_some((at + 1, line))
    })
}

#[derive(Default)]
struct Built {
    ids: Vec<String>,
    nodes: Vec<Element>,
    edges: Vec<Element>,
}

impl Built {
    /// The node `id`'s index, made on first use; a label given later names it.
    fn node(&mut self, id: &str, label: Option<String>, what: &str) -> Result<usize, String> {
        if let Some(at) = self.ids.iter().position(|known| known == id) {
            if let Some(label) = label {
                self.nodes[at].label = label;
            }
            return Ok(at);
        }
        if self.nodes.len() >= MAX_NODES {
            return Err(format!("more than {MAX_NODES} boxes"));
        }
        self.ids.push(id.to_owned());
        self.nodes.push(element(
            format!("node {}", self.nodes.len()),
            what,
            label.unwrap_or_else(|| id.to_owned()),
        ));
        Ok(self.nodes.len() - 1)
    }

    fn edge(&mut self, from: usize, to: usize, label: String, what: String) -> Result<(), String> {
        if self.edges.len() >= MAX_EDGES {
            return Err(format!("more than {MAX_EDGES} arrows"));
        }
        let mut edge = element(format!("edge {}", self.edges.len()), &what, label);
        edge.ends = Some((from, to));
        self.edges.push(edge);
        Ok(())
    }

    fn diagram(self, kind: LensKind) -> Diagram {
        Diagram {
            kind,
            id: LANGUAGE.into(),
            title: "Mermaid diagram".into(),
            file: String::new(),
            status: LensStatus::Current,
            read_at: String::new(),
            commit: None,
            nodes: self.nodes,
            edges: self.edges,
            about: Vec::new(),
            failures: Vec::new(),
            not_checked: Vec::new(),
            missing: Vec::new(),
            extra: Vec::new(),
            excerpts: Vec::new(),
        }
    }
}

fn element(name: String, what: &str, label: String) -> Element {
    Element {
        name,
        what: what.to_owned(),
        label,
        ends: None,
        detail: None,
        anchor: None,
        location: None,
        marks: Vec::new(),
    }
}

/// Whether the line starts with one of `words` as a whole word.
fn starts_with_word(line: &str, words: &[&str]) -> bool {
    let first = line
        .split(|c: char| c.is_whitespace() || c == '{' || c == '[')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    words.contains(&first.as_str())
}

/// A label as it reads: without quotes, Markdown-string backticks and line-break tags.
fn label(raw: &str) -> String {
    let trimmed = raw.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(trimmed);
    let unticked = unquoted
        .strip_prefix('`')
        .and_then(|rest| rest.strip_suffix('`'))
        .unwrap_or(unquoted);
    let mut words = unticked.to_owned();
    for tag in ["<br/>", "<br />", "<br>"] {
        words = words.replace(tag, " ");
    }
    words.split_whitespace().collect::<Vec<_>>().join(" ")
}

// Flowcharts.

const FLOW_SKIP: &[&str] = &[
    "classdef",
    "class",
    "style",
    "linkstyle",
    "click",
    "direction",
    "subgraph",
    "end",
];

fn flow_line(built: &mut Built, line: &str) -> Result<(), String> {
    for statement in split_outside(line, ';') {
        let statement = statement.trim();
        if statement.is_empty() || starts_with_word(statement, FLOW_SKIP) {
            continue;
        }
        flow_statement(built, statement)?;
    }
    Ok(())
}

/// `text` split at `separator` where it stands outside brackets and quotes.
fn split_outside(text: &str, separator: char) -> Vec<&str> {
    let (mut parts, mut depth, mut quoted, mut start) = (Vec::new(), 0_i32, false, 0);
    for (at, c) in text.char_indices() {
        match c {
            '"' => quoted = !quoted,
            '[' | '(' | '{' if !quoted => depth += 1,
            ']' | ')' | '}' if !quoted => depth -= 1,
            c if c == separator && !quoted && depth <= 0 => {
                parts.push(&text[start..at]);
                start = at + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

/// One chain: `A[x] & B --> |words| C -- more --> D`.
fn flow_statement(built: &mut Built, statement: &str) -> Result<(), String> {
    let mut rest = statement;
    let mut left = flow_group(built, &mut rest)?;
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            return Ok(());
        }
        let words = arrow(&mut rest).ok_or_else(|| format!("“{}” is not understood", rest))?;
        let right = flow_group(built, &mut rest)?;
        for &from in &left {
            for &to in &right {
                built.edge(from, to, words.clone(), "Arrow".into())?;
            }
        }
        left = right;
    }
}

/// `A & B[x]`: the nodes one end of an arrow names.
fn flow_group(built: &mut Built, rest: &mut &str) -> Result<Vec<usize>, String> {
    let mut group = vec![flow_node(built, rest)?];
    loop {
        let trimmed = rest.trim_start();
        let Some(after) = trimmed.strip_prefix('&') else {
            return Ok(group);
        };
        *rest = after;
        group.push(flow_node(built, rest)?);
    }
}

fn flow_node(built: &mut Built, rest: &mut &str) -> Result<usize, String> {
    let text = rest.trim_start();
    let end = text
        .char_indices()
        .find(|(_, c)| !(c.is_alphanumeric() || matches!(c, '_' | ':' | '$')))
        .map_or(text.len(), |(at, _)| at);
    // An id may hold a hyphen, but an arrow starts with one: keep the hyphens before a letter.
    let mut end = end;
    while text[end..].starts_with('-')
        && text[end + 1..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric())
    {
        end += 1;
        end += text[end..]
            .char_indices()
            .find(|(_, c)| !(c.is_alphanumeric() || *c == '_'))
            .map_or(text.len() - end, |(at, _)| at);
    }
    if end == 0 {
        return Err(format!("“{text}” does not start with a node"));
    }
    let id = &text[..end];
    let mut after = &text[end..];
    let shaped = shape(&mut after)?;
    *rest = after;
    built.node(id, shaped, "Node")
}

/// A node's shape and its words: `[x]`, `(x)`, `{x}`, `((x))`, `[(x)]`, `>x]` and the rest.
fn shape(rest: &mut &str) -> Result<Option<String>, String> {
    let Some(open) = rest
        .chars()
        .next()
        .filter(|c| matches!(c, '[' | '(' | '{' | '>'))
    else {
        return Ok(None);
    };
    let close = match open {
        '[' | '>' => ']',
        '(' => ')',
        _ => '}',
    };
    let (mut depth, mut quoted) = (0_i32, false);
    for (at, c) in rest.char_indices() {
        match c {
            '"' => quoted = !quoted,
            c if c == open && open != '>' && !quoted => depth += 1,
            c if c == close && !quoted => {
                depth -= 1;
                if depth <= 0 {
                    let inner = &rest[open.len_utf8()..at];
                    let inner = inner
                        .trim_start_matches(['[', '(', '{', '/', '\\'])
                        .trim_end_matches([']', ')', '}', '/', '\\']);
                    *rest = &rest[at + 1..];
                    // A doubled shape (`((x))`, `[[x]]`) closes with its last bracket.
                    *rest = rest.trim_start_matches([')', ']', '}']);
                    return Ok(Some(label(inner)));
                }
            }
            _ => {}
        }
    }
    Err("a node's shape is not closed".into())
}

/// The arrow at the start of `rest` and its words (`-->`, `-.->`, `==>`, `-- x -->`,
/// `-->|x|`); none when no arrow starts there.
fn arrow(rest: &mut &str) -> Option<String> {
    let text = *rest;
    let run = text
        .char_indices()
        .find(|(_, c)| !matches!(c, '-' | '=' | '.' | '<' | '>' | '~'))
        .map_or(text.len(), |(at, _)| at);
    let mut end = run;
    let body = &text[..run];
    if run < 2 || !body.contains(['-', '=', '~']) {
        return None;
    }
    // A circle or cross head stands right after the line, before a space.
    if body.ends_with(['-', '=']) && text[run..].starts_with(['o', 'x']) {
        let next = text[run + 1..].chars().next();
        if next.is_none_or(|c| c.is_whitespace()) {
            end += 1;
        }
    }
    let mut words = String::new();
    let mut after = &text[end..];
    // Words inside the arrow: `-- words -->`, `== words ==>`, `-. words .->`.
    if matches!(body, "--" | "==" | "-.") && after.starts_with(char::is_whitespace) {
        let ends = ["-->", "---", "==>", "===", ".->", ".-", "--o", "--x"];
        let (at, ending) = ends
            .iter()
            .filter_map(|ending| after.find(ending).map(|at| (at, *ending)))
            .min_by_key(|(at, _)| *at)?;
        words = label(&after[..at]);
        after = &after[at + ending.len()..];
        after = after.trim_start_matches(['-', '=', '.', '>']);
    }
    let trimmed = after.trim_start();
    if let Some(piped) = trimmed.strip_prefix('|') {
        let close = piped.find('|')?;
        words = label(&piped[..close]);
        after = &piped[close + 1..];
    }
    *rest = after;
    Some(words)
}

// State diagrams.

const STATE_SKIP: &[&str] = &["direction", "classdef", "class", "--", "}", "style"];
const START: &str = "[*] start";
const END: &str = "[*] end";

fn state_line(built: &mut Built, line: &str, in_note: &mut bool) -> Result<(), String> {
    let lower = line.to_ascii_lowercase();
    if *in_note {
        *in_note = lower != "end note";
        return Ok(());
    }
    if lower.starts_with("note ") {
        // A one-line note has its words after a colon; a longer one runs to `end note`.
        *in_note = !line.contains(':');
        return Ok(());
    }
    if starts_with_word(line, STATE_SKIP) || line == "}" {
        return Ok(());
    }
    if let Some((from, to)) = line.split_once("-->") {
        let (to, words) = match to.split_once(':') {
            Some((to, words)) => (to, label(words)),
            None => (to, String::new()),
        };
        let from = state_end(built, from.trim(), START)?;
        let to = state_end(built, to.trim(), END)?;
        return built.edge(from, to, words, "Transition".into());
    }
    if let Some(declared) = line.strip_prefix("state ") {
        let declared = declared.trim().trim_end_matches('{').trim();
        // `state "Long words" as Id`, `state Id <<fork>>`, `state Id`.
        if let Some((words, id)) = declared.split_once(" as ") {
            built.node(id.trim(), Some(label(words)), "State")?;
        } else {
            let id = declared.split_whitespace().next().unwrap_or_default();
            if id.is_empty() {
                return Err("a state without a name".into());
            }
            built.node(id, None, "State")?;
        }
        return Ok(());
    }
    if let Some((id, words)) = line.split_once(':') {
        built.node(state_id(id.trim())?, Some(label(words)), "State")?;
        return Ok(());
    }
    built.node(state_id(line)?, None, "State").map(|_| ())
}

/// A transition's end: `[*]` is the start on the left and the end on the right.
fn state_end(built: &mut Built, id: &str, star: &str) -> Result<usize, String> {
    if id == "[*]" {
        let words = if star == START { "Start" } else { "End" };
        return built.node(star, Some(words.into()), "State");
    }
    built.node(state_id(id)?, None, "State")
}

fn state_id(id: &str) -> Result<&str, String> {
    let valid = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if valid {
        Ok(id)
    } else {
        Err(format!("“{id}” is not understood"))
    }
}

// Sequence diagrams.

const SEQUENCE_SKIP: &[&str] = &[
    "autonumber",
    "activate",
    "deactivate",
    "note",
    "loop",
    "alt",
    "else",
    "opt",
    "par",
    "and",
    "critical",
    "option",
    "break",
    "rect",
    "end",
    "box",
    "title",
    "link",
    "links",
    "create",
    "destroy",
];
/// Message arrows, longest first so `-->>` is not read as `-->`.
const MESSAGES: &[&str] = &[
    "<<-->>", "<<->>", "-->>", "->>", "-->", "--x", "--)", "->", "-x", "-)",
];

fn sequence_line(built: &mut Built, line: &str) -> Result<(), String> {
    for word in ["participant ", "actor "] {
        if line.len() > word.len() && line[..word.len()].eq_ignore_ascii_case(word) {
            let declared = line[word.len()..].trim();
            let (id, words) = match declared.split_once(" as ") {
                Some((id, words)) => (id.trim(), Some(label(words))),
                None => (declared, None),
            };
            built.node(&label(id), words, "Participant")?;
            return Ok(());
        }
    }
    if starts_with_word(line, SEQUENCE_SKIP) {
        return Ok(());
    }
    let (at, sign) = MESSAGES
        .iter()
        .filter_map(|sign| line.find(sign).map(|at| (at, *sign)))
        .min_by_key(|(at, sign)| (*at, std::cmp::Reverse(sign.len())))
        .ok_or_else(|| format!("“{line}” is not understood"))?;
    let from = line[..at].trim();
    let (to, words) = line[at + sign.len()..]
        .split_once(':')
        .ok_or_else(|| format!("the message “{line}” has no colon"))?;
    let to = to.trim().trim_start_matches(['+', '-']).trim();
    if from.is_empty() || to.is_empty() {
        return Err(format!("“{line}” names no participant"));
    }
    let from = built.node(&label(from), None, "Participant")?;
    let to = built.node(&label(to), None, "Participant")?;
    let what = format!("Message {}", built.edges.len() + 1);
    built.edge(from, to, label(words), what)
}

#[cfg(test)]
#[path = "mermaid_tests.rs"]
mod tests;
