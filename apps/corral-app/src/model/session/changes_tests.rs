use corral_protocol::detail::{
    ChangedFile, ChangesComparison, ComparisonInfo, DetailBody, DetailRef, DetailReply, PatchPage,
    SessionChanges,
};

use super::{Action, ChangesAction, ChangesTab, Facts, SessionRole, State, Ui, changes_tab};
use crate::model::Id;
use crate::model::daemon::{DetailCache, Feed, detail_key};
use crate::model::health::daemon_env;
use crate::model::nav::Form;

const SESSION: &str = "B-k3x9";

fn file(path: &str, authors: &[&str], outside: bool) -> ChangedFile {
    ChangedFile {
        path: path.into(),
        original_path: None,
        status: "M".into(),
        added: Some(3),
        removed: Some(1),
        authors: authors.iter().map(|author| (*author).to_owned()).collect(),
        outside,
    }
}

fn comparison(id: ChangesComparison, label: &str, unavailable: Option<&str>) -> ComparisonInfo {
    ComparisonInfo {
        id,
        label: label.into(),
        unavailable: unavailable.map(str::to_owned),
    }
}

fn changes(files: Vec<ChangedFile>) -> SessionChanges {
    SessionChanges {
        comparison: ChangesComparison::Session,
        comparisons: vec![
            comparison(ChangesComparison::Session, "since session", None),
            comparison(
                ChangesComparison::Candidate,
                "candidate",
                Some("This session has no bound task candidate"),
            ),
            comparison(ChangesComparison::Unstaged, "unstaged", None),
            comparison(ChangesComparison::Staged, "staged", None),
        ],
        scopes: Vec::new(),
        scope: None,
        root: "/work/repo".into(),
        base: Some("aece0a19aece0a19aece0a19aece0a19aece0a19".into()),
        head: Some("7c41d0e27c41d0e27c41d0e27c41d0e27c41d0e2".into()),
        total: u32::try_from(files.len()).unwrap(),
        added: 3,
        removed: 1,
        truncated: false,
        error: None,
        files,
    }
}

fn served(feed: &mut Feed, target: DetailRef, body: DetailBody) {
    feed.details.insert(
        detail_key(&target),
        DetailCache {
            transcript_seams: Default::default(),
            target,
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
}

fn list(feed: &mut Feed, files: Vec<ChangedFile>) {
    served(
        feed,
        DetailRef::SessionChanges {
            id: SESSION.into(),
            comparison: ChangesComparison::Session,
            scope: None,
        },
        DetailBody::SessionChanges(changes(files)),
    );
}

fn tab(feed: &Feed, ui: &Ui) -> ChangesTab {
    let id = Id::from(SESSION);
    let facts = Facts {
        role: SessionRole::Build,
        task: None,
    };
    let cx = super::Cx::new(feed, ui, &id, daemon_env(feed, Form::Desktop), &facts);
    changes_tab(&cx)
}

/// The host offers a comparison it cannot serve now: it must stay in the switch with the host's
/// reason and switch to nothing, rather than vanish or open an empty list.
#[test]
fn a_comparison_the_host_cannot_serve_is_shown_unavailable_with_its_reason() {
    let mut feed = Feed::default();
    list(&mut feed, vec![file("src/a.rs", &[], false)]);
    let shown = tab(&feed, &Ui::default());
    let candidate = shown
        .comparisons
        .iter()
        .find(|chip| chip.value == ChangesComparison::Candidate)
        .expect("the candidate stays in the switch");
    assert_eq!(
        candidate.unavailable.as_deref(),
        Some("This session has no bound task candidate")
    );
    assert!(candidate.action.is_none());
    let unstaged = shown
        .comparisons
        .iter()
        .find(|chip| chip.value == ChangesComparison::Unstaged)
        .expect("unstaged is offered");
    assert!(unstaged.unavailable.is_none() && unstaged.action.is_some());
}

/// A captured hunk is what the owner comments on: the draft must carry that hunk's served text
/// exactly, and nothing of the file's other hunks.
#[test]
fn a_hunk_capture_is_the_patch_text_of_that_hunk_only() {
    let first = "@@ -1,2 +1,2 @@ fn load\n-let a = 1;\n+let a = 2;\n context\n";
    let second =
        "@@ -10,3 +10,4 @@ fn save\n keep\n+let cache = Cache::new();\n-let cache = None;\n tail\n";
    let patch = format!(
        "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n{first}{second}"
    );
    let mut feed = Feed::default();
    list(&mut feed, vec![file("src/a.rs", &[], false)]);
    served(
        &mut feed,
        DetailRef::SessionChangeFile {
            id: SESSION.into(),
            comparison: ChangesComparison::Session,
            scope: None,
            path: "src/a.rs".into(),
        },
        DetailBody::SessionChangeFile(PatchPage {
            path: "src/a.rs".into(),
            patch,
            truncated: false,
            candidate: None,
        }),
    );
    let mut state = State::default();
    let id = Id::from(SESSION);
    state.act(
        Action::Changes(ChangesAction::Open {
            session: id.clone(),
            path: "src/a.rs".into(),
        }),
        &feed,
    );
    let shown = tab(&feed, state.ui(SESSION));
    let diff = shown.selected.expect("the opened file shows its diff");
    assert_eq!(diff.hunks.len(), 2);
    state.act(diff.hunks[1].capture.clone(), &feed);
    let draft = &state.ui(SESSION).draft;
    let block = draft
        .split_once("```diff\n")
        .and_then(|(_, rest)| rest.rsplit_once("\n```"))
        .map(|(block, _)| block)
        .expect("the hunk is a diff block");
    assert_eq!(format!("{block}\n"), second);
    assert!(!draft.contains("let a = 2;"));
}

/// A child that edited a file outside the paths it owns must be visible on that file's row, so
/// the owner sees the overlap before landing; a file edited in scope carries no flag.
#[test]
fn an_outside_edit_is_flagged_on_its_file() {
    let mut feed = Feed::default();
    list(
        &mut feed,
        vec![
            file("src/auth/token.rs", &["a1"], true),
            file("src/auth/cache.rs", &["a1"], false),
        ],
    );
    let shown = tab(&feed, &Ui::default());
    let outside = shown
        .files
        .iter()
        .find(|row| row.path == "src/auth/token.rs")
        .expect("the outside edit is listed");
    assert!(
        outside
            .flag
            .as_deref()
            .is_some_and(|flag| flag.contains("a1"))
    );
    let inside = shown
        .files
        .iter()
        .find(|row| row.path == "src/auth/cache.rs")
        .expect("the scoped edit is listed");
    assert!(inside.flag.is_none());
}

/// Details previews the session's own comparison: a row pressed after the tab was switched to
/// Staged in another repository with a filter asked for that view's patch of the path, which
/// the preview never listed.
#[test]
fn a_details_row_opens_its_file_in_the_session_comparison() {
    let feed = Feed::default();
    let mut state = State::default();
    let id = Id::from(SESSION);
    for action in [
        ChangesAction::Compare {
            session: id.clone(),
            comparison: ChangesComparison::Staged,
        },
        ChangesAction::Scope {
            session: id.clone(),
            scope: Some("vendor/lib".into()),
        },
        ChangesAction::Filter {
            session: id.clone(),
            text: "zzz".into(),
        },
        ChangesAction::OpenOwn {
            session: id.clone(),
            path: "src/a.rs".into(),
        },
    ] {
        state.act(Action::Changes(action), &feed);
    }
    let changes = &state.ui(SESSION).changes;
    assert_eq!(changes.comparison, None);
    assert_eq!(changes.scope, None);
    assert!(changes.filter.is_empty());
    assert_eq!(changes.file.as_deref(), Some("src/a.rs"));

    // Opened again from Details with its patch pinned: the pin stays until Apply.
    let pinned = super::changes_diff::Pinned {
        target: DetailRef::SessionChangeFile {
            id: SESSION.into(),
            comparison: ChangesComparison::Session,
            scope: None,
            path: "src/a.rs".into(),
        },
        patch: "@@ -1 +1 @@\n-a\n+b\n".into(),
        truncated: false,
        read_at: "2026-11-19T12:00:00Z".into(),
    };
    state.ui_mut(&id).changes.pinned = Some(pinned.clone());
    state.act(
        Action::Changes(ChangesAction::OpenOwn {
            session: id,
            path: "src/a.rs".into(),
        }),
        &feed,
    );
    assert_eq!(state.ui(SESSION).changes.pinned, Some(pinned));
}
