//! A Mermaid fence in an artifact's Markdown, read into the diagram the app already draws for a
//! lens (`model::system::lens::Diagram`): a flowchart or state diagram as boxes and arrows laid
//! out by the layered layout, a sequence diagram as participants and numbered messages, an
//! entity-relationship diagram as entities and labelled relationships (`mermaid_er.rs`). The app
//! reads the source itself; no script runs, nothing is fetched and no markup reaches a page, so
//! a diagram cannot reach the app's credentials. Anything it does not read (another diagram
//! type, a line it does not understand, a diagram past its bounds) is an error the view shows
//! beside the source.

use corral_types::lenses::{LensStatus, RecipeLayout};

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
    let Some((first, header)) = lines.next() else {
        return Err("it is empty".into());
    };
    // `flowchart TD; A --> B` puts statements on the header's line.
    let (header, tail) = header.split_once(';').unwrap_or((header, ""));
    let mut words = header.split_whitespace();
    let kind = words.next().unwrap_or_default();
    let options: Vec<&str> = words.collect();
    // Mermaid refuses a header it does not know, so the app does too rather than guess.
    let header_ok = match kind {
        "graph" | "flowchart" => options.len() <= 1 && options.iter().all(|d| direction(d)),
        _ => options.is_empty() && tail.trim().is_empty(),
    };
    if !header_ok {
        return Err(format!("the header “{}” is not understood", header.trim()));
    }
    let mut built = Built::default();
    let diagram = match kind {
        "graph" | "flowchart" => {
            for (number, line) in std::iter::once((first, tail)).chain(lines) {
                flow_line(&mut built, line).map_err(|why| at(number, &why))?;
            }
            RecipeLayout::Graph
        }
        "stateDiagram" | "stateDiagram-v2" => {
            let mut in_note = false;
            for (number, line) in lines {
                state_line(&mut built, line, &mut in_note).map_err(|why| at(number, &why))?;
            }
            RecipeLayout::Graph
        }
        "erDiagram" => {
            er::lines(&mut built, lines)?;
            RecipeLayout::Graph
        }
        "sequenceDiagram" => {
            for (number, line) in lines {
                sequence_line(&mut built, line).map_err(|why| at(number, &why))?;
            }
            RecipeLayout::OrderedLanes
        }
        _ => return Err(format!("“{kind}” diagrams are not drawn here")),
    };
    if built.nodes.is_empty() {
        return Err("it names nothing to draw".into());
    }
    Ok(built.diagram(diagram))
}

/// A flowchart's direction, as Mermaid names one.
fn direction(word: &str) -> bool {
    matches!(word, "TB" | "TD" | "BT" | "RL" | "LR")
}

/// Why a statement the canvas cannot represent is not drawn.
fn refused(statement: &str) -> String {
    let word = statement.split_whitespace().next().unwrap_or(statement);
    format!("“{word}” blocks are not drawn here")
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

    fn diagram(self, kind: RecipeLayout) -> Diagram {
        Diagram {
            kind,
            id: LANGUAGE.into(),
            title: "Mermaid diagram".into(),
            file: String::new(),
            status: LensStatus::Current,
            derived: Vec::new(),
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
        authored: Vec::new(),
        group: None,
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
];
/// Groups the canvas does not draw: a box drawn without its group would misstate the diagram.
const FLOW_REFUSED: &[&str] = &["subgraph", "end"];

fn flow_line(built: &mut Built, line: &str) -> Result<(), String> {
    for statement in split_outside(line, ';') {
        let statement = statement.trim();
        if let Some(to) = statement.strip_prefix("direction ") {
            if !direction(to.trim()) {
                return Err(format!("“{statement}” names no direction"));
            }
            continue;
        }
        if statement.is_empty() || starts_with_word(statement, FLOW_SKIP) {
            continue;
        }
        if starts_with_word(statement, FLOW_REFUSED) {
            return Err(refused(statement));
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
        let words = arrow(&mut rest)?.ok_or_else(|| format!("“{}” is not understood", rest))?;
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
        .find(|(_, c)| !(c.is_alphanumeric() || matches!(c, '_' | '$')))
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
    // `A:::name` gives the node a style class: style only, so it is skipped.
    if let Some(class) = after.strip_prefix(":::") {
        // A hyphen joins words of the name (`hot-path`) but `-->` after it is the link.
        let name = class
            .char_indices()
            .find(|&(at, c)| {
                !(c.is_alphanumeric()
                    || c == '_'
                    || (c == '-'
                        && class[at + 1..]
                            .chars()
                            .next()
                            .is_some_and(char::is_alphanumeric)))
            })
            .map_or(class.len(), |(at, _)| at);
        after = &class[name..];
    }
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
                    let inner = framed(&rest[open.len_utf8()..at], open == '>')?;
                    *rest = &rest[at + 1..];
                    return Ok(Some(label(inner)));
                }
            }
            _ => {}
        }
    }
    Err("a node's shape is not closed".into())
}

/// A shape's words inside its inner frame (`(x)` of `[(x)]`, `/x\` of `[/x\]`), which must
/// close as it opened; an asymmetric shape (`>x]`) has none. Words may end in a bracket of
/// their own (`[Run (fast)]`): only an opening frame asks for a closing one.
fn framed(inner: &str, asymmetric: bool) -> Result<&str, String> {
    let opens = inner.len() - inner.trim_start_matches(['[', '(', '{', '/', '\\']).len();
    let mut words = &inner[opens..];
    for open in inner[..opens].chars() {
        let closed = match open {
            '[' => words.strip_suffix(']'),
            '(' => words.strip_suffix(')'),
            '{' => words.strip_suffix('}'),
            _ => words.strip_suffix(['/', '\\']),
        };
        match closed {
            Some(rest) if !asymmetric => words = rest,
            _ => return Err(format!("the shape “{inner}” does not close as it opens")),
        }
    }
    Ok(words)
}

/// The line a one-way link draws: solid (`-->`), thick (`==>`) or dotted (`-.->`), of any
/// length; none for an open, two-way or invisible link, which the app's arrows would misstate.
fn one_way(link: &str) -> Option<char> {
    let line = link.strip_suffix('>')?;
    let all = |c: char| line.len() >= 2 && line.chars().all(|d| d == c);
    if all('-') {
        Some('-')
    } else if all('=') {
        Some('=')
    } else {
        let dots = line.strip_prefix('-')?.strip_suffix('-')?;
        (!dots.is_empty() && dots.chars().all(|c| c == '.')).then_some('.')
    }
}

fn not_drawn(link: &str) -> String {
    format!("the link “{link}” is not a one-way arrow, the only link the app draws")
}

/// The length of the link characters `text` starts with.
fn link_run(text: &str) -> usize {
    text.find(|c: char| !matches!(c, '-' | '=' | '.' | '<' | '>' | '~'))
        .unwrap_or(text.len())
}

/// A circle or cross head (`--o`, `--x`) right after a link's line.
fn headed(line: &str, after: &str) -> bool {
    line.ends_with(['-', '='])
        && after.starts_with(['o', 'x'])
        && after[1..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric())
}

/// The one-way link at the start of `rest` and its words (`-->`, `-.->`, `==>`, `-- x -->`,
/// `-->|x|`); none when no link starts there, an error for a link the app would misdraw.
fn arrow(rest: &mut &str) -> Result<Option<String>, String> {
    let text = *rest;
    let run = link_run(text);
    let body = &text[..run];
    if run < 2 || !body.contains(['-', '=', '~']) {
        return Ok(None);
    }
    let mut after = &text[run..];
    if headed(body, after) {
        return Err(not_drawn(&text[..run + 1]));
    }
    let mut words = String::new();
    // Words inside the link: `-- words -->`, `== words ==>`, `-. words .->`.
    if matches!(body, "--" | "==" | "-.") && after.starts_with(char::is_whitespace) {
        let at = ["--", "==", ".-"]
            .iter()
            .filter_map(|close| after.find(close))
            .min()
            .ok_or_else(|| not_drawn(body))?;
        let length = link_run(&after[at..]);
        let close = &after[at..at + length];
        let line = match body {
            "-." => one_way(&format!("-{close}")).filter(|&kind| kind == '.'),
            _ => one_way(close).filter(|&kind| body.starts_with(kind)),
        };
        if line.is_none() || headed(close, &after[at + length..]) {
            return Err(not_drawn(&format!("{body} … {close}")));
        }
        words = label(&after[..at]);
        after = &after[at + length..];
    } else if one_way(body).is_none() {
        return Err(not_drawn(body));
    }
    if let Some(piped) = after.trim_start().strip_prefix('|') {
        let close = piped
            .find('|')
            .ok_or_else(|| "a link's words are not closed".to_owned())?;
        words = label(&piped[..close]);
        after = &piped[close + 1..];
    }
    *rest = after;
    Ok(Some(words))
}

// State diagrams.

const STATE_SKIP: &[&str] = &["direction", "classdef", "class", "style"];
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
    if starts_with_word(line, STATE_SKIP) {
        return Ok(());
    }
    // Composite and concurrent states, forks, joins and choices: drawn flat, their inner
    // start and end would merge with the diagram's and a choice would read as a state.
    if line.contains(['{', '}']) || line.contains("<<") || line == "--" {
        return Err(format!("“{line}” is a structure not drawn here"));
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
        let declared = declared.trim();
        // `state "Long words" as Id`, `state Id`.
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

/// What changes no message: numbering (messages are numbered anyway), activation bars, notes,
/// the title and participants' menus.
const SEQUENCE_SKIP: &[&str] = &[
    "autonumber",
    "activate",
    "deactivate",
    "note",
    "title",
    "link",
    "links",
];
/// Blocks that make messages conditional, repeated or parallel, and participants' lifetimes:
/// drawn as plain steps they would say something else.
const SEQUENCE_REFUSED: &[&str] = &[
    "loop", "alt", "else", "opt", "par", "and", "critical", "option", "break", "rect", "end",
    "box", "create", "destroy",
];
/// Message arrows, longest first so `-->>` is not read as `-->`.
const MESSAGES: &[&str] = &[
    "<<-->>", "<<->>", "-->>", "->>", "-->", "--x", "--)", "->", "-x", "-)",
];
/// The messages the app draws faithfully as one arrow: an arrowhead or an async open head.
const ARROWS: &[&str] = &["-->>", "->>", "--)", "-)"];

fn sequence_line(built: &mut Built, line: &str) -> Result<(), String> {
    for word in ["participant ", "actor "] {
        let head = line.get(..word.len());
        if line.len() > word.len() && head.is_some_and(|head| head.eq_ignore_ascii_case(word)) {
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
    if starts_with_word(line, SEQUENCE_REFUSED) {
        return Err(refused(line));
    }
    let (at, sign) = MESSAGES
        .iter()
        .filter_map(|sign| line.find(sign).map(|at| (at, *sign)))
        .min_by_key(|(at, sign)| (*at, std::cmp::Reverse(sign.len())))
        .ok_or_else(|| format!("“{line}” is not understood"))?;
    if !ARROWS.contains(&sign) {
        return Err(format!(
            "the message “{sign}” has no single arrowhead, the only message the app draws"
        ));
    }
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

#[path = "mermaid_er.rs"]
mod er;

#[cfg(test)]
#[path = "mermaid_tests.rs"]
mod tests;
