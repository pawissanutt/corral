//! The artifact finder: every task's artifacts, read from corrald one served page at a time as
//! the list is walked, narrowed by a filter that keeps the keyboard. Words match an artifact's
//! title, kind and reference; `kind:`, `task:` and `project:` narrow by that field; every token
//! must match. The request carries the field tokens as corrald's narrowing and the words as its
//! filter, so corrald pages through matching rows only, newest first. The selected artifact's
//! content shows beside the results (under them where the page is narrow) and opens full size;
//! from either, its task opens.

pub mod browser;
mod content;
pub mod diagram;
#[cfg(test)]
mod diagram_tests;
pub mod download;
pub mod mermaid;
#[cfg(test)]
mod tests;

pub use content::{
    Body, Content, FULL_PARTIAL_LINES, Line, PREVIEW_BYTES, PREVIEW_LINES, content, content_of,
    is_html, target as content_target,
};
pub(crate) use content::{log_hue, plain_text, utf8_text};

use corral_protocol::detail::{ArtifactNarrowing, DetailBody, DetailGet, DetailRef};
use corral_types::{Artifact, ArtifactGroup, ArtifactKind, Reference};

use crate::model::daemon::{DetailCache, Feed};
use crate::model::work::{stamp_at, when_at};
use crate::model::{Destination, Effect, Form, Id, Live};

pub const TITLE: &str = "Artifacts";
pub const SUBTITLE: &str = "Every task's artifacts, newest first";
pub const PLACEHOLDER: &str = "Filter: words, kind:report, task:T-12, project:site";
/// What a screen reader calls the filter field.
pub const FIELD_LABEL: &str = "Filter artifacts";
pub const HINT: &str = "Words match title, kind and reference; kind:, task: and project: narrow by that field. Every token must match. Submissions, records and replaced artifacts show under All or a kind: token.";
/// The switch between the owner's artifacts and every artifact.
pub const SHOW: [(bool, &str); 2] = [(false, "Default"), (true, "All")];
pub const OPEN_FULL: &str = "Open full size";
pub const BACK: &str = "Back to results";
pub const LOAD_MORE: &str = "Load older artifacts";
pub const LOADING_MORE: &str = "Loading older artifacts…";
pub const LOADING: &str = "Loading artifacts from corrald…";
/// Said where an artifact's content will show once corrald serves it.
pub const LOADING_CONTENT: &str = "Loading its content from corrald…";
/// The task list's way into the finder, with the task's token filled in.
pub const FIND_IN_ALL: &str = "Find in all artifacts";
pub const RETRY: &str = "Retry";
/// What a screen reader calls the preview beside the results.
pub const PREVIEW_LABEL: &str = "Selected artifact";
/// The terminal key line's keys while the finder shows.
pub const KEYS: [(&str, &str); 3] = [("↑ ↓", "walk"), ("Enter", "full size"), ("Tab", "actions")];

/// Rows a page key moves the selection.
const PAGE_STEP: usize = 10;

/// The finder's own state; the results are corrald's, in the feed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    /// The filter as typed.
    pub query: String,
    /// The selected artifact's reference (`T-12:3`); the first result while none is.
    pub selected: Option<String>,
    /// The artifact shown full size in the results' place.
    pub full: Option<String>,
    /// Grows each time the filter should take the keyboard focus again.
    pub focus: u32,
    /// The filter has the keyboard focus: letters type into it, and the terminal's key line
    /// gives the filter's keys rather than the destinations'.
    pub typing: bool,
    /// Ask corrald for submissions, records and replaced artifacts too.
    pub all: bool,
    /// The cited section the full view shows of a Markdown artifact (`4-fact-model`).
    pub section: Option<String>,
    /// The selected element of a workflow recipe: the artifact's reference and the element's
    /// ID, accepted only while that artifact shows and its served recipe holds the ID.
    pub pick: Option<(String, String)>,
}

/// A move of the selection through the results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Walk {
    Up,
    Down,
    PageUp,
    PageDown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Query(String),
    Walk(Walk),
    /// A press on a result: it selects; on the selected one it opens full size.
    Press(String),
    Open(String),
    /// Enter in the filter: open the selected result full size.
    OpenSelected,
    /// Back from full size to the results.
    Close,
    OpenTask(Id),
    /// Open the finder narrowed to one task.
    Find(Id),
    /// Ask corrald for the page after the last served one.
    LoadMore,
    /// Show every artifact, or only the owner's.
    All(bool),
    /// Open a cited artifact full size, at its section when the citation names one.
    Cite {
        reference: String,
        section: Option<String>,
    },
    /// Select an element of the shown workflow recipe, from its graph or its list.
    Pick {
        reference: String,
        element: String,
    },
}

/// The filter, read into the fields it narrows and the words left over (all lower case; a task
/// as corrald spells it).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Query {
    pub words: Vec<String>,
    pub kinds: Vec<String>,
    pub tasks: Vec<String>,
    pub projects: Vec<String>,
}

/// `T-12` from `t-12`, `T-12` or `12`; anything else as typed. A known project's name is kept
/// as typed even where it reads as a task (`t-42`, `42` are valid project names), because
/// corrald matches an owner exactly and a project owning a library is an owner too.
fn task_id(value: &str, project: &dyn Fn(&str) -> bool) -> String {
    if project(value) {
        return value.to_owned();
    }
    let number = value
        .strip_prefix("t-")
        .or_else(|| value.strip_prefix("T-"))
        .unwrap_or(value);
    if !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()) {
        format!("T-{number}")
    } else {
        value.to_owned()
    }
}

/// The groups an owner reads, in the order they are listed: what a task refers to, what it
/// delivers and what proves it. Submissions and records show only when asked for.
pub const OWNER_GROUPS: [ArtifactGroup; 3] = [
    ArtifactGroup::Reference,
    ArtifactGroup::Deliverable,
    ArtifactGroup::Proof,
];

/// Whether the artifact shows without All, as corrald's list serves by default: an owner's
/// group and not replaced by a newer one.
pub fn shown_by_default(artifact: &Artifact) -> bool {
    OWNER_GROUPS.contains(&artifact.kind.group()) && artifact.replaced_by.is_none()
}

/// The artifacts that show by default, by owner group in [`OWNER_GROUPS`]' order, each group
/// keeping the order given; a group with none is left out.
pub fn by_group(artifacts: &[Artifact]) -> Vec<(ArtifactGroup, Vec<&Artifact>)> {
    OWNER_GROUPS
        .into_iter()
        .map(|group| {
            let rows = artifacts
                .iter()
                .filter(|row| row.kind.group() == group && shown_by_default(row))
                .collect::<Vec<_>>();
            (group, rows)
        })
        .filter(|(_, rows)| !rows.is_empty())
        .collect()
}

/// Whether a project owns the artifact (a library row such as `corral:3`) rather than a task.
pub fn library(artifact: &Artifact) -> bool {
    artifact.project.as_deref() == Some(artifact.task.as_str())
}

impl Query {
    /// A field token still being typed (`kind:`) narrows nothing yet; no name is known as a
    /// project.
    pub fn parse(text: &str) -> Query {
        Query::parse_with(text, &|_| false)
    }

    /// [`Query::parse`] where `project` says which `task:` values name a known project.
    pub fn parse_with(text: &str, project: &dyn Fn(&str) -> bool) -> Query {
        let mut query = Query::default();
        for typed in text.split_whitespace() {
            let token = typed.to_lowercase();
            let (field, value) = token.split_once(':').unwrap_or_default();
            let into = match field {
                "kind" => &mut query.kinds,
                "task" => &mut query.tasks,
                "project" => &mut query.projects,
                _ => {
                    query.words.push(token);
                    continue;
                }
            };
            if !value.is_empty() {
                into.push(if field == "task" {
                    // corrald matches an owner exactly, so a library's project keeps its case.
                    let (_, typed) = typed.split_once(':').unwrap_or_default();
                    task_id(typed, project)
                } else {
                    value.to_owned()
                });
            }
        }
        query
    }

    /// The list request: the words as corrald's filter and one value of each field as its
    /// narrowing (the longest prefix of a kind or project, the first task); a second value of
    /// the same field narrows the served rows here. `all` asks for the rows corrald hides by
    /// default (submissions, records, replaced rows); a kind narrowing reaches them anyway.
    pub fn request(&self, all: bool) -> DetailRef {
        let longest = |values: &[String]| values.iter().max_by_key(|value| value.len()).cloned();
        let narrowing = ArtifactNarrowing {
            all,
            kind: longest(&self.kinds),
            task: self.tasks.first().cloned(),
            project: longest(&self.projects),
        };
        DetailRef::Artifacts {
            filter: (!self.words.is_empty()).then(|| self.words.join(" ")),
            narrowing: (narrowing != ArtifactNarrowing::default()).then_some(narrowing),
        }
    }

    /// Whether the artifact, of a task in `project`, matches every token.
    pub fn matches(&self, artifact: &Artifact, project: Option<&str>) -> bool {
        let kind = artifact.kind.as_str();
        let title = artifact.title.to_lowercase();
        let reference = artifact.reference().to_lowercase();
        let project = project.map(str::to_lowercase);
        self.kinds
            .iter()
            .all(|wanted| kind.starts_with(wanted.as_str()))
            && self.tasks.iter().all(|wanted| artifact.task == *wanted)
            && self.projects.iter().all(|wanted| {
                project
                    .as_deref()
                    .is_some_and(|project| project.starts_with(wanted.as_str()))
            })
            && self.words.iter().all(|word| {
                title.contains(word.as_str())
                    || kind.contains(word.as_str())
                    || reference.contains(word.as_str())
            })
    }
}

/// The project of an artifact's task: as corrald's row states it, else from what the app holds
/// (the board, the task's detail, a Closed page).
pub fn project_of(feed: &Feed, artifact: &Artifact) -> Option<String> {
    if artifact.project.is_some() {
        return artifact.project.clone();
    }
    let task = artifact.task.as_str();
    if let Some(row) = feed.board.task(task) {
        return row.task().project.clone();
    }
    let detail = DetailRef::Task { id: task.into() };
    if let Some(DetailBody::Task(detail)) = feed.detail(&detail).and_then(DetailCache::body) {
        return detail.task.project.clone();
    }
    feed.details
        .values()
        .filter_map(|cache| match cache.body() {
            Some(DetailBody::Closed(page)) => page.rows.iter().find(|row| row.id == task),
            _ => None,
        })
        .find_map(|row| row.project.clone())
}

/// The task's title, where the app holds it.
fn task_title(feed: &Feed, task: &str) -> Option<String> {
    if let Some(row) = feed.board.task(task) {
        return Some(row.title().to_owned());
    }
    let detail = DetailRef::Task { id: task.into() };
    match feed.detail(&detail).and_then(DetailCache::body) {
        Some(DetailBody::Task(detail)) => Some(detail.task.title.clone()),
        _ => None,
    }
}

impl State {
    /// The filter, read with the projects corrald has named.
    pub fn query(&self, feed: &Feed) -> Query {
        Query::parse_with(&self.query, &|name| {
            feed.projects.keys().any(|id| id.as_str() == name)
        })
    }

    /// The list request the filter and the All switch make.
    pub fn request(&self, feed: &Feed) -> DetailRef {
        self.query(feed).request(self.all)
    }
}

/// The served rows in served order (corrald's, newest first across pages), and the next page's
/// token while nothing is loading or failed.
fn served<'a>(feed: &'a Feed, target: &DetailRef) -> (Vec<&'a Artifact>, Option<&'a str>) {
    let Some(cache) = feed.detail(target) else {
        return (Vec::new(), None);
    };
    let rows = cache
        .pages
        .iter()
        .filter_map(|page| match &page.body {
            Some(DetailBody::Artifacts(page)) => Some(page.rows.iter()),
            _ => None,
        })
        .flatten()
        .collect();
    let next = if cache.loading || cache.error.is_some() {
        None
    } else {
        cache.pages.last().and_then(|page| page.next.as_deref())
    };
    (rows, next)
}

/// The served rows the filter keeps, and the next page's token.
fn visible<'a>(state: &State, feed: &'a Feed) -> (Vec<&'a Artifact>, Option<&'a str>) {
    let query = state.query(feed);
    let (rows, next) = served(feed, &query.request(state.all));
    let rows = rows
        .into_iter()
        .filter(|row| query.matches(row, project_of(feed, row).as_deref()))
        .collect();
    (rows, next)
}

/// Where the selection is among `rows`: the selected reference, else the first row.
fn selected_at(state: &State, rows: &[&Artifact]) -> Option<usize> {
    let chosen = state
        .selected
        .as_deref()
        .and_then(|selected| rows.iter().position(|row| row.reference() == selected));
    chosen.or((!rows.is_empty()).then_some(0))
}

/// The request for the page after the last served one, while one exists and none is on its
/// way.
fn next_page(state: &State, feed: &Feed) -> Option<DetailGet> {
    let target = state.request(feed);
    let (_, next) = served(feed, &target);
    Some(DetailGet {
        target,
        etag: None,
        page: Some(next?.to_owned()),
    })
}

/// The next page to ask for as the list is walked: once the selection stands on the last
/// result shown, or nothing shown matches while corrald has more.
pub fn more(state: &State, feed: &Feed) -> Option<DetailGet> {
    let (rows, _) = visible(state, feed);
    let at_end = selected_at(state, &rows).is_none_or(|at| at + 1 >= rows.len());
    if at_end { next_page(state, feed) } else { None }
}

impl State {
    pub fn act(&mut self, action: Action, feed: &Feed) -> Vec<Effect> {
        if !matches!(action, Action::Cite { .. } | Action::Pick { .. }) {
            self.section = None;
        }
        match action {
            Action::Pick { reference, element } => {
                let shown = match &self.full {
                    Some(full) => Some(full.clone()),
                    None => {
                        let (rows, _) = visible(self, feed);
                        selected_at(self, &rows).map(|at| rows[at].reference())
                    }
                };
                if shown.as_deref() == Some(reference.as_str())
                    && diagram::holds(feed, &reference, &element)
                {
                    self.pick = Some((reference, element));
                }
            }
            Action::Query(text) => {
                self.query = text;
                self.selected = None;
            }
            Action::Walk(walk) => {
                let (rows, _) = visible(self, feed);
                if let Some(at) = selected_at(self, &rows) {
                    let last = rows.len() - 1;
                    let to = match walk {
                        Walk::Up => at.saturating_sub(1),
                        Walk::Down => (at + 1).min(last),
                        Walk::PageUp => at.saturating_sub(PAGE_STEP),
                        Walk::PageDown => (at + PAGE_STEP).min(last),
                    };
                    self.selected = Some(rows[to].reference());
                }
            }
            Action::Press(reference) => {
                let (rows, _) = visible(self, feed);
                let shown = selected_at(self, &rows).map(|at| rows[at].reference());
                if shown.as_deref() == Some(reference.as_str()) {
                    self.full = Some(reference.clone());
                }
                self.selected = Some(reference);
                self.focus = self.focus.wrapping_add(1);
            }
            Action::Open(reference) => {
                self.selected = Some(reference.clone());
                self.full = Some(reference);
            }
            Action::OpenSelected => {
                let (rows, _) = visible(self, feed);
                if let Some(at) = selected_at(self, &rows) {
                    let reference = rows[at].reference();
                    self.selected = Some(reference.clone());
                    self.full = Some(reference);
                }
            }
            Action::Close => {
                self.full = None;
                self.focus = self.focus.wrapping_add(1);
            }
            Action::OpenTask(task) => {
                self.full = None;
                return vec![Effect::OpenLive(Live::Task(task))];
            }
            Action::Find(task) => {
                self.query = format!("task:{task} ");
                self.selected = None;
                self.full = None;
                self.focus = self.focus.wrapping_add(1);
                return vec![Effect::Go(Destination::Artifacts)];
            }
            Action::All(all) => {
                self.all = all;
                self.selected = None;
            }
            Action::Cite { reference, section } => {
                if let Some(task) = Reference::parse(&reference).map(Reference::task) {
                    self.query = format!("task:{task} ");
                }
                self.selected = Some(reference.clone());
                self.full = Some(reference);
                self.section = section;
                self.focus = self.focus.wrapping_add(1);
                return vec![Effect::Go(Destination::Artifacts)];
            }
            Action::LoadMore => {
                return next_page(self, feed)
                    .map(Effect::Detail)
                    .into_iter()
                    .collect();
            }
        }
        Vec::new()
    }

    /// Escape: back from full size, else clear a typed filter; whether it did either.
    pub fn escape(&mut self) -> bool {
        if self.full.is_some() && self.pick.take().is_some() {
            true
        } else if self.full.take().is_some() {
            self.section = None;
            self.focus = self.focus.wrapping_add(1);
            true
        } else if !self.query.is_empty() {
            self.query.clear();
            self.selected = None;
            true
        } else {
            false
        }
    }
}

/// One result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub reference: String,
    pub task: Id,
    pub kind: String,
    pub title: String,
    pub project: Option<String>,
    /// The run that recorded it, when a run did (`R2`).
    pub run: Option<String>,
    /// The title as the row shows it: led by what replaced it, when a newer artifact did
    /// (`→ T-12:4 · draft.md`), so a cut title still names its successor.
    pub shown_title: String,
    /// When it was added: the clock today, else the day.
    pub at: String,
    /// What a screen reader hears for the row.
    pub label: String,
}

/// How the results stand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Loading,
    Failed(String),
    Ready,
}

/// What stands under the last result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum More {
    /// corrald has served every page.
    Done,
    /// More pages wait: walking down, or [`LOAD_MORE`], asks for them.
    Offer,
    /// The next page is on its way.
    Loading,
}

/// The results list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Results {
    pub rows: Vec<Row>,
    pub selected: Option<String>,
    pub status: Status,
    /// Said where no result is shown.
    pub empty: Option<String>,
    /// How many are shown and whether more exist.
    pub count: String,
    pub more: More,
}

/// The selected artifact: its facts, its content, and its task.
#[derive(Debug, Clone, PartialEq)]
pub struct Preview {
    pub reference: String,
    /// What corrald is asked for to show it: its reference, or a cited section's.
    pub read: String,
    pub title: String,
    pub task: Id,
    /// Labelled facts in display order: kind, size, task, run, project, added, hash.
    pub facts: Vec<(&'static str, String)>,
    /// The same facts on one line: kind, size, task (and run), project, when, the hash's start.
    pub summary: String,
    pub content: Content,
    /// The button that opens its task; none for a project's library artifact.
    pub open_task: Option<String>,
    /// What Download saves: the whole artifact's bytes, checked against its catalog row.
    pub save: Save,
    /// A standalone HTML page, which Open in browser opens and View source reads.
    pub html: bool,
    /// The whole artifact checked as a workflow recipe, while it may be one.
    pub diagram_read: Option<DetailRef>,
    /// The recipe corrald served `ready`, drawn in the content's place.
    pub diagram: Option<diagram::Graph>,
}

/// The whole artifact a download saves, and what its bytes must match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Save {
    pub reference: String,
    pub file_name: String,
    pub bytes: u64,
    pub sha256: String,
}

/// The finder as every form shows it: the results and the selected artifact's preview, or that
/// artifact full size in their place.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub results: Results,
    pub preview: Option<Preview>,
    pub full: Option<Preview>,
    /// The All switch is on.
    pub all: bool,
}

/// The task, and the run that recorded it where one did: `T-12 R2`.
fn source(artifact: &Artifact) -> String {
    match &artifact.run {
        Some(run) => format!("{} {run}", artifact.task),
        None => artifact.task.clone(),
    }
}

pub fn display_kind(kind: ArtifactKind) -> String {
    match kind {
        ArtifactKind::Legacy => "artifact",
        kind => kind.as_str(),
    }
    .to_owned()
}

fn replaced(artifact: &Artifact) -> Option<String> {
    artifact
        .replaced_by
        .as_ref()
        .map(|by| format!("replaced by {by}"))
}

fn row(feed: &Feed, artifact: &Artifact) -> Row {
    let kind = display_kind(artifact.kind);
    let project = project_of(feed, artifact);
    let at = stamp_at(artifact.at, feed);
    let replaced = replaced(artifact);
    let label = format!(
        "{kind} {}, {}{}{}, added {at}",
        artifact.title,
        source(artifact),
        project
            .as_deref()
            .map(|project| format!(" in {project}"))
            .unwrap_or_default(),
        replaced
            .as_deref()
            .map(|replaced| format!(", {replaced}"))
            .unwrap_or_default()
    );
    Row {
        reference: artifact.reference(),
        task: Id::from(artifact.task.as_str()),
        kind,
        title: artifact.title.clone(),
        project,
        run: artifact.run.clone(),
        shown_title: match &artifact.replaced_by {
            Some(by) => format!("→ {by} · {}", artifact.title),
            None => artifact.title.clone(),
        },
        at,
        label,
    }
}

fn preview(
    feed: &Feed,
    artifact: &Artifact,
    form: Form,
    full: bool,
    pick: Option<&(String, String)>,
) -> Preview {
    let task = Id::from(artifact.task.as_str());
    let hash = if full {
        artifact.sha256.clone()
    } else {
        format!("{}…", artifact.sha256.chars().take(12).collect::<String>())
    };
    let kind = display_kind(artifact.kind);
    let size = crate::model::session::transcript::size(artifact.bytes);
    let project = project_of(feed, artifact);
    let added = format!(
        "{} by {}",
        when_at(artifact.at, feed),
        crate::model::review::actor(artifact.by)
    );
    let library = library(artifact);
    let summary = [
        replaced(artifact),
        Some(kind.clone()),
        Some(size.clone()),
        (!library).then(|| source(artifact)),
        project.clone(),
        Some(added.clone()),
        Some(format!("sha256 {hash}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let mut facts = vec![("Kind", kind), ("Size", size)];
    if !library {
        facts.push((
            "Task",
            match task_title(feed, &artifact.task) {
                Some(title) => format!("{task} · {title}"),
                None => task.to_string(),
            },
        ));
    }
    if let Some(run) = &artifact.run {
        facts.push(("Run", run.clone()));
    }
    if let Some(project) = project {
        facts.push(("Project", project));
    }
    facts.push(("Added", added));
    facts.push(("SHA-256", hash));
    if let Some(by) = &artifact.replaced_by {
        facts.push(("Replaced by", by.clone()));
    }
    let mut content = content(feed, artifact, form, full);
    let (diagram_read, diagram) = diagram::of(feed, artifact, &mut content, pick);
    Preview {
        diagram_read,
        diagram,
        reference: artifact.reference(),
        read: artifact.reference(),
        title: artifact.title.clone(),
        open_task: (!library).then(|| format!("Open {task}")),
        task,
        facts,
        summary,
        content,
        html: is_html(feed, artifact),
        save: Save {
            reference: artifact.reference(),
            file_name: download::file_name(artifact),
            bytes: artifact.bytes,
            sha256: artifact.sha256.clone(),
        },
    }
}

/// The finder now.
pub fn page(state: &State, feed: &Feed, form: Form) -> Page {
    let target = state.request(feed);
    let (rows, next) = visible(state, feed);
    let cache = feed.detail(&target);
    let status = match cache {
        Some(cache) if cache.pages.is_empty() => match &cache.error {
            Some(error) => Status::Failed(format!(
                "corrald could not list artifacts: {}",
                error.message
            )),
            None => Status::Loading,
        },
        Some(_) => Status::Ready,
        None => Status::Loading,
    };
    let more = match (next, cache.is_some_and(|cache| cache.loading)) {
        (Some(_), _) => More::Offer,
        (None, true) if !rows.is_empty() => More::Loading,
        _ => More::Done,
    };
    let at = selected_at(state, &rows);
    let shown = at.map(|at| rows[at]);
    // A cited artifact need not be among the filter's served rows: its task's detail holds it.
    let full = state.full.as_deref().and_then(|full| {
        let artifact = match rows.iter().find(|row| row.reference() == full) {
            Some(artifact) => (*artifact).clone(),
            None => crate::model::cite::held(feed, full, Reference::parse(full)?.task())?,
        };
        let mut shown = preview(feed, &artifact, form, true, state.pick.as_ref());
        // corrald cuts a cited section by the engine's rules; the app shows what it serves.
        if let Some(section) = &state.section {
            // A section is Markdown, never a whole recipe.
            shown.diagram = None;
            shown.diagram_read = None;
            shown.read = format!("{full}#{section}");
            let cited = content_of(feed, &artifact, &shown.read, form, true);
            let note = format!(
                "Only the cited section “{section}” shows; open the artifact from the results for all of it."
            );
            shown.content = Content {
                cut: Some(match cited.cut {
                    Some(cut) => format!("{note} {cut}"),
                    None => note,
                }),
                body: cited.body,
            };
            shown.title = format!("{} § {section}", shown.title);
        }
        Some(shown)
    });
    let count = match (rows.len(), more) {
        (0, _) => String::new(),
        (1, More::Done) => "1 artifact".into(),
        (n, More::Done) => format!("{n} artifacts"),
        (n, _) => format!("{n} so far · older ones load as you walk down"),
    };
    let empty = (status == Status::Ready && rows.is_empty() && more == More::Done).then(|| {
        if state.query.trim().is_empty() && state.all {
            "No task has recorded an artifact yet.".to_owned()
        } else if state.query.trim().is_empty() {
            "No reference, report or proof yet; All shows submissions and records too.".to_owned()
        } else {
            format!("No artifact matches “{}”.", state.query.trim())
        }
    });
    Page {
        results: Results {
            rows: rows.iter().map(|artifact| row(feed, artifact)).collect(),
            selected: shown.map(Artifact::reference),
            status,
            empty,
            count,
            more,
        },
        preview: shown.map(|artifact| preview(feed, artifact, form, false, state.pick.as_ref())),
        full,
        all: state.all,
    }
}
