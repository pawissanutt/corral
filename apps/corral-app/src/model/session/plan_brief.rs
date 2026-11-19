//! A proposed task's whole brief. The board reply carries at most 2 KiB of each brief; when the
//! host kept part of one back, Show brief reads the rest from the host (`BuildPlanTask`, paged,
//! joined in offset order) and the whole text takes the clipped one's place. Until the owner asks,
//! or while the host cannot serve it, the task says how much was sent. A read that fails keeps
//! the clipped text with the host's reason and a Retry; a brief whose proposal changed while it
//! was read (a conflict) reads the board again, once.

use corral_protocol::detail::{BoardTaskPlan, BuildPlanTaskPage, DetailBody, DetailRef};

use crate::model::daemon::Feed;
use crate::model::hue::Hue;

use super::super::tabs::load;
use super::super::{Action, Can, Cx, Host};
use super::BoardAction;

/// The host part the rest of a brief is served by.
const CAPABILITY: &str = "board_plan_task";

pub fn target(session: &str, key: &str) -> DetailRef {
    DetailRef::BuildPlanTask {
        id: session.to_owned(),
        key: key.to_owned(),
    }
}

/// The host kept part of the task's brief back from the board reply.
fn clipped(task: &BoardTaskPlan) -> bool {
    task.body_bytes > task.body.len() as u64
}

/// The whole brief of `key`, once every byte of it has been read.
fn whole<'f>(feed: &'f Feed, session: &str, key: &str) -> Option<&'f BuildPlanTaskPage> {
    match feed.detail(&target(session, key))?.body()? {
        DetailBody::BuildPlanTask(page)
            if page.offset == 0 && page.body.len() as u64 == page.bytes =>
        {
            Some(page)
        }
        _ => None,
    }
}

/// The read of the rest is under way, or failed with its reason and a Retry.
#[derive(Debug, Clone, PartialEq)]
pub struct BriefRead {
    /// `Reading the whole brief…`, or why the rest could not be read.
    pub words: String,
    pub hue: Hue,
    pub retry: Option<Action>,
}

/// What a proposed task shows of its brief.
pub(super) struct Shown {
    /// The brief while open: the whole one once read, else what the board reply carried.
    pub body: Option<String>,
    /// How much the host kept back: before the owner asks, or while the rest cannot be read.
    pub more: Option<String>,
    pub read: Option<BriefRead>,
}

/// `2.0 KiB`.
fn kib(bytes: u64) -> String {
    format!("{:.1} KiB", bytes as f64 / 1024.0)
}

/// Whether the host can serve the rest of a brief now; else why not.
fn servable(cx: &Cx<'_>) -> Result<(), &'static str> {
    if cx.host != Host::Running {
        return Err("the rest can be read once the host runs");
    }
    match cx.can(CAPABILITY) {
        Can::Yes | Can::Unknown => Ok(()),
        Can::Unavailable(_) | Can::Absent => Err("this host does not serve the rest"),
    }
}

pub(super) fn shown(cx: &Cx<'_>, task: &BoardTaskPlan, open: bool) -> Shown {
    let sent = task.body.trim();
    let sent_body = (open && !sent.is_empty()).then(|| sent.to_owned());
    if !clipped(task) {
        return Shown {
            body: sent_body,
            more: None,
            read: None,
        };
    }
    let first = format!(
        "The host sent the first {} of this brief's {}",
        kib(task.body.len() as u64),
        kib(task.body_bytes)
    );
    if !open {
        return Shown {
            body: None,
            more: Some(format!("{first}; Show brief reads the rest.")),
            read: None,
        };
    }
    let session = cx.id.as_str();
    if let Some(page) = whole(cx.feed, session, &task.key) {
        return Shown {
            body: Some(page.body.trim().to_owned()),
            more: None,
            read: None,
        };
    }
    let retry = Some(Action::Board(BoardAction::Brief {
        session: cx.id.clone(),
        key: task.key.clone(),
    }));
    let cache = cx.feed.detail(&target(session, &task.key));
    if let Some(error) = cache.and_then(|cache| cache.error.as_ref()) {
        let words = match error.code.as_str() {
            "not_found" => {
                "The host no longer proposes this task, so the rest of its brief cannot be read."
                    .to_owned()
            }
            "conflict" => {
                "The plan changed while this brief was read; the board is read again.".to_owned()
            }
            _ if error.message.is_empty() => {
                format!("The rest of the brief could not be read ({}).", error.code)
            }
            _ => format!("The rest of the brief could not be read: {}", error.message),
        };
        return Shown {
            body: sent_body,
            more: None,
            read: Some(BriefRead {
                words,
                hue: Hue::Fail,
                retry,
            }),
        };
    }
    match servable(cx) {
        Ok(()) => Shown {
            body: sent_body,
            more: None,
            read: Some(BriefRead {
                words: "Reading the whole brief…".into(),
                hue: Hue::Neutral,
                retry: None,
            }),
        },
        Err(why) => Shown {
            body: sent_body,
            more: Some(format!("{first}; {why}.")),
            read: None,
        },
    }
}

/// The reads the open, clipped briefs need: each once; a brief whose proposal changed while it
/// was read sends the board and the brief again, once.
pub(super) fn fetches(cx: &Cx<'_>, tasks: &[BoardTaskPlan]) -> Vec<Action> {
    let session = cx.id.as_str();
    tasks
        .iter()
        .filter(|task| clipped(task) && cx.ui.board.opened.contains(&task.key))
        .filter(|task| whole(cx.feed, session, &task.key).is_none())
        .filter_map(|task| {
            let target = target(session, &task.key);
            let conflict = cx
                .feed
                .detail(&target)
                .and_then(|cache| cache.error.as_ref())
                .is_some_and(|error| error.code == "conflict");
            if conflict {
                return (!cx.ui.board.conflicted.contains(&task.key)).then(|| {
                    Action::Board(BoardAction::Conflict {
                        session: cx.id.clone(),
                        key: task.key.clone(),
                    })
                });
            }
            load(cx, Ok(target), Some(CAPABILITY))
        })
        .collect()
}
