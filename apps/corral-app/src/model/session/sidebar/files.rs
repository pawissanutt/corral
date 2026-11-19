//! The sidebar's Files tab: the files the session changed, from the same read the Changes tab
//! shows first (the session's own comparison, `DetailRef::SessionChanges`). One row per file:
//! the agents that wrote it (a build's) or its status letter, `!` when it was edited outside its
//! agent's own files, the last two parts of its path, and its added and removed lines. A row opens
//! the file in the Changes tab.

use corral_protocol::detail::{ChangedFile, DetailBody, DetailRef, SessionChanges};

use crate::model::hue::Hue;

use super::super::{Action, ChangesAction, Cx, Tab};

/// The files the rail lists before it points to the Changes tab.
const FILE_ROWS: usize = 50;

#[derive(Debug, Clone, PartialEq)]
pub struct FilesTab {
    pub rows: Vec<SideFile>,
    /// `12 files · +340 −22`.
    pub total: Option<String>,
    /// Why the host could not list the files (`Git unavailable`), in its tone.
    pub note: Option<(Hue, String)>,
    /// `no changes yet`.
    pub empty: Option<String>,
    /// `+38 more in Changes`, which opens the tab.
    pub more: Option<(String, Action)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SideFile {
    /// `side-file-<n>`.
    pub test_id: String,
    pub path: String,
    /// `a1 a2 +1`, or the status letter (`M A D ? R U`).
    pub lead: String,
    /// Edited outside its agent's own files.
    pub outside: bool,
    /// The last two parts of the path.
    pub shown: String,
    /// `+12 −3`, `new`, `bin`.
    pub counts: String,
    pub action: Action,
}

fn letter(status: &str) -> String {
    match status {
        "modified" => "M".into(),
        "added" => "A".into(),
        "deleted" => "D".into(),
        "untracked" => "?".into(),
        "renamed" => "R".into(),
        "unmerged" | "conflicted" => "U".into(),
        other => other
            .chars()
            .next()
            .map_or_else(|| "–".into(), |first| first.to_uppercase().to_string()),
    }
}

/// The last two parts of `path`: `auth/mod.rs` from `src/auth/mod.rs`.
fn tail(path: &str) -> String {
    let parts: Vec<&str> = path.rsplitn(3, '/').collect();
    match parts.as_slice() {
        [file, dir, _] => format!("…/{dir}/{file}"),
        _ => path.to_owned(),
    }
}

fn counts(file: &ChangedFile) -> String {
    match (file.added, file.removed) {
        (Some(added), Some(removed)) => format!("+{added} −{removed}"),
        (Some(added), None) => format!("+{added}"),
        (None, Some(removed)) => format!("−{removed}"),
        (None, None) if file.status == "untracked" || file.status == "added" => "new".into(),
        (None, None) => "bin".into(),
    }
}

fn lead(file: &ChangedFile) -> String {
    match file.authors.as_slice() {
        [] => letter(&file.status),
        [one] => one.clone(),
        [one, two] => format!("{one} {two}"),
        [one, two, rest @ ..] => format!("{one} {two} +{}", rest.len()),
    }
}

fn served<'a>(cx: &Cx<'a>, target: &DetailRef) -> Option<&'a SessionChanges> {
    match cx.feed.detail(target)?.body()? {
        DetailBody::SessionChanges(changes) => Some(changes),
        _ => None,
    }
}

pub(super) fn files_tab(cx: &Cx<'_>, target: &DetailRef) -> FilesTab {
    let Some(changes) = served(cx, target) else {
        return FilesTab {
            rows: Vec::new(),
            total: None,
            note: None,
            empty: None,
            more: None,
        };
    };
    let rows: Vec<SideFile> = changes
        .files
        .iter()
        .take(FILE_ROWS)
        .enumerate()
        .map(|(at, file)| SideFile {
            test_id: format!("side-file-{at}"),
            path: file.path.clone(),
            lead: lead(file),
            outside: file.outside,
            shown: tail(&file.path),
            counts: counts(file),
            action: Action::Changes(ChangesAction::Open {
                session: cx.id.clone(),
                path: file.path.clone(),
            }),
        })
        .collect();
    let total = u64::from(changes.total).max(changes.files.len() as u64);
    let more = total.saturating_sub(rows.len() as u64);
    FilesTab {
        total: (total > 0).then(|| {
            format!(
                "{total} {} · +{} −{}",
                if total == 1 { "file" } else { "files" },
                changes.added,
                changes.removed
            )
        }),
        note: changes
            .error
            .as_ref()
            .map(|error| (Hue::Held, format!("! {error}"))),
        empty: (rows.is_empty() && changes.error.is_none()).then(|| "no changes yet".into()),
        more: (more > 0).then(|| {
            (
                format!("+{more} more in Changes"),
                Action::Tab {
                    session: cx.id.clone(),
                    tab: Tab::Changes,
                },
            )
        }),
        rows,
    }
}
