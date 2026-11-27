//! Insight › Git history: one project's commits on one ref, read-only, from the `git_history`
//! detail (`DetailRef::GitHistory`) a page of commits at a time. The first page pins the walk to
//! the ref's tip; Load older walks on from that same tip, and Refresh reads the first page again
//! (a moved ref starts a new walk). `graph` lays the commits in lanes; this module turns the
//! served pages into the page's rows, its states and the selected commit's inspector. Links come
//! only from what corrald served for the exact commit, never from subjects, authors or times.

pub mod graph;

use corral_protocol::detail::{
    DetailBody, DetailGet, DetailRef, GitCommit, GitCommitLink, GitHistoryPage, GitLinkRelation,
};

use crate::model::daemon::Feed;
use crate::model::projects::{Scope, branch_page, known_projects, suggest};
use crate::model::work as when;
use crate::model::{Hue, Id};
use graph::{Graph, GraphRow};

/// Pages a walk keeps at most; past them Load older stops and Refresh starts again.
pub const MAX_PAGES: usize = 20;
/// Lanes a desktop row's canvas draws; past them `Page.overflow` says what is not drawn.
pub const DRAWN_LANES: usize = 16;
/// Characters of a short SHA, and of the SHA in a row's test id.
const SHORT: usize = 7;
const ROW_ID: usize = 12;

pub const TITLE: &str = "Git history";
pub const ASK_HEADING: &str = "Pick a project";
pub const ASK_WORDS: &str =
    "Git history shows one project's commits. Choose a project, or pick one in the scope switcher.";
pub const HEAD_LABEL: &str = "HEAD";
pub const BRANCH_CAPTION: &str = "Ref";
pub const BRANCH_FIELD: &str = "Branch";
pub const BRANCH_PLACEHOLDER: &str = "Branch name, Enter reads";
pub const REFRESH: &str = "Refresh";
pub const LOAD_OLDER: &str = "Load older";
pub const LOADING_OLDER: &str = "Loading older commits…";
pub const LOADING: &str = "Reading the history from corrald…";
pub const EMPTY: &str = "No commits yet: this ref has no history.";
pub const HINT: &str =
    "Select a commit to see its parents, dates and what Corral recorded about it.";
pub const CAVEAT: &str = "A link records what Corral did with this commit, not who authored it.";
pub const PARENTS: &str = "Parents";
pub const LINKS: &str = "Corral's records";
pub const NO_PARENT: &str = "none: the history starts here";
pub const NOT_LOADED: &str = "further down the walk, not loaded yet";
pub const EMPTY_HEADING: &str = "No commits";
pub const UNAVAILABLE_HEADING: &str = "Git history is unavailable";
pub const STALE: &str = "Shown as last read; reading the history again failed:";
pub const TIP_CAPTION: &str = "Tip";

/// Why the history cannot be shown, in corrald's words (an older corrald does not know it).
pub fn unavailable(why: &str) -> String {
    format!("corrald could not read this ref: {why}. Refresh tries again.")
}

pub fn open_task(task: &str) -> String {
    format!("Open {task}")
}

pub fn open_session(session: &str) -> String {
    format!("Open session {session}")
}

/// The page's own state: the project it follows, the ref chosen in it and the commit selected.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    pub project: Option<String>,
    /// A local branch; none for the project checkout's HEAD.
    pub branch: Option<String>,
    /// What the branch field holds: it narrows the branches offered, and Enter reads it.
    pub typed: String,
    pub selected: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// The project shown; another one forgets the ref and the selection.
    Follow(String),
    /// Read another ref: HEAD (none) or a local branch. The selection goes with the old ref.
    Branch(Option<String>),
    /// The branch field's words changed.
    Type(String),
    /// Enter in the branch field: read the branch it names (HEAD when empty). corrald decides
    /// whether it exists; an unknown name shows as unavailable.
    Submit,
    /// Select a commit by its full SHA.
    Select(String),
}

impl State {
    pub fn act(&mut self, action: Action) {
        match action {
            Action::Follow(project) => {
                if self.project.as_deref() != Some(project.as_str()) {
                    *self = State {
                        project: Some(project),
                        ..State::default()
                    };
                }
            }
            Action::Branch(branch) => {
                if self.branch != branch {
                    self.branch = branch;
                    self.selected = None;
                }
            }
            Action::Type(words) => self.typed = words,
            Action::Submit => {
                let name = self.typed.trim();
                let branch = (!name.is_empty() && name != HEAD_LABEL).then(|| name.to_owned());
                self.act(Action::Branch(branch));
            }
            Action::Select(sha) => self.selected = Some(sha),
        }
    }

    /// The ref chosen for `project`: HEAD until one is chosen in it.
    fn branch_of(&self, project: &str) -> Option<String> {
        (self.project.as_deref() == Some(project))
            .then(|| self.branch.clone())
            .flatten()
    }
}

/// The detail a project's ref is read from.
pub fn target(project: &str, branch: Option<&str>) -> DetailRef {
    DetailRef::GitHistory {
        project: project.into(),
        branch: branch.map(Into::into),
    }
}

/// One ref the page offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchChoice {
    pub branch: Option<String>,
    pub label: String,
    pub id: String,
    pub chosen: bool,
}

/// Where the read stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Loading,
    Ready,
    /// The ref has no commits yet.
    Empty,
    /// corrald could not read it (an unknown project or branch, an older corrald): why.
    Unavailable(String),
    /// Commits are shown as last read; reading again failed.
    Stale(String),
}

/// What stands under the rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum More {
    None,
    Offer(DetailGet),
    Loading,
    /// The walk holds [`MAX_PAGES`] pages: say so rather than keep growing.
    Capped(String),
}

/// A mark on a row for one of Corral's records of the commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    pub words: String,
    pub hue: Hue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub sha: String,
    pub id: String,
    pub short: String,
    pub refs: Vec<String>,
    pub subject: String,
    pub author: String,
    pub date: String,
    pub marks: Vec<Mark>,
    pub graph: GraphRow,
    /// The row's lanes as glyphs, for a terminal and the phone.
    pub glyphs: String,
    pub label: String,
}

/// One of Corral's records of the selected commit, in words, with the pages it opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkLine {
    pub words: String,
    pub task: String,
    pub title: String,
    pub hue: Hue,
    pub session: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inspector {
    pub subject: String,
    pub facts: Vec<(&'static str, String)>,
    /// Each parent's full SHA and, when it is not loaded or there is none, why.
    pub parents: Vec<(String, Option<&'static str>)>,
    pub links: Vec<LinkLine>,
    pub caveat: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Page {
    /// None while All projects is in scope: the page asks for one.
    pub project: Option<String>,
    pub projects: Vec<Id>,
    pub target: Option<DetailRef>,
    pub branches: Vec<BranchChoice>,
    pub status: Option<Status>,
    /// The walk's tip and ref: what Load older keeps walking from.
    pub pinned: String,
    pub rows: Vec<Row>,
    pub lanes: usize,
    /// The line under the last row while lanes continue below it, and its words.
    pub continuing: Option<(String, String)>,
    /// The lanes that continue below the last row.
    pub below: Vec<usize>,
    /// The graph is wider than a canvas draws: what is left out, for the forms that draw one.
    pub overflow: Option<String>,
    pub more: Option<More>,
    pub inspector: Option<Inspector>,
}

/// The page for the project `scope` names, on the ref `state` chose there.
pub fn page(state: &State, feed: &Feed, scope: &Scope) -> Page {
    let Scope::Project(project) = scope.resolved(feed) else {
        return Page {
            projects: known_projects(feed),
            ..Page::default()
        };
    };
    let project = project.to_string();
    let branch = state.branch_of(&project);
    let target = target(&project, branch.as_deref());
    let branches = branch_choices(feed, &project, branch.as_deref(), &state.typed);
    let cache = feed.detail(&target);
    let body = cache.and_then(|cache| match cache.body() {
        Some(DetailBody::GitHistory(page)) => Some(page),
        _ => None,
    });
    let loading = cache.is_none_or(|cache| cache.loading);
    let error = cache.and_then(|cache| cache.error.as_ref().map(|e| e.message.clone()));
    let status = match (body, error) {
        (None, Some(why)) => Status::Unavailable(why),
        (None, None) => Status::Loading,
        (Some(_), Some(why)) => Status::Stale(why),
        (Some(page), None) if page.tip.is_none() => Status::Empty,
        (Some(_), None) => Status::Ready,
    };
    let mut out = Page {
        project: Some(project),
        target: Some(target.clone()),
        branches,
        status: Some(status),
        ..Page::default()
    };
    let Some(walk) = body else {
        return out;
    };
    let commits: Vec<(&str, &[String])> = walk
        .commits
        .iter()
        .map(|commit| (commit.sha.as_str(), commit.parents.as_slice()))
        .collect();
    let graph = graph::lanes(&commits);
    out.pinned = pinned(walk);
    out.lanes = graph.lanes;
    out.overflow = (graph.lanes > DRAWN_LANES).then(|| {
        format!(
            "+{} lanes not drawn: the graph shows its first {DRAWN_LANES} lanes, and every commit is still listed.",
            graph.lanes - DRAWN_LANES
        )
    });
    out.continuing = continuing(&graph);
    out.below = graph.continuing.iter().map(|open| open.lane).collect();
    out.rows = walk
        .commits
        .iter()
        .zip(&graph.rows)
        .map(|(commit, row)| commit_row(feed, commit, row, graph.lanes))
        .collect();
    let pages = cache.map_or(0, |cache| cache.pages.len());
    let next = cache
        .filter(|cache| !cache.loading && cache.error.is_none())
        .and_then(|cache| cache.pages.last())
        .and_then(|page| page.next.clone());
    out.more = Some(match next {
        _ if loading => More::Loading,
        Some(_) if pages >= MAX_PAGES => More::Capped(format!(
            "Showing the newest {} commits, as far as this device keeps one walk. Refresh starts again from the tip.",
            walk.commits.len()
        )),
        Some(token) => More::Offer(DetailGet {
            target,
            etag: None,
            page: Some(token),
        }),
        None => More::None,
    });
    out.inspector = state
        .selected
        .as_deref()
        .and_then(|sha| {
            walk.commits
                .iter()
                .zip(&graph.rows)
                .find(|(c, _)| c.sha == sha)
        })
        .map(|(commit, row)| inspect(feed, commit, row));
    out
}

/// HEAD, then the project's local branches newest first (as corrald lists them, else the
/// project row's) that the typed words narrow to, a listed name typed whole first, the chosen
/// one always among them. A branch past those listed is reached by typing its name.
fn branch_choices(
    feed: &Feed,
    project: &str,
    chosen: Option<&str>,
    typed: &str,
) -> Vec<BranchChoice> {
    let listed = branch_page(feed, project).branches;
    let listed = if listed.is_empty() {
        crate::model::projects::branches(feed, Some(project))
    } else {
        listed
    };
    let whole = listed.iter().find(|name| name.as_str() == typed.trim());
    let mut names: Vec<String> = Vec::new();
    for name in whole.cloned().into_iter().chain(suggest(&listed, typed)) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    if let Some(chosen) = chosen.filter(|chosen| !names.iter().any(|name| name == chosen)) {
        names.push(chosen.to_owned());
    }
    std::iter::once(BranchChoice {
        branch: None,
        label: HEAD_LABEL.into(),
        id: format!("seg-history-branch-{HEAD_LABEL}"),
        chosen: chosen.is_none(),
    })
    .chain(names.into_iter().map(|name| BranchChoice {
        id: format!("seg-history-branch-{name}"),
        chosen: chosen == Some(name.as_str()),
        label: name.clone(),
        branch: Some(name),
    }))
    .collect()
}

fn short(sha: &str) -> String {
    sha.chars().take(SHORT).collect()
}

fn pinned(walk: &GitHistoryPage) -> String {
    match &walk.tip {
        Some(tip) => format!(
            "{} at {} · older pages walk from this tip",
            walk.reference,
            short(tip)
        ),
        None => walk.reference.clone(),
    }
}

fn continuing(graph: &Graph) -> Option<(String, String)> {
    let n = graph.continuing.len();
    (n > 0).then(|| {
        let words = if n == 1 {
            "1 line continues below: its parent is further down the walk.".to_owned()
        } else {
            format!("{n} lines continue below: their parents are further down the walk.")
        };
        (graph::continuing_glyphs(graph), words)
    })
}

fn relation_word(relation: GitLinkRelation) -> &'static str {
    match relation {
        GitLinkRelation::Landed => "landed",
        GitLinkRelation::Candidate => "candidate",
        GitLinkRelation::Reviewed => "reviewed",
        GitLinkRelation::Other => "linked",
    }
}

/// Landed and candidate differ by word and colour: a landing is done, a candidate is waiting.
fn relation_hue(relation: GitLinkRelation) -> Hue {
    match relation {
        GitLinkRelation::Landed => Hue::Ok,
        GitLinkRelation::Candidate => Hue::Held,
        GitLinkRelation::Reviewed => Hue::Corral,
        GitLinkRelation::Other => Hue::Neutral,
    }
}

fn commit_row(feed: &Feed, commit: &GitCommit, row: &GraphRow, lanes: usize) -> Row {
    let marks: Vec<Mark> = commit
        .links
        .iter()
        .map(|link| Mark {
            words: format!("{} {}", relation_word(link.relation), link.task),
            hue: relation_hue(link.relation),
        })
        .collect();
    let date = when::stamp(&commit.committed, feed);
    let mut label = format!(
        "{} {}, {}, {}",
        short(&commit.sha),
        commit.subject,
        commit.author,
        date
    );
    for mark in &marks {
        label.push_str(", ");
        label.push_str(&mark.words);
    }
    Row {
        sha: commit.sha.clone(),
        id: format!(
            "commit-{}",
            commit.sha.chars().take(ROW_ID).collect::<String>()
        ),
        short: short(&commit.sha),
        refs: commit.refs.clone(),
        subject: commit.subject.clone(),
        author: commit.author.clone(),
        date,
        marks,
        graph: row.clone(),
        glyphs: graph::glyphs(row, lanes),
        label,
    }
}

/// What a link says Corral did with the commit.
pub fn link_words(link: &GitCommitLink) -> String {
    let (task, run) = (&link.task, &link.run);
    match link.relation {
        GitLinkRelation::Landed => {
            format!(
                "Landed: task {task}'s worker run {run} fast-forwarded its target to this commit"
            )
        }
        GitLinkRelation::Candidate => {
            format!(
                "Candidate: run {run} of task {task} submitted this commit for review (not landed)"
            )
        }
        GitLinkRelation::Reviewed => {
            format!(
                "Reviewed: reviewer run {run} of task {task} was reviewing this candidate when its verdict was recorded"
            )
        }
        GitLinkRelation::Other => format!("Linked: run {run} of task {task} names this commit"),
    }
}

fn inspect(feed: &Feed, commit: &GitCommit, row: &GraphRow) -> Inspector {
    let dated = |at: &str| {
        let local = when::when(at, feed);
        if local.is_empty() {
            at.to_owned()
        } else {
            format!("{local} · {at}")
        }
    };
    let mut facts = vec![
        ("Commit", commit.sha.clone()),
        ("Author", commit.author.clone()),
        ("Authored", dated(&commit.authored)),
        ("Committed", dated(&commit.committed)),
    ];
    if !commit.refs.is_empty() {
        facts.push(("Refs", commit.refs.join(", ")));
    }
    let parents = if row.parents.is_empty() {
        vec![(String::new(), Some(NO_PARENT))]
    } else {
        row.parents
            .iter()
            .map(|edge| (edge.sha.clone(), (!edge.loaded).then_some(NOT_LOADED)))
            .collect()
    };
    let links: Vec<LinkLine> = commit
        .links
        .iter()
        .map(|link| LinkLine {
            words: link_words(link),
            task: link.task.clone(),
            title: format!("{} {}", link.task, link.title)
                .trim_end()
                .to_owned(),
            hue: relation_hue(link.relation),
            session: link.session.clone(),
        })
        .collect();
    Inspector {
        subject: commit.subject.clone(),
        facts,
        parents,
        caveat: (!links.is_empty()).then_some(CAVEAT),
        links,
    }
}

#[cfg(test)]
mod tests;
