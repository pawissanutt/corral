use corral_protocol::detail::{
    BoardPlan, BoardTask, BoardTaskPlan, BuildBoard, BuildPlanTaskPage, DetailBody, DetailRef,
    DetailReply,
};
use corral_protocol::session_state::{BoardBrief, Capability, SessionState};

use super::transcript::LiveSession;
use super::{Cx, Facts, SessionRole, Ui, board_tab};
use crate::model::Id;
use crate::model::daemon::{ClientError, DetailCache, Feed, detail_key};
use crate::model::health::daemon_env;
use crate::model::hue::Hue;
use crate::model::nav::Form;

const SESSION: &str = "B-k3x9";

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
    feed
}

const CLIPPED: &str = "Move the token store behind a trait.";
const WHOLE: &str = "Move the token store behind a trait.\n\nAcceptance: every caller compiles against the trait, and the old module is gone.";

/// A build proposing one task whose brief the board reply clipped, with that brief open on this
/// device and its read in `brief`: the whole brief, or the read's failure.
fn brief_tab(brief: Result<&str, &str>) -> super::PlanTask {
    let mut feed = feed(None, Vec::new());
    let board = DetailRef::BuildBoard { id: SESSION.into() };
    let proposed = DetailBody::BuildBoard(BuildBoard {
        tasks: Vec::new(),
        proposed: Some(BoardPlan {
            title: "Split auth".into(),
            project: None,
            why: "Two services.".into(),
            tasks: vec![BoardTaskPlan {
                key: "t1".into(),
                title: "Token store".into(),
                kind: None,
                size: None,
                difficulty: None,
                tags: Vec::new(),
                profile: None,
                depends: Vec::new(),
                body: CLIPPED.into(),
                body_bytes: WHOLE.len() as u64,
            }],
        }),
        unavailable: None,
    });
    if let Some(cache) = feed.details.get_mut(&detail_key(&board)) {
        cache.merged = Some(proposed);
    }
    let target = DetailRef::BuildPlanTask {
        id: SESSION.into(),
        key: "t1".into(),
    };
    let (merged, error) = match brief {
        Ok(text) => (
            Some(DetailBody::BuildPlanTask(BuildPlanTaskPage {
                key: "t1".into(),
                body: text.into(),
                bytes: text.len() as u64,
                offset: 0,
            })),
            None,
        ),
        Err(code) => (
            None,
            Some(ClientError {
                code: code.into(),
                message: "no proposed board task has this key".into(),
            }),
        ),
    };
    feed.details.insert(
        detail_key(&target),
        DetailCache {
            transcript_seams: Default::default(),
            target,
            pages: Vec::new(),
            loading: false,
            stale: false,
            error,
            merged,
        },
    );
    let id = Id::from(SESSION);
    let facts = Facts {
        role: SessionRole::Build,
        task: None,
    };
    let mut ui = Ui::default();
    ui.board.opened.insert("t1".into());
    let cx = Cx::new(&feed, &ui, &id, daemon_env(&feed, Form::Desktop), &facts);
    let tab = board_tab(&cx);
    tab.plan
        .and_then(|plan| plan.tasks.into_iter().next())
        .expect("the proposed task is shown")
}

/// A clipped brief is replaced by the whole brief once read, and stays clipped with its reason
/// (and a way to read it again) when the read fails, instead of showing nothing or pretending
/// the clipped text is all of it.
#[test]
fn a_clipped_brief_is_replaced_by_the_whole_brief_once_read_and_stays_clipped_when_the_read_fails()
{
    let read = brief_tab(Ok(WHOLE));
    assert_eq!(read.body.as_deref(), Some(WHOLE));
    assert!(read.read.is_none());
    assert!(read.more.is_none());

    let failed = brief_tab(Err("not_found"));
    assert_eq!(failed.body.as_deref(), Some(CLIPPED));
    let why = failed.read.expect("the failed read is said");
    assert_eq!(why.hue, Hue::Fail);
    assert!(why.retry.is_some());
}
