//! Activity › System's Lenses view: a project's saved behaviour diagrams (a state lens or a
//! sequence lens), each checked against the code by corrald. The view reads `system_lenses`
//! for the list and `system_lens` for the one drawn; a task's Impact reads `system_lens` at its
//! head for a lens the change touches. A served lens becomes a [`Diagram`]: its elements in
//! file order, each with the checker's name for it (so a failure, a location or a touch lands
//! on the element it names), its anchor, its committed line and its marks.

use corral_protocol::detail::{DetailBody, DetailRef};
use corral_types::lenses::{
    Lens, LensAnchor, LensDiagnostic, LensExcerpt, LensKind, LensResult, LensStatus,
};
use corral_types::{SystemLens, SystemLenses, SystemRow, SystemStatus};

use super::words::Notice;
use crate::model::Hue;
use crate::model::daemon::Feed;

/// The `system_lenses` detail: every lens file of the project at its head.
pub fn list_request(project: &str) -> DetailRef {
    DetailRef::SystemLenses {
        project: project.to_owned(),
    }
}

/// The `system_lens` detail: one lens checked at `at`, the project head when none.
pub fn lens_request(project: &str, id: &str, at: Option<&str>) -> DetailRef {
    DetailRef::SystemLens {
        project: project.to_owned(),
        id: id.to_owned(),
        at: at.map(str::to_owned),
    }
}

pub fn list_of<'a>(feed: &'a Feed, project: &str) -> Option<&'a SystemLenses> {
    match feed.detail(&list_request(project))?.body()? {
        DetailBody::SystemLenses(list) => Some(list),
        _ => None,
    }
}

pub fn lens_of<'a>(feed: &'a Feed, target: &DetailRef) -> Option<&'a SystemLens> {
    match feed.detail(target)?.body()? {
        DetailBody::SystemLens(lens) => Some(lens),
        _ => None,
    }
}

pub const fn status_word(status: LensStatus) -> &'static str {
    status.as_str()
}

pub const fn status_hue(status: LensStatus) -> Hue {
    match status {
        LensStatus::Current => Hue::Ok,
        LensStatus::Stale => Hue::Fail,
        LensStatus::Incomplete => Hue::Held,
        LensStatus::Malformed => Hue::Fail,
    }
}

/// A status's glyph, so it differs by shape as well as colour.
pub const fn status_glyph(status: LensStatus) -> &'static str {
    match status {
        LensStatus::Current => "✓",
        LensStatus::Stale => "✗",
        LensStatus::Incomplete => "◐",
        LensStatus::Malformed => "!",
    }
}

pub const fn kind_word(kind: Option<LensKind>) -> &'static str {
    match kind {
        Some(LensKind::State) => "State",
        Some(LensKind::Sequence) => "Sequence",
        None => "Unreadable",
    }
}

/// One line of the list.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// What `system_lens` is asked for: the lens id, or the file of one that has none.
    pub key: String,
    pub title: String,
    pub kind: Option<LensKind>,
    pub status: LensStatus,
    pub file: String,
}

pub fn rows(list: &SystemLenses) -> Vec<Row> {
    list.rows
        .iter()
        .map(|row| Row {
            key: row.id.clone().unwrap_or_else(|| row.file.clone()),
            title: row.title.clone().unwrap_or_else(|| row.file.clone()),
            kind: row.kind,
            status: row.status,
            file: row.file.clone(),
        })
        .collect()
}

/// What an element carries besides what is checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mark {
    /// A check on it failed.
    Failed,
    /// A state the code's variants do not have.
    Extra,
    /// The change under review edits its anchored line or definition.
    Touched,
    /// The change under review adds it.
    Added,
}

impl Mark {
    pub const fn glyph(self) -> &'static str {
        match self {
            Mark::Failed => "✗",
            Mark::Extra => "?",
            Mark::Touched => "Δ",
            Mark::Added => "+",
        }
    }
    pub const fn word(self) -> &'static str {
        match self {
            Mark::Failed => "failing",
            Mark::Extra => "not in the code",
            Mark::Touched => "touched",
            Mark::Added => "added",
        }
    }
    pub const fn hue(self) -> Hue {
        match self {
            Mark::Failed => Hue::Fail,
            Mark::Extra => Hue::Held,
            Mark::Touched => Hue::Worker,
            Mark::Added => Hue::Ok,
        }
    }
}

/// A state, a participant, a transition, a message, or a lens-level element (`states_from`,
/// `entry`, `file`).
#[derive(Debug, Clone, PartialEq)]
pub struct Element {
    /// The checker's name for it.
    pub name: String,
    /// What it is: "State", "Transition", "Message 3"…
    pub what: String,
    pub label: String,
    /// An edge's ends, by index in `Diagram.nodes`.
    pub ends: Option<(usize, usize)>,
    /// A transition's guard or a message's note.
    pub detail: Option<String>,
    pub anchor: Option<LensAnchor>,
    /// Where the checker found it: path and one-based line at the checked commit.
    pub location: Option<(String, u64)>,
    pub marks: Vec<Mark>,
}

impl Element {
    pub fn marked(&self) -> Option<Mark> {
        self.marks.iter().min().copied()
    }
}

/// A check that failed or could not run, on the element it names.
#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    pub element: String,
    /// The element in the owner's words.
    pub what: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Diagram {
    pub kind: LensKind,
    pub id: String,
    pub title: String,
    pub file: String,
    pub status: LensStatus,
    pub read_at: String,
    /// The commit it was checked at.
    pub commit: Option<String>,
    /// States or participants, in file order.
    pub nodes: Vec<Element>,
    /// Transitions or messages, in file order (a sequence's order).
    pub edges: Vec<Element>,
    /// `states_from` and `entry`: checked, not drawn.
    pub about: Vec<Element>,
    pub failures: Vec<Issue>,
    pub not_checked: Vec<Issue>,
    /// Variants of the `states_from` enum the lens has no state for.
    pub missing: Vec<String>,
    /// States the enum has no variant for.
    pub extra: Vec<String>,
    /// Committed source around located elements, as corrald served it.
    pub excerpts: Vec<LensExcerpt>,
}

/// The marks a review puts on a lens's elements, by the checker's names.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Touch {
    pub touched: Vec<String>,
    pub added: Vec<String>,
}

fn node_name(kind: LensKind, id: &str) -> String {
    match kind {
        LensKind::State => format!("state {id}"),
        LensKind::Sequence => format!("participant {id}"),
    }
}

/// The checker's names for edges: a repeat of the same ends and label is numbered from its
/// second occurrence (`corral-facts` `lenses::elements`).
fn edge_names<'a>(
    word: &str,
    edges: impl Iterator<Item = (&'a str, &'a str, &'a str)>,
) -> Vec<String> {
    let mut seen: Vec<((&str, &str, &str), usize)> = Vec::new();
    edges
        .map(|key| {
            let n = match seen.iter_mut().find(|(k, _)| *k == key) {
                Some((_, n)) => {
                    *n += 1;
                    *n
                }
                None => {
                    seen.push((key, 1));
                    1
                }
            };
            let (from, to, label) = key;
            let mut name = format!("{word} {from:?} -> {to:?}: {label:?}");
            if n > 1 {
                name.push_str(&format!(" (#{n})"));
            }
            name
        })
        .collect()
}

/// A served, readable lens as a diagram, with `touch`'s marks; none for a malformed file.
pub fn diagram(
    result: &LensResult,
    commit: Option<&str>,
    excerpts: &[LensExcerpt],
    touch: &Touch,
) -> Option<Diagram> {
    let lens = result.lens.as_ref()?;
    let kind = lens.kind();
    let (nodes, raw_edges, node_word) = match lens {
        Lens::State {
            states,
            transitions,
            ..
        } => (
            states,
            transitions
                .iter()
                .map(|t| (&t.from, &t.to, &t.label, &t.guard, &t.anchor))
                .collect::<Vec<_>>(),
            "State",
        ),
        Lens::Sequence {
            participants,
            messages,
            ..
        } => (
            participants,
            messages
                .iter()
                .map(|m| (&m.from, &m.to, &m.label, &m.note, &m.anchor))
                .collect(),
            "Participant",
        ),
    };
    let located = |name: &str| {
        result
            .locations
            .iter()
            .find(|l| l.element == name)
            .map(|l| (l.path.clone(), l.line))
    };
    let failed = |name: &str| result.failures.iter().any(|f| f.element == name);
    let marks_of = |name: &str, extra: bool| {
        let mut marks = Vec::new();
        if failed(name) {
            marks.push(Mark::Failed);
        }
        if extra {
            marks.push(Mark::Extra);
        }
        if touch.touched.iter().any(|t| t == name) {
            marks.push(Mark::Touched);
        }
        if touch.added.iter().any(|t| t == name) {
            marks.push(Mark::Added);
        }
        marks
    };
    let nodes: Vec<Element> = nodes
        .iter()
        .map(|node| {
            let name = node_name(kind, &node.id);
            let extra = kind == LensKind::State && result.extra_states.contains(&node.id);
            Element {
                marks: marks_of(&name, extra),
                location: located(&name),
                what: node_word.into(),
                label: node.label.clone(),
                ends: None,
                detail: None,
                anchor: Some(node.anchor.clone()),
                name,
            }
        })
        .collect();
    let index = |id: &str| match lens {
        Lens::State { states, .. } => states.iter().position(|s| s.id == id),
        Lens::Sequence { participants, .. } => participants.iter().position(|p| p.id == id),
    };
    let word = match kind {
        LensKind::State => "transition",
        LensKind::Sequence => "message",
    };
    let names = edge_names(
        word,
        raw_edges
            .iter()
            .map(|(f, t, l, _, _)| (f.as_str(), t.as_str(), l.as_str())),
    );
    let edges: Vec<Element> = raw_edges
        .iter()
        .zip(names)
        .enumerate()
        .map(|(n, ((from, to, label, detail, anchor), name))| Element {
            marks: marks_of(&name, false),
            location: located(&name),
            what: match kind {
                LensKind::State => "Transition".into(),
                LensKind::Sequence => format!("Message {}", n + 1),
            },
            label: (*label).clone(),
            ends: index(from).zip(index(to)),
            detail: (*detail).clone(),
            anchor: Some((*anchor).clone()),
            name,
        })
        .collect();
    let mut about = Vec::new();
    match lens {
        Lens::State { states_from, .. } => about.push(Element {
            marks: marks_of("states_from", false),
            location: located("states_from"),
            what: "States from".into(),
            label: states_from.name.clone(),
            ends: None,
            detail: None,
            anchor: Some(LensAnchor {
                path: states_from.path.clone(),
                name: Some(states_from.name.clone()),
                text: None,
            }),
            name: "states_from".into(),
        }),
        Lens::Sequence {
            entry: Some(entry), ..
        } => about.push(Element {
            marks: marks_of("entry", false),
            location: located("entry"),
            what: "Entry point".into(),
            label: format!("{} {}", entry_word(entry.kind), entry.name),
            ends: None,
            detail: None,
            anchor: None,
            name: "entry".into(),
        }),
        Lens::Sequence { entry: None, .. } => {}
    }
    let mut diagram = Diagram {
        kind,
        id: lens.id().to_owned(),
        title: lens.title().to_owned(),
        file: result.file.clone(),
        status: result.status,
        read_at: lens.read_at().to_owned(),
        commit: commit.map(str::to_owned),
        nodes,
        edges,
        about,
        failures: Vec::new(),
        not_checked: Vec::new(),
        missing: result.missing_states.clone(),
        extra: result.extra_states.clone(),
        excerpts: excerpts.to_vec(),
    };
    diagram.failures = issues(&diagram, &result.failures);
    diagram.not_checked = issues(&diagram, &result.not_checked);
    Some(diagram)
}

fn entry_word(kind: corral_types::EntryKind) -> &'static str {
    match kind {
        corral_types::EntryKind::Command => "command",
        corral_types::EntryKind::Route => "route",
        corral_types::EntryKind::Operation => "operation",
    }
}

fn issues(diagram: &Diagram, list: &[LensDiagnostic]) -> Vec<Issue> {
    list.iter()
        .map(|d| Issue {
            what: diagram
                .element(&d.element)
                .map(|e| format!("{} {}", e.what, e.label))
                .unwrap_or_else(|| d.element.clone()),
            element: d.element.clone(),
            message: d.message.clone(),
        })
        .collect()
}

impl Diagram {
    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.nodes.iter().chain(&self.edges).chain(&self.about)
    }

    pub fn element(&self, name: &str) -> Option<&Element> {
        self.elements().find(|e| e.name == name)
    }

    pub fn node_label(&self, index: usize) -> &str {
        self.nodes.get(index).map_or("?", |n| n.label.as_str())
    }

    /// "State lens · 8 states · 9 transitions".
    pub fn figures(&self) -> String {
        let (nodes, edges) = match self.kind {
            LensKind::State => (("state", "states"), ("transition", "transitions")),
            LensKind::Sequence => (("participant", "participants"), ("message", "messages")),
        };
        format!(
            "{} lens · {} · {}",
            kind_word(Some(self.kind)),
            super::plural(self.nodes.len() as u64, nodes.0, nodes.1),
            super::plural(self.edges.len() as u64, edges.0, edges.1),
        )
    }

    /// "Read at 3aafaafe81 · checked at 7005ad0dc1".
    pub fn provenance(&self) -> String {
        let mut out = format!("Read at {}", short(&self.read_at));
        if let Some(commit) = &self.commit {
            out.push_str(&format!(" · checked at {}", short(commit)));
        }
        out.push_str(&format!(" · {}", self.file));
        out
    }

    /// What the status means for this lens.
    pub fn verdict(&self) -> String {
        match self.status {
            LensStatus::Current if self.not_checked.is_empty() => {
                "Current: every check passed.".into()
            }
            LensStatus::Current => format!(
                "Current: every check that could run passed; {} not checked.",
                self.not_checked.len()
            ),
            LensStatus::Stale => format!(
                "Stale: {} failed. Failing elements are marked ✗.",
                super::plural(self.failures.len() as u64, "check", "checks")
            ),
            LensStatus::Incomplete => {
                "Incomplete: the states do not match the code's variants.".into()
            }
            LensStatus::Malformed => "Malformed: the file could not be read as a lens.".into(),
        }
    }

    /// An incomplete state lens's missing and extra states, named.
    pub fn mismatch(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.missing.is_empty() {
            out.push(format!("Missing states: {}.", self.missing.join(", ")));
        }
        if !self.extra.is_empty() {
            out.push(format!("Extra states: {}.", self.extra.join(", ")));
        }
        out
    }
}

pub fn short(commit: &str) -> &str {
    commit.get(..10).unwrap_or(commit)
}

/// A row of the list form the phone and a terminal draw instead of the diagram.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub element: String,
    /// A message's number.
    pub lead: Option<String>,
    pub main: String,
    pub sub: Option<String>,
    pub mark: Option<Mark>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    pub title: &'static str,
    pub lines: Vec<Line>,
}

/// States then transitions, or participants then numbered messages: execution order is what a
/// sequence means, and a list keeps it on any width.
pub fn listing(diagram: &Diagram) -> Vec<Listing> {
    let (nodes, edges) = match diagram.kind {
        LensKind::State => ("States", "Transitions"),
        LensKind::Sequence => ("Participants", "Messages"),
    };
    let node_lines = diagram
        .nodes
        .iter()
        .map(|e| Line {
            element: e.name.clone(),
            lead: None,
            main: e.label.clone(),
            sub: None,
            mark: e.marked(),
        })
        .collect();
    let edge_lines = diagram
        .edges
        .iter()
        .enumerate()
        .map(|(n, e)| {
            let ends = e.ends.map_or_else(
                || "?".to_owned(),
                |(a, b)| format!("{} → {}", diagram.node_label(a), diagram.node_label(b)),
            );
            let sub = match (diagram.kind, &e.detail) {
                (LensKind::State, Some(guard)) => Some(format!("[{guard}]")),
                (LensKind::Sequence, Some(note)) => Some(note.clone()),
                (_, None) => None,
            };
            Line {
                element: e.name.clone(),
                lead: (diagram.kind == LensKind::Sequence).then(|| format!("{}.", n + 1)),
                main: format!("{ends}: {}", e.label),
                sub,
                mark: e.marked(),
            }
        })
        .collect();
    vec![
        Listing {
            title: nodes,
            lines: node_lines,
        },
        Listing {
            title: edges,
            lines: edge_lines,
        },
    ]
}

/// What the inspector says about one element.
#[derive(Debug, Clone, PartialEq)]
pub struct Inspect {
    pub what: String,
    pub title: String,
    pub rows: Vec<(&'static str, String)>,
    /// Its checks, each with how it stands.
    pub checks: Vec<(Hue, String)>,
    /// Its committed source line.
    pub line: Option<(String, u64)>,
    /// The committed lines around it; none when corrald could not read them.
    pub source: Option<Source>,
}

/// A committed excerpt as the inspector shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    /// `path:line @ commit`.
    pub heading: String,
    /// Line number, text, and whether it is the anchored line.
    pub lines: Vec<(u64, String, bool)>,
}

pub const SOURCE_UNAVAILABLE: &str =
    "Source unavailable: corrald served no committed lines for this element.";

fn source(diagram: &Diagram, name: &str) -> Option<Source> {
    let excerpt = diagram.excerpts.iter().find(|e| e.element == name)?;
    let at = diagram
        .commit
        .as_deref()
        .map(|c| format!(" @ {}", short(c)))
        .unwrap_or_default();
    Some(Source {
        heading: format!("{}:{}{at}", excerpt.path, excerpt.line),
        lines: excerpt
            .lines
            .iter()
            .enumerate()
            .map(|(n, text)| {
                let number = excerpt.first + n as u64;
                (number, text.clone(), number == excerpt.line)
            })
            .collect(),
    })
}

pub fn inspect(diagram: &Diagram, name: &str) -> Option<Inspect> {
    let element = diagram.element(name)?;
    let mut rows = Vec::new();
    if let Some((a, b)) = element.ends {
        rows.push(("From", diagram.node_label(a).to_owned()));
        rows.push(("To", diagram.node_label(b).to_owned()));
    }
    if let Some(detail) = &element.detail {
        rows.push((
            match diagram.kind {
                LensKind::State => "Guard",
                LensKind::Sequence => "Note",
            },
            detail.clone(),
        ));
    }
    if let Some(anchor) = &element.anchor {
        rows.push(("Anchor", anchor.path.clone()));
        if let Some(name) = &anchor.name {
            rows.push(("Definition", name.clone()));
        }
        if let Some(text) = &anchor.text {
            rows.push(("Text", text.replace('\n', "⏎ ")));
        }
    }
    match (&element.location, &diagram.commit) {
        (Some((path, line)), Some(commit)) => {
            rows.push(("Line", format!("{path}:{line} @ {}", short(commit))))
        }
        (Some((path, line)), None) => rows.push(("Line", format!("{path}:{line}"))),
        (None, _) => rows.push(("Line", "not located at this commit".into())),
    }
    let mut checks: Vec<(Hue, String)> = Vec::new();
    for issue in diagram.failures.iter().filter(|i| i.element == name) {
        checks.push((Hue::Fail, format!("Failed: {}", issue.message)));
    }
    if element.marks.contains(&Mark::Extra) {
        checks.push((
            Hue::Held,
            "Not a variant of the states' enum in the code.".into(),
        ));
    }
    for issue in diagram.not_checked.iter().filter(|i| i.element == name) {
        checks.push((Hue::Neutral, format!("Not checked: {}", issue.message)));
    }
    if checks.is_empty() {
        checks.push((Hue::Ok, "Every check that ran on it passed.".into()));
    }
    if element.marks.contains(&Mark::Touched) {
        checks.push((
            Hue::Worker,
            "The change under review edits its anchor.".into(),
        ));
    }
    if element.marks.contains(&Mark::Added) {
        checks.push((Hue::Ok, "The change under review adds it.".into()));
    }
    Some(Inspect {
        what: element.what.clone(),
        title: element.label.clone(),
        rows,
        checks,
        line: element.location.clone(),
        source: source(diagram, name),
    })
}

pub const LEAD: &str = "A lens is a saved state or sequence diagram, anchored in the code and checked on every commit.";
pub const NONE: &str =
    "No lenses in this project yet. Draft one: an agent writes it once, Corral checks it after.";
pub const HINT: &str =
    "Select a state, a transition, a participant or a message for its anchor and its checks.";
pub const DRAFT_LEAD: &str =
    "Files a task for an agent to write the lens. Say in one sentence what it should show.";
pub const DRAFT_PLACEHOLDER: &str = "e.g. how a run moves from dispatch to review";
pub const REPAIR_LEAD: &str = "Files a task for an agent to fix the failing checks.";

/// Where the list's read stands, then what corrald says about it.
pub fn notices(list: Option<&SystemLenses>, row: Option<&SystemRow>) -> Vec<Notice> {
    let mut out = Vec::new();
    if let Some(list) = list {
        if list.status == SystemStatus::Building {
            out.push(Notice {
                hue: Hue::Neutral,
                words: "Corral is checking the lenses at the new head.".into(),
            });
        }
        if let Some(error) = &list.error {
            out.push(Notice {
                hue: Hue::Fail,
                words: format!("The lenses could not be read: {error}"),
            });
        }
        if list.omitted_rows > 0 {
            out.push(Notice {
                hue: Hue::Held,
                words: format!(
                    "{} left out to fit the reply.",
                    super::plural(list.omitted_rows as u64, "lens", "lenses")
                ),
            });
        }
    }
    if let Some(error) = row.and_then(|row| row.error.as_ref()) {
        out.push(Notice {
            hue: Hue::Fail,
            words: format!("The project's facts failed: {error}"),
        });
    }
    out
}

/// The draft form's kinds.
pub const KINDS: [(LensKind, &str); 2] =
    [(LensKind::State, "State"), (LensKind::Sequence, "Sequence")];

#[cfg(test)]
#[path = "lens_tests.rs"]
mod tests;
