use corral_protocol::detail::{BoardTask, BuildBoard, DetailBody, DetailRef, DetailReply};
use corral_protocol::session_state::{BoardBrief, Capability, SessionState};

use super::transcript::LiveSession;
use super::{Badge, Cx, Facts, SessionRole, Tab, Ui};
use crate::model::Id;
use crate::model::daemon::{DetailCache, Feed, detail_key};
use crate::model::health::daemon_env;
use crate::model::hue::Hue;
use crate::model::nav::Form;

const SESSION: &str = "B-k3x9";

fn task(id: &str, phase: &str) -> BoardTask {
    BoardTask {
        id: id.into(),
        title: format!("Task {id}"),
        status: "open".into(),
        phase: phase.into(),
        depends: Vec::new(),
        detail: None,
        updated_at: "2026-11-19T12:00:00Z".into(),
    }
}

/// A build whose host serves the board, with its state's board brief, and the board as read.
fn feed(brief: Option<BoardBrief>, tasks: Vec<BoardTask>) -> Feed {
    let mut feed = Feed::default();
    let mut live = LiveSession::default();
    live.replace_state(Some(Box::new(SessionState {
        can: vec![Capability {
            id: "board".into(),
            unavailable: None,
        }],
        board: brief,
        ..SessionState::default()
    })));
    feed.live.insert(Id::from(SESSION), live);
    let target = DetailRef::BuildBoard { id: SESSION.into() };
    let body = DetailBody::BuildBoard(BuildBoard {
        tasks,
        proposed: None,
        unavailable: None,
    });
    feed.details.insert(
        detail_key(&target),
        DetailCache {
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
    feed
}

fn board_badge(feed: &Feed) -> Option<Badge> {
    let id = Id::from(SESSION);
    let facts = Facts {
        role: SessionRole::Build,
        task: None,
    };
    let ui = Ui::default();
    let cx = Cx::new(feed, &ui, &id, daemon_env(feed, Form::Desktop), &facts);
    super::tabs::items(&cx)
        .into_iter()
        .find(|item| item.tab == Tab::Board)
        .and_then(|item| item.badge)
}

fn marked(badge: Option<Badge>) -> bool {
    badge.is_some_and(|badge| badge.text.contains('!') && badge.hue == Hue::Held)
}

/// A blocked task waits on the owner: the Board tab must say so from the strip, whether the
/// host's brief counts it or (before the host briefs it) only the board as read lists it. A
/// board where nothing is blocked or failed is not marked.
#[test]
fn a_blocked_task_marks_the_tab() {
    let briefed = feed(
        Some(BoardBrief {
            open: 2,
            blocked: 1,
            failed: 0,
            proposed: false,
        }),
        vec![task("T-41", "running"), task("T-43", "blocked")],
    );
    assert!(marked(board_badge(&briefed)));

    let read_only = feed(None, vec![task("T-41", "running"), task("T-43", "blocked")]);
    assert!(marked(board_badge(&read_only)));

    let calm = feed(None, vec![task("T-41", "running"), task("T-42", "landed")]);
    assert!(!marked(board_badge(&calm)));
}
