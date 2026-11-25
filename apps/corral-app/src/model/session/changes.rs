//! The Changes tab: what the session changed, read from its host (`SessionChanges`, the
//! session's own comparison first). The comparisons the host offers as a switch (one it cannot
//! serve stays listed with its reason and does not switch), the repositories it can read, the
//! author chips (each child agent that edited, and the parent), the path filter, the file list
//! with each file's status, rename, counts, authors and the flag on an edit outside the editing
//! agent's scope, the totals and the bound's notice; the opened file's diff is
//! `changes_diff.rs`. The client follows every page the host serves, so the list and the patch
//! arrive whole up to the host's bounds. The tab's badge is the host's file count, `git?` after
//! a failed read.

use corral_protocol::detail::{
    ChangedFile, ChangesComparison, ComparisonInfo, DetailBody, DetailRef, SessionChanges,
};

use crate::model::Id;
use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::hue::Hue;

use super::changes_diff::{self, FileDiff, Opened, Pinned};
use super::tabs::{PartBody, load, part};
use super::{Action, Cx, Focus, State, Tab};

#[path = "changes_keys.rs"]
mod catalog;

pub use catalog::{actions, keys, press};

pub(super) const CAPABILITY: &str = "changes";
/// The session composer's field, which a capture focuses.
const COMPOSER_FIELD: &str = "field-message";
/// The tab's path filter.
pub const FILTER_FIELD: &str = "field-changes-filter";

/// Whose edits the list shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Author {
    /// Files this child agent edited.
    Child(String),
    /// Files no child edited: the session's own agent's.
    Parent,
}

/// The Changes tab's state on this device, per session.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChangesUi {
    /// The comparison shown; none is the session's own.
    pub comparison: Option<ChangesComparison>,
    /// The repository read; none is the session's own worktree.
    pub scope: Option<String>,
    /// Whose edits show; none is everyone's.
    pub author: Option<Author>,
    /// Paths containing this show.
    pub filter: String,
    /// The opened file, as asked (a list row's path, or a transcript row's).
    pub file: Option<String>,
    /// The current hunk of the opened file.
    pub hunk: usize,
    /// The patch kept on screen until a newer one is applied.
    pub pinned: Option<Pinned>,
    /// The head's folded part shows.
    pub more: bool,
}

impl ChangesUi {
    pub const EMPTY: Self = Self {
        comparison: None,
        scope: None,
        author: None,
        filter: String::new(),
        file: None,
        hunk: 0,
        pinned: None,
        more: false,
    };
}

/// What the Changes tab does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangesAction {
    /// Show another comparison.
    Compare {
        session: Id,
        comparison: ChangesComparison,
    },
    /// Read another repository (none: the session's own); clears the file, filter and authors.
    Scope {
        session: Id,
        scope: Option<String>,
    },
    /// Show one author's files, or everyone's.
    Author {
        session: Id,
        author: Option<Author>,
    },
    Filter {
        session: Id,
        text: String,
    },
    /// Open the Changes tab on this file (from the transcript, the sidebar or a row).
    Open {
        session: Id,
        path: String,
    },
    /// Open the Changes tab on this file in the session's own comparison and repository, as
    /// Details lists it: another comparison, repository, author or filter chosen in the tab could
    /// leave the file out or serve another patch for the same path.
    OpenOwn {
        session: Id,
        path: String,
    },
    /// Make this file and hunk current (`usize::MAX`: its last hunk).
    Select {
        session: Id,
        path: String,
        hunk: usize,
    },
    /// Back to the list.
    Close {
        session: Id,
    },
    /// Append to the draft (never sent), and focus the composer.
    Capture {
        session: Id,
        text: String,
    },
    /// Keep this patch on screen.
    Pin {
        session: Id,
        pinned: Pinned,
    },
    /// Show the newer patch the host serves.
    Apply {
        session: Id,
    },
    /// Unfold or fold the head's switches, identity, repositories and filter.
    More {
        session: Id,
    },
    /// Move the keyboard focus to the path filter.
    FocusFilter {
        session: Id,
    },
}

/// A comparison in the switch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparisonChip {
    /// `seg-compare-<slug>`.
    pub id: String,
    /// `since session`, `unstaged`.
    pub label: String,
    pub value: ChangesComparison,
    pub selected: bool,
    /// The host cannot serve it now, for this reason; it does not switch.
    pub unavailable: Option<String>,
    pub action: Option<Action>,
}

/// A repository the tab can read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeChip {
    /// `seg-scope-<n>`.
    pub id: String,
    pub label: String,
    pub selected: bool,
    pub action: Action,
}

/// An author chip: `all 14`, `a1 6`, `parent 3`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorChip {
    /// `seg-author-<all|parent|id>`.
    pub id: String,
    pub label: String,
    pub count: usize,
    pub selected: bool,
    pub hint: String,
    pub action: Action,
}

/// The comparison's identity: what it compares, between which commits, in which repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparisonLine {
    pub label: String,
    /// `aece0a19 → 7c41d0e2`; `→ 7c41d0e2` without a base.
    pub range: Option<String>,
    pub root: String,
    /// `14 files · +120 −30`.
    pub totals: String,
}

/// A file's git status, as a letter and in words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub letter: &'static str,
    pub words: &'static str,
}

impl Status {
    pub(super) const UNKNOWN: Self = Self {
        letter: "·",
        words: "changed",
    };

    pub(super) fn of(status: &str) -> Self {
        let (letter, words) = match status {
            "??" => ("?", "untracked"),
            "Conflict" | "DD" | "AU" | "UD" | "UA" | "DU" | "AA" | "UU" => ("!", "conflict"),
            _ => match status.chars().next() {
                Some('M') => ("M", "modified"),
                Some('A') => ("A", "added"),
                Some('D') => ("D", "deleted"),
                Some('R') => ("R", "renamed"),
                Some('C') => ("C", "copied"),
                Some('T') => ("T", "type changed"),
                Some('U') => ("!", "conflict"),
                _ => return Self::UNKNOWN,
            },
        };
        Self { letter, words }
    }
}

/// One changed file in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRow {
    /// `change-file-<n>`.
    pub id: String,
    pub path: String,
    /// Renamed from.
    pub from: Option<String>,
    pub status: Status,
    pub added: Option<u32>,
    pub removed: Option<u32>,
    /// Neither count is known: git reports no lines (a binary file).
    pub binary: bool,
    pub authors: Vec<String>,
    /// `! edited outside a1's scope`, `! edited by a1 and a2`, in the Held tone.
    pub flag: Option<String>,
    pub selected: bool,
    /// What screen readers and the hint say.
    pub label: String,
    pub open: Action,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangesTab {
    pub session: Id,
    /// The summary and where the list's read stands.
    pub part: PartBody,
    pub comparison: Option<ComparisonLine>,
    pub comparisons: Vec<ComparisonChip>,
    /// The repositories the host can read here; none while it offers only one.
    pub scopes: Vec<ScopeChip>,
    /// Absent while no file names its author.
    pub authors: Vec<AuthorChip>,
    pub filter: String,
    pub files: Vec<FileRow>,
    /// `Showing 500 of 812 paths`.
    pub bound: Option<String>,
    /// What the host says went wrong reading the repository (git unavailable, no project).
    pub error: Option<String>,
    /// Stands for the list when it is read but shows nothing.
    pub empty: Option<String>,
    pub selected: Option<FileDiff>,
    /// The head's switches, identity, repositories and filter show where the head folds (a
    /// terminal, a phone).
    pub more: bool,
    /// `a1's files · paths with “auth”`: what narrows the list, said on the folded head.
    pub narrowed: Option<String>,
}

/// The comparison the tab shows.
pub(super) fn comparison(ui: &ChangesUi) -> ChangesComparison {
    ui.comparison.unwrap_or(ChangesComparison::Session)
}

fn slug(comparison: ChangesComparison) -> &'static str {
    match comparison {
        ChangesComparison::Session => "session",
        ChangesComparison::Candidate => "candidate",
        ChangesComparison::All => "all",
        ChangesComparison::Unstaged => "unstaged",
        ChangesComparison::Staged => "staged",
        ChangesComparison::Other => "other",
    }
}

/// The comparison's name before the host has named it.
fn fallback(comparison: ChangesComparison) -> &'static str {
    match comparison {
        ChangesComparison::Session => "since session",
        ChangesComparison::Candidate => "candidate",
        ChangesComparison::All => "working tree",
        ChangesComparison::Unstaged => "unstaged",
        ChangesComparison::Staged => "staged",
        ChangesComparison::Other => "other",
    }
}

/// The session's own comparison, which the tab's count and the sidebar read.
pub fn target(cx: &Cx<'_>) -> DetailRef {
    DetailRef::SessionChanges {
        id: cx.id.to_string(),
        comparison: ChangesComparison::Session,
        scope: None,
    }
}

/// The list the tab shows: its comparison in its repository.
pub(super) fn shown_target(cx: &Cx<'_>) -> DetailRef {
    DetailRef::SessionChanges {
        id: cx.id.to_string(),
        comparison: comparison(&cx.ui.changes),
        scope: cx.ui.changes.scope.clone(),
    }
}

fn served<'a>(cx: &Cx<'a>, target: &DetailRef) -> Option<&'a SessionChanges> {
    match cx.feed.detail(target)?.body()? {
        DetailBody::SessionChanges(changes) => Some(changes),
        _ => None,
    }
}

/// `12 files changed`, `no changes yet`; not read while the host has not counted them.
pub fn summary(cx: &Cx<'_>) -> String {
    match cx.state.and_then(|state| state.changes) {
        Some(0) => "no changes yet".into(),
        Some(1) => "1 file changed".into(),
        Some(n) => format!("{n} files changed"),
        None => "changed files not counted yet".into(),
    }
}

fn plural(n: u32) -> String {
    if n == 1 {
        "1 file".into()
    } else {
        format!("{n} files")
    }
}

/// `+12 −3`; empty when git counts no lines.
pub(super) fn counts(file: &ChangedFile) -> String {
    match (file.added, file.removed) {
        (None, None) => String::new(),
        (added, removed) => format!("+{} \u{2212}{}", added.unwrap_or(0), removed.unwrap_or(0)),
    }
}

/// The path the owner asked for, as the list names it: the same path, the path under the
/// repository's root, or the one path that ends the other (a transcript row names its file
/// from the session's directory).
fn resolve<'a>(asked: &str, served: &'a SessionChanges) -> Option<&'a ChangedFile> {
    if let Some(file) = served.files.iter().find(|file| file.path == asked) {
        return Some(file);
    }
    let root = served.root.trim_end_matches('/');
    if let Some(rest) = asked
        .strip_prefix(root)
        .and_then(|rest| rest.strip_prefix('/'))
    {
        return served.files.iter().find(|file| file.path == rest);
    }
    let mut ends = served.files.iter().filter(|file| {
        file.path.ends_with(&format!("/{asked}")) || asked.ends_with(&format!("/{}", file.path))
    });
    let found = ends.next()?;
    ends.next().is_none().then_some(found)
}

fn flag(file: &ChangedFile) -> Option<String> {
    let names = |authors: &[String]| match authors {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    };
    if file.outside {
        return Some(if file.authors.is_empty() {
            "! edited outside the editing agent's scope".into()
        } else {
            format!("! edited outside {}'s scope", names(&file.authors))
        });
    }
    (file.authors.len() > 1).then(|| format!("! edited by {}", names(&file.authors)))
}

fn matches(file: &ChangedFile, ui: &ChangesUi) -> bool {
    let author = match &ui.author {
        None => true,
        Some(Author::Parent) => file.authors.is_empty(),
        Some(Author::Child(child)) => file.authors.iter().any(|author| author == child),
    };
    let filter = ui.filter.trim().to_lowercase();
    author && (filter.is_empty() || file.path.to_lowercase().contains(&filter))
}

/// What narrows the list, for a head that folds its chips and filter away.
fn narrowed(ui: &ChangesUi) -> Option<String> {
    let mut said = Vec::new();
    match &ui.author {
        Some(Author::Child(child)) => said.push(format!("{child}'s files")),
        Some(Author::Parent) => said.push("the parent's files".to_owned()),
        None => {}
    }
    let filter = ui.filter.trim();
    if !filter.is_empty() {
        said.push(format!("paths with \u{201c}{filter}\u{201d}"));
    }
    (!said.is_empty()).then(|| said.join(" · "))
}

fn comparisons(cx: &Cx<'_>, served: Option<&SessionChanges>) -> Vec<ComparisonChip> {
    let shown = comparison(&cx.ui.changes);
    let offered: Vec<ComparisonInfo> = match served {
        Some(served) if !served.comparisons.is_empty() => served.comparisons.clone(),
        _ => vec![ComparisonInfo {
            id: shown,
            label: fallback(shown).into(),
            unavailable: None,
        }],
    };
    offered
        .into_iter()
        .filter(|info| info.id != ChangesComparison::Other)
        .map(|info| ComparisonChip {
            id: format!("seg-compare-{}", slug(info.id)),
            selected: info.id == shown,
            value: info.id,
            action: info.unavailable.is_none().then(|| {
                Action::Changes(ChangesAction::Compare {
                    session: cx.id.clone(),
                    comparison: info.id,
                })
            }),
            label: info.label,
            unavailable: info.unavailable,
        })
        .collect()
}

fn scopes(cx: &Cx<'_>, served: &SessionChanges) -> Vec<ScopeChip> {
    // Only the session's own worktree: nothing to choose. A session that must choose a project
    // (the assistant without one) is offered even a single project.
    let own_only = matches!(served.scopes.as_slice(), [only] if only.id == "bound");
    if served.scopes.is_empty() || own_only {
        return Vec::new();
    }
    let current = cx.ui.changes.scope.as_deref().unwrap_or("bound");
    served
        .scopes
        .iter()
        .enumerate()
        .map(|(n, scope)| ScopeChip {
            id: format!("seg-scope-{n}"),
            label: scope.label.clone(),
            selected: scope.id == current,
            action: Action::Changes(ChangesAction::Scope {
                session: cx.id.clone(),
                scope: (scope.id != "bound").then(|| scope.id.clone()),
            }),
        })
        .collect()
}

fn authors(cx: &Cx<'_>, files: &[ChangedFile]) -> Vec<AuthorChip> {
    let mut children: Vec<(&str, usize)> = Vec::new();
    for author in files.iter().flat_map(|file| &file.authors) {
        match children.iter_mut().find(|(id, _)| *id == author.as_str()) {
            Some((_, n)) => *n += 1,
            None => children.push((author, 1)),
        }
    }
    if children.is_empty() {
        return Vec::new();
    }
    let parent = files.iter().filter(|file| file.authors.is_empty()).count();
    let chosen = &cx.ui.changes.author;
    let chip = |id: String, label: String, count: usize, hint: String, author: Option<Author>| {
        AuthorChip {
            id,
            label,
            count,
            hint,
            selected: *chosen == author,
            action: Action::Changes(ChangesAction::Author {
                session: cx.id.clone(),
                author,
            }),
        }
    };
    let mut chips = vec![chip(
        "seg-author-all".into(),
        "all".into(),
        files.len(),
        "Every changed file".into(),
        None,
    )];
    chips.extend(children.into_iter().map(|(id, n)| {
        chip(
            format!("seg-author-{id}"),
            id.to_owned(),
            n,
            format!("Files {id} edited"),
            Some(Author::Child(id.to_owned())),
        )
    }));
    if parent > 0 {
        chips.push(chip(
            "seg-author-parent".into(),
            "parent".into(),
            parent,
            "Files no child agent edited".into(),
            Some(Author::Parent),
        ));
    }
    chips
}

fn file_row(cx: &Cx<'_>, n: usize, file: &ChangedFile, selected: bool) -> FileRow {
    let status = Status::of(&file.status);
    let counts = counts(file);
    let flag = flag(file);
    let mut label = format!("{} · {}", file.path, status.words);
    if let Some(from) = &file.original_path {
        label.push_str(&format!(" from {from}"));
    }
    if !counts.is_empty() {
        label.push_str(&format!(" · {counts}"));
    }
    if !file.authors.is_empty() {
        label.push_str(&format!(" · by {}", file.authors.join(", ")));
    }
    if let Some(flag) = &flag {
        label.push_str(&format!(" · {}", flag.trim_start_matches("! ")));
    }
    FileRow {
        id: format!("change-file-{n}"),
        path: file.path.clone(),
        from: file.original_path.clone(),
        status,
        added: file.added,
        removed: file.removed,
        binary: file.added.is_none() && file.removed.is_none(),
        authors: file.authors.clone(),
        flag,
        selected,
        label,
        open: Action::Changes(ChangesAction::Select {
            session: cx.id.clone(),
            path: file.path.clone(),
            hunk: 0,
        }),
    }
}

fn line(served: &SessionChanges) -> ComparisonLine {
    let short = |sha: &String| sha.chars().take(8).collect::<String>();
    let label = served
        .comparisons
        .iter()
        .find(|info| info.id == served.comparison)
        .map_or_else(
            || fallback(served.comparison).to_owned(),
            |info| info.label.clone(),
        );
    let range = match (&served.base, &served.head) {
        (Some(base), Some(head)) => Some(format!("{} → {}", short(base), short(head))),
        (None, Some(head)) => Some(format!("→ {}", short(head))),
        (Some(base), None) => Some(format!("{} →", short(base))),
        (None, None) => None,
    };
    ComparisonLine {
        label,
        range,
        root: served.root.clone(),
        totals: format!(
            "{} · +{} \u{2212}{}",
            plural(served.total),
            served.added,
            served.removed
        ),
    }
}

pub fn changes_tab(cx: &Cx<'_>) -> ChangesTab {
    let ui = &cx.ui.changes;
    let target = shown_target(cx);
    let served = served(cx, &target);
    let summary = match served {
        Some(served) if served.error.is_some() => "Changes cannot be read".to_owned(),
        Some(served) if served.total == 0 => "no changes yet".to_owned(),
        Some(served) => format!(
            "{} changed · +{} \u{2212}{}",
            plural(served.total),
            served.added,
            served.removed
        ),
        None if target == self::target(cx) => summary(cx),
        None => "changed files not read yet".to_owned(),
    };
    let hue = if served.is_some_and(|served| served.error.is_some()) {
        Hue::Held
    } else {
        Hue::Neutral
    };
    let part = part(cx, Ok(target.clone()), Some(CAPABILITY), summary, hue);
    let all: &[ChangedFile] = served.map_or(&[], |served| served.files.as_slice());
    let shown: Vec<&ChangedFile> = all.iter().filter(|file| matches(file, ui)).collect();
    let opened = ui.file.as_deref().map(|asked| {
        let file = served.and_then(|served| resolve(asked, served));
        let at = file.and_then(|file| shown.iter().position(|row| row.path == file.path));
        Opened {
            asked,
            file,
            prev: at
                .and_then(|at| at.checked_sub(1))
                .and_then(|at| shown.get(at).copied()),
            next: match at {
                Some(at) => shown.get(at + 1).copied(),
                None => shown.first().copied(),
            },
            list_read: served.is_some(),
            reference: served.map_or_else(
                || fallback(comparison(ui)).to_owned(),
                |served| {
                    let line = line(served);
                    match line.range {
                        Some(range) => format!("{} {}", line.label, range.replace(' ', "")),
                        None => line.label,
                    }
                },
            ),
        }
    });
    let selected_path = opened
        .as_ref()
        .and_then(|opened| opened.file)
        .map(|file| file.path.as_str());
    let files: Vec<FileRow> = shown
        .iter()
        .enumerate()
        .map(|(n, file)| file_row(cx, n, file, selected_path == Some(file.path.as_str())))
        .collect();
    let bound = served.and_then(|served| {
        let listed = u32::try_from(served.files.len()).unwrap_or(u32::MAX);
        if listed < served.total {
            Some(format!("Showing {listed} of {} paths", served.total))
        } else {
            served
                .truncated
                .then(|| "The list was cut at the host's read limit".to_owned())
        }
    });
    let empty = served
        .filter(|served| served.error.is_none())
        .and_then(|served| {
            if served.files.is_empty() {
                Some("Nothing changed in this comparison.".to_owned())
            } else if files.is_empty() && !ui.filter.trim().is_empty() {
                Some(format!(
                    "No changed path matches \u{201c}{}\u{201d}.",
                    ui.filter.trim()
                ))
            } else if files.is_empty() {
                Some("This author changed no files here.".to_owned())
            } else {
                None
            }
        });
    ChangesTab {
        session: cx.id.clone(),
        part,
        comparison: served.map(line),
        comparisons: comparisons(cx, served),
        scopes: served.map(|served| scopes(cx, served)).unwrap_or_default(),
        authors: authors(cx, all),
        filter: ui.filter.clone(),
        files,
        bound,
        error: served.and_then(|served| served.error.clone()),
        empty,
        more: ui.more,
        narrowed: narrowed(ui),
        selected: opened
            .as_ref()
            .map(|opened| changes_diff::file_diff(cx, opened)),
    }
}

/// What the tab reads while it shows: its list, the opened file's patch, and the pin of a
/// patch once read.
pub fn fetches(cx: &Cx<'_>) -> Vec<Action> {
    let target = shown_target(cx);
    let mut wanted: Vec<Action> = load(cx, Ok(target.clone()), Some(CAPABILITY))
        .into_iter()
        .collect();
    let path = cx.ui.changes.file.as_deref().and_then(|asked| {
        served(cx, &target)
            .and_then(|served| resolve(asked, served))
            .map(|file| file.path.clone())
    });
    if let Some(path) = path {
        wanted.extend(load(
            cx,
            Ok(changes_diff::target(cx, &path)),
            Some(CAPABILITY),
        ));
        wanted.extend(changes_diff::pin(cx, &path));
    }
    wanted
}

/// Appends `text` to the draft, a blank line after what is there.
fn append(draft: &str, text: &str) -> String {
    let kept = draft.trim_end();
    if kept.is_empty() {
        text.to_owned()
    } else {
        format!("{kept}\n\n{text}")
    }
}

pub(super) fn act(state: &mut State, action: ChangesAction, feed: &Feed) -> Vec<Effect> {
    match action {
        ChangesAction::Compare {
            session,
            comparison,
        } => {
            let ui = state.ui_mut(&session);
            ui.focus = Focus::Main;
            let changes = &mut ui.changes;
            changes.comparison = Some(comparison);
            changes.hunk = 0;
            changes.pinned = None;
            Vec::new()
        }
        ChangesAction::Scope { session, scope } => {
            let ui = state.ui_mut(&session);
            ui.focus = Focus::Main;
            ui.changes = ChangesUi {
                comparison: ui.changes.comparison,
                scope,
                more: ui.changes.more,
                ..ChangesUi::EMPTY
            };
            Vec::new()
        }
        ChangesAction::Author { session, author } => {
            let ui = state.ui_mut(&session);
            ui.focus = Focus::Main;
            ui.changes.author = author;
            Vec::new()
        }
        ChangesAction::Filter { session, text } => {
            state.ui_mut(&session).changes.filter = text;
            Vec::new()
        }
        ChangesAction::Open { session, path } => {
            let effects = state.act(
                Action::Tab {
                    session: session.clone(),
                    tab: Tab::Changes,
                },
                feed,
            );
            let changes = &mut state.ui_mut(&session).changes;
            changes.file = Some(path);
            changes.hunk = 0;
            effects
        }
        ChangesAction::OpenOwn { session, path } => {
            // A patch pinned for this very file in this comparison stays, so a newer one still
            // waits for Apply rather than replacing it unasked.
            let own = DetailRef::SessionChangeFile {
                id: session.to_string(),
                comparison: ChangesComparison::Session,
                scope: None,
                path: path.clone(),
            };
            let ui = state.ui_mut(&session);
            let pinned = ui.changes.pinned.take().filter(|pin| pin.target == own);
            ui.changes = ChangesUi {
                more: ui.changes.more,
                pinned,
                ..ChangesUi::EMPTY
            };
            act(state, ChangesAction::Open { session, path }, feed)
        }
        ChangesAction::Select {
            session,
            path,
            hunk,
        } => {
            let ui = state.ui_mut(&session);
            ui.focus = Focus::Main;
            ui.changes.file = Some(path);
            ui.changes.hunk = hunk;
            Vec::new()
        }
        ChangesAction::Close { session } => {
            let changes = &mut state.ui_mut(&session).changes;
            changes.file = None;
            changes.hunk = 0;
            Vec::new()
        }
        ChangesAction::Capture { session, text } => {
            let now = feed.now;
            let ui = state.ui_mut(&session);
            ui.draft = append(&ui.draft, &text);
            ui.focus = Focus::Composer;
            ui.notices.push(
                Hue::Neutral,
                "Added to your message; it is not sent until you send it",
                now,
            );
            vec![Effect::Focus(COMPOSER_FIELD)]
        }
        ChangesAction::Pin { session, pinned } => {
            state.ui_mut(&session).changes.pinned = Some(pinned);
            Vec::new()
        }
        ChangesAction::Apply { session } => {
            state.ui_mut(&session).changes.pinned = None;
            Vec::new()
        }
        ChangesAction::More { session } => {
            let changes = &mut state.ui_mut(&session).changes;
            changes.more = !changes.more;
            Vec::new()
        }
        // The filter may sit in the folded part of the head: unfold it first.
        ChangesAction::FocusFilter { session } => {
            state.ui_mut(&session).changes.more = true;
            vec![Effect::Focus(FILTER_FIELD)]
        }
    }
}
