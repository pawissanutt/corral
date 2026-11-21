//! One changed file's diff in the Changes tab, from its host (`SessionChangeFile`): the patch
//! split into its hunks, each with its served text kept whole (what the owner adds to the
//! message is exactly that text), the rows numbered as the Review screen numbers them, the
//! current hunk and the walk to the hunks and files around it. Once a patch is read it is
//! pinned on this device, so a newer one the host serves waits behind `New diff available`
//! rather than replacing the hunk the owner reads.

use corral_protocol::detail::{ChangedFile, DetailBody, DetailRef, PatchPage};

use crate::model::Id;
use crate::model::daemon::DetailCache;
use crate::model::hue::Hue;
use crate::model::review::{NumberedLine, parse_patch};

use super::changes::{ChangesAction, Status};
use super::tabs::{PartBody, part};
use super::{Action, Cx};

/// The patch the diff shows, kept on this device until the owner applies a newer one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pinned {
    /// The `SessionChangeFile` read it came from.
    pub target: DetailRef,
    pub patch: String,
    /// The host cut the patch at its read limit.
    pub truncated: bool,
    /// When it was read, as served (`as_of`).
    pub read_at: String,
}

/// One hunk of the shown file.
#[derive(Debug, Clone, PartialEq)]
pub struct HunkView {
    /// `hunk-<n>`.
    pub id: String,
    /// `@@ -10,6 +10,9 @@ fn load`, as served.
    pub header: String,
    pub lines: Vec<NumberedLine>,
    /// The hunk the keys act on: drawn bold with its `i/n`.
    pub current: bool,
    /// Makes it the current hunk.
    pub select: Action,
    /// Appends exactly this hunk to the draft, with where it comes from.
    pub capture: Action,
}

/// The opened file: its head, where its patch read stands, its hunks and the walk.
#[derive(Debug, Clone, PartialEq)]
pub struct FileDiff {
    pub path: String,
    /// Renamed from.
    pub from: Option<String>,
    pub status: &'static str,
    /// The child agents that edited it.
    pub authors: Vec<String>,
    /// The patch's read: its summary is the path, then unavailable, reading, failed or read.
    pub part: PartBody,
    /// What git says of the file outside any hunk: binary, mode change, rename.
    pub notes: Vec<String>,
    pub hunks: Vec<HunkView>,
    /// `2/3`, while there are hunks.
    pub position: Option<String>,
    /// Stands for the hunks when there are none: the file left this comparison, or git shows
    /// no text for it.
    pub empty: Option<String>,
    /// `The patch was cut at the host's read limit; the last hunk may be incomplete.`
    pub truncated: Option<String>,
    /// The host serves a newer patch than the one shown: Apply shows it.
    pub newer: Option<Action>,
    pub prev_hunk: Option<Action>,
    pub next_hunk: Option<Action>,
    pub prev_file: Option<Action>,
    pub next_file: Option<Action>,
    /// Appends a reference to the whole file to the draft.
    pub capture_file: Action,
    /// Back to the file list (the phone shows one at a time).
    pub close: Action,
    pub open_worktree_file: Option<Action>,
}

/// The file the diff opens and its neighbours in the shown list.
pub(super) struct Opened<'a> {
    /// The path the owner asked for (a list row's, or a transcript row's).
    pub asked: &'a str,
    /// The served row it resolves to, once the list is read.
    pub file: Option<&'a ChangedFile>,
    pub prev: Option<&'a ChangedFile>,
    /// The file list is read, so a path not in it has no changes in this comparison.
    pub list_read: bool,
    pub next: Option<&'a ChangedFile>,
    /// `since session · aece0a19→7c41d0e2`: where a capture says it came from.
    pub reference: String,
}

/// The read of one file's patch in the comparison and scope the tab shows.
pub(super) fn target(cx: &Cx<'_>, path: &str) -> DetailRef {
    let ui = &cx.ui.changes;
    DetailRef::SessionChangeFile {
        id: cx.id.to_string(),
        comparison: super::changes::comparison(ui),
        scope: ui.scope.clone(),
        path: path.to_owned(),
    }
}

fn page(cache: Option<&DetailCache>) -> Option<&PatchPage> {
    match cache?.body()? {
        DetailBody::SessionChangeFile(page) => Some(page),
        _ => None,
    }
}

/// The pin a read patch takes once nothing (or another file's patch) is pinned.
pub(super) fn pin(cx: &Cx<'_>, path: &str) -> Option<Action> {
    let target = target(cx, path);
    if cx
        .ui
        .changes
        .pinned
        .as_ref()
        .is_some_and(|pinned| pinned.target == target)
    {
        return None;
    }
    let cache = cx.feed.detail(&target)?;
    let served = page(Some(cache))?;
    Some(Action::Changes(ChangesAction::Pin {
        session: cx.id.clone(),
        pinned: Pinned {
            target,
            patch: served.patch.clone(),
            truncated: served.truncated,
            read_at: cache
                .pages
                .first()
                .map(|reply| reply.as_of.clone())
                .unwrap_or_default(),
        },
    }))
}

/// One served hunk: its header line and its text from that line to the next hunk, verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RawHunk<'a> {
    pub header: &'a str,
    pub text: &'a str,
}

/// The patch's text before its first hunk, and its hunks. A hunk ends where the next one or
/// another file's `diff --git` starts.
pub(super) fn split(patch: &str) -> (&str, Vec<RawHunk<'_>>) {
    let mut starts: Vec<(usize, &str)> = Vec::new();
    let mut ends: Vec<usize> = Vec::new();
    let mut at = 0;
    for line in patch.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        if bare.starts_with("@@ -") {
            starts.push((at, bare));
        } else if bare.starts_with("diff --git ") {
            ends.push(at);
        }
        at += line.len();
    }
    let preamble = starts.first().map_or(patch, |(start, _)| &patch[..*start]);
    let hunks = starts
        .iter()
        .enumerate()
        .map(|(n, (start, header))| {
            let next = starts.get(n + 1).map_or(patch.len(), |(next, _)| *next);
            let end = ends
                .iter()
                .copied()
                .find(|end| end > start && *end < next)
                .unwrap_or(next);
            RawHunk {
                header,
                text: &patch[*start..end],
            }
        })
        .collect();
    (preamble, hunks)
}

/// A fence longer than any run of backticks in `text`, so the block cannot close early.
fn fence(text: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for c in text.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    "`".repeat(longest.max(2) + 1)
}

/// What a hunk capture appends: where it is from on one line, then exactly the hunk's text as
/// a `diff` block.
pub(super) fn hunk_capture(path: &str, hunk: &RawHunk<'_>, reference: &str) -> String {
    let fence = fence(hunk.text);
    let body = hunk.text.strip_suffix('\n').unwrap_or(hunk.text);
    format!(
        "`{path}` {} · {reference}\n{fence}diff\n{body}\n{fence}\n",
        hunk.header
    )
}

/// What a file capture appends: the file, its counts and where it is from.
pub(super) fn file_capture(path: &str, counts: &str, reference: &str) -> String {
    if counts.is_empty() {
        format!("`{path}` · {reference}\n")
    } else {
        format!("`{path}` {counts} · {reference}\n")
    }
}

fn select(session: &Id, path: &str, hunk: usize) -> Action {
    Action::Changes(ChangesAction::Select {
        session: session.clone(),
        path: path.to_owned(),
        hunk,
    })
}

/// The diff of the opened file.
pub(super) fn file_diff(cx: &Cx<'_>, opened: &Opened<'_>) -> FileDiff {
    let session = cx.id;
    let path = opened.file.map_or(opened.asked, |file| file.path.as_str());
    let target = target(cx, path);
    let cache = cx.feed.detail(&target);
    let served = page(cache);
    let pinned = cx
        .ui
        .changes
        .pinned
        .as_ref()
        .filter(|pinned| pinned.target == target);
    let newer = pinned
        .zip(served)
        .filter(|(pinned, served)| {
            !cache.is_some_and(|cache| cache.loading) && pinned.patch != served.patch
        })
        .map(|_| {
            Action::Changes(ChangesAction::Apply {
                session: session.clone(),
            })
        });
    let (patch, truncated, read_at) = match (pinned, served) {
        (Some(pinned), _) => (
            Some(pinned.patch.as_str()),
            pinned.truncated,
            Some(pinned.read_at.as_str()),
        ),
        (None, Some(served)) => (
            Some(served.patch.as_str()),
            served.truncated,
            cache
                .and_then(|cache| cache.pages.first())
                .map(|reply| reply.as_of.as_str()),
        ),
        (None, None) => (None, false, None),
    };
    let listed = opened.file.is_some();
    // Until the list is read the asked path is not resolved yet: the diff waits as unread.
    let read = if listed || !opened.list_read {
        Ok(target.clone())
    } else {
        Err(format!(
            "{} has no changes in this comparison",
            opened.asked
        ))
    };
    let status = opened
        .file
        .map_or(Status::UNKNOWN, |file| Status::of(&file.status));
    let part = part(
        cx,
        read,
        Some(super::changes::CAPABILITY),
        path.to_owned(),
        Hue::Neutral,
    );
    let read_words = read_at
        .and_then(|at| crate::model::agents::clock(Some(at), cx.feed))
        .map(|at| format!(" · read {at}"))
        .unwrap_or_default();
    let reference = format!("{}{read_words}", opened.reference);
    let (preamble, raw) = patch.filter(|_| listed).map_or(("", Vec::new()), split);
    let count = raw.len();
    let at = cx.ui.changes.hunk.min(count.saturating_sub(1));
    let hunks: Vec<HunkView> = raw
        .iter()
        .enumerate()
        .map(|(n, hunk)| {
            let cut = truncated && n + 1 == count;
            let lines = parse_patch(hunk.text, !cut)
                .hunks
                .into_iter()
                .next()
                .map(|parsed| parsed.lines)
                .unwrap_or_default();
            let mut reference = reference.clone();
            if cut {
                reference.push_str(" · cut at the host's read limit");
            }
            HunkView {
                id: format!("hunk-{n}"),
                header: hunk.header.to_owned(),
                lines,
                current: n == at,
                select: select(session, path, n),
                capture: Action::Changes(ChangesAction::Capture {
                    session: session.clone(),
                    text: hunk_capture(path, hunk, &reference),
                }),
            }
        })
        .collect();
    let notes = parse_patch(preamble, true).notes;
    let prev_file = opened
        .prev
        .map(|file| select(session, &file.path, usize::MAX));
    let next_file = opened.next.map(|file| select(session, &file.path, 0));
    let empty = if patch.is_some() && listed && hunks.is_empty() && notes.is_empty() {
        Some("Git shows no text changes for this file.".into())
    } else {
        None
    };
    let counts = opened.file.map(super::changes::counts).unwrap_or_default();
    FileDiff {
        path: path.to_owned(),
        from: opened.file.and_then(|file| file.original_path.clone()),
        status: status.words,
        authors: opened
            .file
            .map(|file| file.authors.clone())
            .unwrap_or_default(),
        part,
        notes,
        position: (count > 0).then(|| format!("{}/{count}", at + 1)),
        prev_hunk: if at > 0 && count > 0 {
            Some(select(session, path, at - 1))
        } else {
            prev_file.clone()
        },
        next_hunk: if at + 1 < count {
            Some(select(session, path, at + 1))
        } else {
            next_file.clone()
        },
        hunks,
        empty,
        truncated: (truncated && listed).then(|| {
            "The patch was cut at the host's read limit; the last hunk may be incomplete.".into()
        }),
        newer,
        prev_file,
        next_file,
        capture_file: Action::Changes(ChangesAction::Capture {
            session: session.clone(),
            text: file_capture(path, &counts, &reference),
        }),
        open_worktree_file: (super::files::applies(cx)
            && cx.ui.changes.scope.is_none()
            && opened
                .file
                .is_some_and(|file| !file.outside && !file.status.contains('D')))
        .then(|| {
            Action::Files(super::FilesAction::Open {
                session: session.clone(),
                path: path.to_owned(),
                directory: false,
            })
        }),
        close: Action::Changes(ChangesAction::Close {
            session: session.clone(),
        }),
    }
}
