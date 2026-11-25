//! The Details panel's Changes section: a summary first, from the same read the Changes view
//! shows first (the session's own comparison, `DetailRef::SessionChanges`). The section's header
//! already counts the files, so the body says the added and removed lines and links to the full
//! Changes view; the file list stays folded until the owner unfolds a short preview of its first
//! rows, which ends in a link to the rest. A preview row leads with the file's name, then its
//! folder, quieter; its status mark, `!` when it was edited outside its agent's own files, and its
//! added and removed lines. Who wrote it is said to screen readers and shown in Changes, not on
//! the row. A row opens the file's diff in the Changes view.

use corral_protocol::detail::{ChangedFile, DetailBody, DetailRef, SessionChanges};

use crate::model::hue::Hue;

use super::super::changes::Status;
use super::super::{Action, ChangesAction, Cx, Tab};
use super::SidebarAction;

/// The files an unfolded preview lists before it points to the Changes view.
pub const PREVIEW_ROWS: usize = 5;

#[derive(Debug, Clone, PartialEq)]
pub struct FilesTab {
    /// `+340` and `−22`: the lines the comparison adds and removes.
    pub totals: Option<(String, String)>,
    /// `See all changes`, which opens the Changes view.
    pub all: Option<(String, Action)>,
    /// Why the host could not list the files (`Git unavailable`), in its tone.
    pub note: Option<(Hue, String)>,
    /// `no changes yet`.
    pub empty: Option<String>,
    /// `U 1 conflict · ! 2 outside scope` (edited outside their agent's files), across every
    /// file, so a folded list still says them; in the Fail tone with a conflict, else the Held tone.
    pub cues: Option<(Hue, String)>,
    /// `▸ Preview files` or `▾ Hide files`, which unfolds or folds the preview.
    pub preview: Option<(String, Action)>,
    /// The preview's rows; none while it is folded.
    pub rows: Vec<SideFile>,
    /// `See all 167 changes` under an unfolded preview that does not list every file.
    pub more: Option<(String, Action)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SideFile {
    /// `side-file-<n>`.
    pub test_id: String,
    /// `mod.rs`.
    pub name: String,
    /// `src/auth`, empty at the root.
    pub folder: String,
    /// `M A D ? R U`: the file's state, never by colour alone.
    pub mark: &'static str,
    pub mark_hue: Hue,
    /// Edited outside its agent's own files.
    pub outside: bool,
    /// `+12`, `−3`; none when git counts no lines.
    pub added: Option<String>,
    pub removed: Option<String>,
    /// `new` or `bin` when git counts no lines.
    pub other: Option<String>,
    /// The path, state, counts and who wrote it, for screen readers.
    pub label: String,
    pub action: Action,
}

/// The file's state as the list's mark, from the short or long form a host reports.
fn status(status: &str) -> (Status, Hue) {
    let status = match status {
        "modified" => Status::of("M"),
        "added" => Status::of("A"),
        "deleted" => Status::of("D"),
        "untracked" => Status::of("??"),
        "renamed" => Status::of("R"),
        "copied" => Status::of("C"),
        "typechange" | "type changed" => Status::of("T"),
        "unmerged" | "conflicted" => Status::of("Conflict"),
        other => Status::of(other),
    };
    let hue = match status.letter {
        "!" => Hue::Fail,
        "A" | "?" => Hue::Ok,
        "D" => Hue::Held,
        _ => Hue::Neutral,
    };
    // `!` marks an edit outside its agent's files on this row; a conflict reads `U`.
    let status = if status.letter == "!" {
        Status {
            letter: "U",
            words: status.words,
        }
    } else {
        status
    };
    (status, hue)
}

fn split(path: &str) -> (String, String) {
    match path.rsplit_once('/') {
        Some((folder, name)) => (name.to_owned(), folder.to_owned()),
        None => (path.to_owned(), String::new()),
    }
}

fn row(cx: &Cx<'_>, at: usize, file: &ChangedFile) -> SideFile {
    let (state, mark_hue) = status(&file.status);
    let (name, folder) = split(&file.path);
    let other = match (file.added, file.removed) {
        (None, None) if matches!(state.letter, "A" | "?") => Some("new".to_owned()),
        (None, None) => Some("bin".to_owned()),
        _ => None,
    };
    let counted = file.added.is_some() || file.removed.is_some();
    let added = counted.then(|| format!("+{}", file.added.unwrap_or(0)));
    let removed = counted.then(|| format!("\u{2212}{}", file.removed.unwrap_or(0)));
    let mut label = format!("{}, {}", file.path, state.words);
    if let (Some(added), Some(removed)) = (&added, &removed) {
        label.push_str(&format!(", {added} {removed}"));
    }
    if let Some(other) = &other {
        label.push_str(&format!(", {other}"));
    }
    if !file.authors.is_empty() {
        label.push_str(&format!(", by {}", file.authors.join(", ")));
    }
    if file.outside {
        label.push_str(", edited outside its agent's files");
    }
    SideFile {
        test_id: format!("side-file-{at}"),
        name,
        folder,
        mark: state.letter,
        mark_hue,
        outside: file.outside,
        added,
        removed,
        other,
        label,
        action: Action::Changes(ChangesAction::OpenOwn {
            session: cx.id.clone(),
            path: file.path.clone(),
        }),
    }
}

fn served<'a>(cx: &Cx<'a>, target: &DetailRef) -> Option<&'a SessionChanges> {
    match cx.feed.detail(target)?.body()? {
        DetailBody::SessionChanges(changes) => Some(changes),
        _ => None,
    }
}

/// The section's header count, from the same read as its totals and preview: `167 files
/// changed`. Before that read arrives, or when it failed, the host's state brief counts them
/// (`changes::summary`), whose comparison can differ (a task run's candidate).
pub(super) fn summary(cx: &Cx<'_>, target: &DetailRef) -> String {
    match served(cx, target) {
        Some(changes) if changes.error.is_none() => {
            match u64::from(changes.total).max(changes.files.len() as u64) {
                0 => "no changes yet".into(),
                1 => "1 file changed".into(),
                n => format!("{n} files changed"),
            }
        }
        _ => super::super::changes::summary(cx),
    }
}

/// The section's body, its preview unfolded when `preview`.
pub(super) fn files_tab(cx: &Cx<'_>, target: &DetailRef, preview: bool) -> FilesTab {
    let Some(changes) = served(cx, target) else {
        return FilesTab {
            totals: None,
            all: None,
            note: None,
            empty: None,
            cues: None,
            preview: None,
            rows: Vec::new(),
            more: None,
        };
    };
    let total = u64::from(changes.total).max(changes.files.len() as u64);
    let view = Action::Tab {
        session: cx.id.clone(),
        tab: Tab::Changes,
    };
    let listed = !changes.files.is_empty();
    let rows: Vec<SideFile> = if preview {
        changes
            .files
            .iter()
            .take(PREVIEW_ROWS)
            .enumerate()
            .map(|(at, file)| row(cx, at, file))
            .collect()
    } else {
        Vec::new()
    };
    let rest = total.saturating_sub(rows.len() as u64);
    let conflicts = changes
        .files
        .iter()
        .filter(|file| status(&file.status).0.letter == "U")
        .count();
    let outside = changes.files.iter().filter(|file| file.outside).count();
    let mut cues = Vec::new();
    if conflicts > 0 {
        cues.push(format!(
            "U {conflicts} {}",
            if conflicts == 1 {
                "conflict"
            } else {
                "conflicts"
            }
        ));
    }
    if outside > 0 {
        cues.push(format!("! {outside} outside scope"));
    }
    FilesTab {
        totals: (total > 0).then(|| {
            (
                format!("+{}", changes.added),
                format!("\u{2212}{}", changes.removed),
            )
        }),
        all: (total > 0).then(|| ("See all changes".to_owned(), view.clone())),
        note: changes
            .error
            .as_ref()
            .map(|error| (Hue::Held, format!("! {error}"))),
        empty: (total == 0 && changes.error.is_none()).then(|| "no changes yet".into()),
        cues: (!cues.is_empty()).then(|| {
            let hue = if conflicts > 0 { Hue::Fail } else { Hue::Held };
            (hue, cues.join(" · "))
        }),
        preview: listed.then(|| {
            // The header counts the files; the fold does not count them again.
            let words = if preview {
                "▾ Hide files"
            } else {
                "▸ Preview files"
            };
            (words.to_owned(), Action::Sidebar(SidebarAction::Preview))
        }),
        more: (preview && rest > 0 && !rows.is_empty())
            .then(|| ("See all changes".to_owned(), view)),
        rows,
    }
}

#[cfg(test)]
mod tests {
    use corral_protocol::detail::{
        ChangedFile, ChangesComparison, DetailBody, DetailRef, DetailReply, SessionChanges,
    };

    use super::{PREVIEW_ROWS, files_tab, status};
    use crate::model::Id;
    use crate::model::daemon::{DetailCache, Feed, detail_key};
    use crate::model::health::daemon_env;
    use crate::model::nav::Form;
    use crate::model::session::{Cx, Facts, SessionRole, Ui};

    /// A 167-file session listed every file in Details (up to fifty rows), burying the sections
    /// under it: the list must stay folded, and unfolded show only the preview and a link on.
    #[test]
    fn a_large_change_keeps_its_files_folded_and_previews_only_a_few() {
        let files: Vec<ChangedFile> = (0..167)
            .map(|n| ChangedFile {
                path: format!("src/dir/file{n}.rs"),
                original_path: None,
                status: "M".into(),
                added: Some(2),
                removed: Some(1),
                authors: vec!["a1".into()],
                outside: n == 150,
            })
            .collect();
        let target = DetailRef::SessionChanges {
            id: "B-1".into(),
            comparison: ChangesComparison::Session,
            scope: None,
        };
        let body = DetailBody::SessionChanges(SessionChanges {
            comparison: ChangesComparison::Session,
            comparisons: Vec::new(),
            scopes: Vec::new(),
            scope: None,
            root: "/work".into(),
            base: None,
            head: None,
            total: 167,
            added: 334,
            removed: 167,
            truncated: false,
            error: None,
            files,
        });
        let mut feed = Feed::default();
        feed.details.insert(
            detail_key(&target),
            DetailCache {
                transcript_seams: Default::default(),
                target: target.clone(),
                pages: vec![DetailReply {
                    etag: "e1".into(),
                    as_of: "2026-11-19T12:00:00Z".into(),
                    seq: 1,
                    next: None,
                    body: Some(body.clone()),
                }],
                loading: false,
                stale: false,
                error: None,
                merged: Some(body),
            },
        );
        let id = Id::from("B-1");
        let facts = Facts {
            role: SessionRole::Build,
            task: None,
        };
        let ui = Ui::default();
        let cx = Cx::new(&feed, &ui, &id, daemon_env(&feed, Form::Desktop), &facts);

        // The host's state brief counts nothing here (a task run's would count its candidate):
        // the header follows the read the totals come from.
        assert_eq!(super::summary(&cx, &target), "167 files changed");

        let folded = files_tab(&cx, &target, false);
        assert!(folded.rows.is_empty());
        assert_eq!(folded.more, None);
        assert_eq!(
            folded.totals,
            Some(("+334".to_owned(), "\u{2212}167".to_owned()))
        );
        assert!(folded.all.is_some() && folded.preview.is_some());
        // The one outside edit lies past the preview: the summary still says it.
        assert!(folded.cues.is_some());

        let open = files_tab(&cx, &target, true);
        assert_eq!(open.rows.len(), PREVIEW_ROWS);
        assert_eq!(open.rows[0].name, "file0.rs");
        assert!(open.more.is_some());
    }

    /// A conflict served in Git's short form kept its cue: the old letter table knew only the
    /// long names and showed `UU` as a plain `U` in the neutral ink, like any other state.
    #[test]
    fn a_conflict_in_either_form_reads_u_in_the_fail_tone() {
        for served in ["UU", "AA", "conflicted", "unmerged"] {
            let (state, hue) = status(served);
            assert_eq!((state.letter, hue), ("U", crate::model::hue::Hue::Fail));
        }
        assert_eq!(status("modified").0.letter, "M");
        assert_eq!(status("??").0.letter, "?");
    }
}
