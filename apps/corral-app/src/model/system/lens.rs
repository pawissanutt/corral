//! Insight › Lenses: generic repository diagrams checked against an immutable revision. The view reads `system_lenses`
//! for the list and `system_lens` for the one drawn; a task's Impact reads `system_lens` at its
//! head for a lens the change touches. A served lens becomes a [`Diagram`]: its elements in
//! file order, each with the checker's name for it (so a failure, a location or a touch lands
//! on the element it names), its anchor, its committed line and its marks.

use std::collections::BTreeMap;

use corral_protocol::detail::{DetailBody, DetailRef};
use corral_types::lenses::{
    EvidenceAnchor, LensDiagnostic, LensExcerpt, LensLocation, LensResult, LensStatus,
    RecipeEvidence, RecipeLayout,
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

pub const fn kind_word(kind: Option<RecipeLayout>) -> &'static str {
    match kind {
        Some(RecipeLayout::Graph) => "Graph",
        Some(RecipeLayout::OrderedLanes) => "Ordered lanes",
        None => "Unreadable",
    }
}

/// One line of the list.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// What `system_lens` is asked for: the lens id, or the file of one that has none.
    pub key: String,
    pub title: String,
    pub kind: Option<RecipeLayout>,
    pub status: LensStatus,
    pub file: String,
}

impl Row {
    /// Its kind; a readable lens an older corrald served without one is just a recipe.
    pub const fn kind_word(&self) -> &'static str {
        match (self.kind, self.status) {
            (None, LensStatus::Malformed) | (Some(_), _) => kind_word(self.kind),
            (None, _) => "Lens",
        }
    }
}

pub fn rows(list: &SystemLenses) -> Vec<Row> {
    list.rows
        .iter()
        .map(|row| Row {
            // The file names one lens exactly; an ID could equal another lens's path.
            key: row.file.clone(),
            title: row.title.clone().unwrap_or_else(|| row.file.clone()),
            kind: row.layout,
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
    /// The change under review removes it (drawn on the lens as it was at the base).
    Removed,
}

impl Mark {
    pub const fn glyph(self) -> &'static str {
        match self {
            Mark::Failed => "✗",
            Mark::Extra => "?",
            Mark::Touched => "Δ",
            Mark::Added => "+",
            Mark::Removed => "−",
        }
    }
    pub const fn word(self) -> &'static str {
        match self {
            Mark::Failed => "failing",
            Mark::Extra => "not in the code",
            Mark::Touched => "touched",
            Mark::Added => "added",
            Mark::Removed => "removed",
        }
    }
    pub const fn hue(self) -> Hue {
        match self {
            Mark::Failed => Hue::Fail,
            Mark::Extra => Hue::Held,
            Mark::Touched => Hue::Worker,
            Mark::Added => Hue::Ok,
            Mark::Removed => Hue::Fail,
        }
    }
}

/// A recipe node, edge or group. Its ID is independent of labels and array position.
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
    pub authored: Vec<(&'static str, String)>,
    pub group: Option<String>,
    pub marks: Vec<Mark>,
}

impl Element {
    /// The strongest mark, which colours it.
    pub fn marked(&self) -> Option<Mark> {
        self.marks.iter().min().copied()
    }

    /// Every mark's glyph, so a failing element the change touches shows both.
    pub fn glyphs(&self) -> Option<String> {
        (!self.marks.is_empty()).then(|| self.marks.iter().map(|m| m.glyph()).collect())
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
    pub kind: RecipeLayout,
    pub id: String,
    pub title: String,
    pub file: String,
    pub status: LensStatus,
    pub evidence: BTreeMap<RecipeEvidence, Option<LensLocation>>,
    pub derived: Vec<corral_types::lenses::InboundReference>,
    /// The commit it was checked at.
    pub commit: Option<String>,
    /// States or participants, in file order.
    pub nodes: Vec<Element>,
    /// Transitions or messages, in file order (a sequence's order).
    pub edges: Vec<Element>,
    /// Selectable groups.
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
    pub removed: Vec<String>,
}

/// Project the checked recipe without manufacturing identities from display labels.
pub fn diagram(
    result: &LensResult,
    commit: Option<&str>,
    excerpts: &[LensExcerpt],
    touch: &Touch,
) -> Option<Diagram> {
    let recipe = result.lens.as_ref()?;
    let kind = recipe.layout;
    let element = |id: &str, what: &str, label: &str| {
        let mut marks = Vec::new();
        if result.failures.iter().any(|f| f.element == id) {
            marks.push(Mark::Failed);
        }
        if result.extra_states.iter().any(|n| n == id) {
            marks.push(Mark::Extra);
        }
        for (ids, mark) in [
            (&touch.touched, Mark::Touched),
            (&touch.added, Mark::Added),
            (&touch.removed, Mark::Removed),
        ] {
            if ids.iter().any(|n| n == id) {
                marks.push(mark);
            }
        }
        let mut authored = vec![("ID", id.to_string())];
        for binding in recipe.evidence.iter().filter(|e| e.element == id) {
            authored.push(match &binding.anchor {
                EvidenceAnchor::Path { path } => ("Path", path.clone()),
                EvidenceAnchor::Symbol { path, name } => ("Symbol", format!("{name} in {path}")),
                EvidenceAnchor::Text { path, text } => ("Text", format!("{path}: {text}")),
                EvidenceAnchor::Entry { entry_kind, name } => {
                    ("Entry", format!("{} {name}", entry_word(*entry_kind)))
                }
            });
        }
        Element {
            name: id.into(),
            what: what.into(),
            label: label.into(),
            ends: None,
            detail: None,
            group: None,
            authored,
            marks,
        }
    };
    let about = recipe
        .groups
        .iter()
        .map(|g| element(&g.id, "Group", &g.label))
        .collect();
    let nodes = recipe
        .nodes
        .iter()
        .map(|n| {
            let mut e = element(&n.id, "Node", &n.label);
            e.group = n.group.clone();
            if let Some(group) = &e.group {
                e.authored.push(("Group", group.clone()));
            }
            e
        })
        .collect();
    let index = |id: &str| recipe.nodes.iter().position(|n| n.id == id);
    let edges = recipe
        .edges
        .iter()
        .map(|edge| {
            let mut e = element(&edge.id, "Edge", &edge.label);
            e.ends = index(&edge.from).zip(index(&edge.to));
            e.detail = edge.note.clone();
            for (name, value) in [
                ("Source label", &edge.from_label),
                ("Target label", &edge.to_label),
            ] {
                if let Some(value) = value {
                    e.authored.push((name, value.clone()));
                }
            }
            e
        })
        .collect();
    let mut diagram = Diagram {
        kind,
        id: recipe.id.clone(),
        title: recipe.title.clone(),
        file: result.file.clone(),
        status: result.status,
        evidence: {
            let mut evidence: BTreeMap<_, _> =
                recipe.evidence.iter().cloned().map(|e| (e, None)).collect();
            for loc in &result.locations {
                if let Some(span) = &loc.span
                    && let Some(binding) = recipe.evidence.get(span.binding)
                    && binding.element == loc.element
                    && let Some(value) = evidence.get_mut(binding)
                {
                    *value = Some(loc.clone());
                }
            }
            evidence
        },
        derived: result.derived.clone(),
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
        format!(
            "{} · {} nodes · {} edges",
            kind_word(Some(self.kind)),
            self.nodes.len(),
            self.edges.len()
        )
    }

    pub fn provenance(&self) -> String {
        let mut out = format!(
            "Checked at {}",
            self.commit
                .as_deref()
                .map(short)
                .unwrap_or("unknown revision")
        );
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
            LensStatus::Malformed => "Malformed: the file could not be read as a recipe.".into(),
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
    /// The strongest mark, and every mark's glyph.
    pub mark: Option<Mark>,
    pub glyphs: Option<String>,
    /// Every mark's word, for a screen reader: ", failing, touched".
    pub words: String,
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
        RecipeLayout::Graph => ("Nodes", "Edges"),
        RecipeLayout::OrderedLanes => ("Lanes", "Ordered edges"),
    };
    let lines = |elements: &[Element]| {
        elements
            .iter()
            .map(|e| Line {
                element: e.name.clone(),
                lead: None,
                main: e.label.clone(),
                sub: None,
                mark: e.marked(),
                glyphs: e.glyphs(),
                words: mark_words(e),
            })
            .collect()
    };
    let edge_lines = diagram
        .edges
        .iter()
        .enumerate()
        .map(|(n, e)| {
            let ends = e.ends.map_or_else(
                || "?".to_owned(),
                |(a, b)| {
                    let end = |n, label| {
                        let name = diagram.node_label(n);
                        e.authored
                            .iter()
                            .find(|(k, _)| *k == label)
                            .map_or_else(|| name.to_owned(), |(_, v)| format!("{name} ({v})"))
                    };
                    format!("{} → {}", end(a, "Source label"), end(b, "Target label"))
                },
            );
            let sub = match (diagram.kind, &e.detail) {
                (RecipeLayout::Graph, Some(guard)) => Some(guard.clone()),
                (RecipeLayout::OrderedLanes, Some(note)) => Some(note.clone()),
                (_, None) => None,
            };
            Line {
                element: e.name.clone(),
                lead: (diagram.kind == RecipeLayout::OrderedLanes).then(|| format!("{}.", n + 1)),
                main: format!("{ends}: {}", e.label),
                sub,
                mark: e.marked(),
                glyphs: e.glyphs(),
                words: mark_words(e),
            }
        })
        .collect();
    let mut sections = Vec::new();
    if !diagram.about.is_empty() {
        sections.push(Listing {
            title: "Groups",
            lines: lines(&diagram.about),
        });
    }
    sections.extend([
        Listing {
            title: nodes,
            lines: lines(&diagram.nodes),
        },
        Listing {
            title: edges,
            lines: edge_lines,
        },
    ]);
    sections
}

fn mark_words(e: &Element) -> String {
    e.marks.iter().map(|m| format!(", {}", m.word())).collect()
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
    pub sources: Vec<Source>,
    pub derived: Vec<String>,
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

/// Committed lines from `first`, the anchored `line` marked, headed by where and at which
/// commit they were read.
pub fn excerpt_source(
    path: &str,
    line: u64,
    first: u64,
    lines: &[String],
    commit: Option<&str>,
) -> Source {
    let at = commit
        .map(|c| format!(" @ {}", short(c)))
        .unwrap_or_default();
    Source {
        heading: format!("{path}:{line}{at}"),
        lines: lines
            .iter()
            .enumerate()
            .map(|(n, text)| {
                let number = first + n as u64;
                (number, text.clone(), number == line)
            })
            .collect(),
    }
}

pub fn inspect(diagram: &Diagram, name: &str) -> Option<Inspect> {
    let element = diagram.element(name)?;
    let mut rows = Vec::new();
    if let Some((a, b)) = element.ends {
        rows.push(("From", diagram.node_label(a).to_owned()));
        rows.push(("To", diagram.node_label(b).to_owned()));
    }
    if let Some(detail) = &element.detail {
        rows.push(("Note", detail.clone()));
    }
    rows.extend(element.authored.clone());
    for (key, loc) in &diagram.evidence {
        if key.element != name {
            continue;
        }
        let source = loc.as_ref().and_then(|loc| {
            let span = loc.span.as_ref()?;
            Some(format!(
                "{}:{}–{} · {}",
                loc.path,
                loc.line,
                span.end_line,
                span.digest.as_deref().unwrap_or("content not checked")
            ))
        });
        rows.push((
            "Bound span",
            source.unwrap_or_else(|| "No resolved span available for comparison".into()),
        ));
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
            "Binding, resolved path, or bound source differs between the revisions.".into(),
        ));
    }
    if element.marks.contains(&Mark::Added) {
        checks.push((Hue::Ok, "Added in this revision.".into()));
    }
    if element.marks.contains(&Mark::Removed) {
        checks.push((Hue::Fail, "Removed since this revision.".into()));
    }
    Some(Inspect {
        what: element.what.clone(),
        title: element.label.clone(),
        rows,
        checks,

        sources: diagram
            .excerpts
            .iter()
            .filter(|e| e.element == name)
            .map(|e| {
                excerpt_source(
                    &e.path,
                    e.line,
                    e.first,
                    &e.lines,
                    diagram.commit.as_deref(),
                )
            })
            .collect(),
        derived: diagram
            .derived
            .iter()
            .filter(|d| d.element == name)
            .map(|d| {
                let r = &d.reference;
                format!(
                    "{} → {} · {:?} ×{} · {:?} (file reference)",
                    r.from, r.to, r.kind, r.count, r.basis
                )
            })
            .collect(),
    })
}

pub const LEAD: &str =
    "Repository diagrams bind authored nodes and edges to evidence at a Git commit.";
pub const NONE: &str =
    "No diagrams in this project yet. Draft one: an agent writes it once, Corral checks it after.";
pub const HINT: &str = "Select an element for its authored bindings and committed evidence.";
pub const DRAFT_LEAD: &str =
    "Files a task for an agent to write the diagram. Say in one sentence what it should show.";
pub const DRAFT_PLACEHOLDER: &str = "e.g. how a run moves from dispatch to review";
pub const MISSING: &str = "No such diagram at this commit.";
/// Impact's heading over the lens as it was before the change.
pub const AT_BASE: &str = "At the base: what the change removed or edited there";
pub const AT_BASE_DELETED: &str = "The change deletes this diagram; as it was at the base";
pub const REPAIR_LEAD: &str = "Files a task for an agent to fix the failing checks.";

/// Where the list's read stands, then what corrald says about it.
pub fn notices(list: Option<&SystemLenses>, row: Option<&SystemRow>) -> Vec<Notice> {
    let mut out = Vec::new();
    if let Some(list) = list {
        if list.status == SystemStatus::Building {
            out.push(Notice {
                hue: Hue::Neutral,
                words: "Corral is checking the diagrams at the new head.".into(),
            });
        }
        if let Some(error) = &list.error {
            out.push(Notice {
                hue: Hue::Fail,
                words: format!("The diagrams could not be read: {error}"),
            });
        }
        if list.omitted_rows > 0 {
            out.push(Notice {
                hue: Hue::Held,
                words: format!(
                    "{} left out to fit the reply.",
                    super::plural(list.omitted_rows as u64, "diagram", "diagrams")
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
pub const KINDS: [(RecipeLayout, &str); 2] = [
    (RecipeLayout::Graph, "Graph"),
    (RecipeLayout::OrderedLanes, "Ordered lanes"),
];

#[cfg(test)]
#[path = "lens_tests.rs"]
mod tests;
