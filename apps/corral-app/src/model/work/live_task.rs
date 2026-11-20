//! A task's detail as corrald serves it (runs, checks, artifacts, events) with its loading,
//! stale and error states, a Blocked task's reason, detail and next steps with the reply box,
//! and the task's artifacts.

use corral_protocol::detail::{DetailBody, DetailRef, TaskDetail};

use crate::model::Id;
use crate::model::act;
use crate::model::daemon::{self, Feed, Link};
use crate::model::hue::Hue;

use super::{Button, ButtonKind, Press, stamp};

/// The detail reference of a task.
pub fn task_ref(id: &str) -> DetailRef {
    DetailRef::Task { id: id.to_owned() }
}

/// The task detail corrald last served for `id`, when one has arrived.
pub fn loaded_task<'a>(feed: &'a Feed, id: &str) -> Option<&'a TaskDetail> {
    match feed.detail(&task_ref(id))?.body()? {
        DetailBody::Task(detail) => Some(detail),
        _ => None,
    }
}

/// A one-line state of a detail beside what it shows: loading, stale or failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailNote {
    pub text: String,
    pub hue: Hue,
}

/// The detail's state for a task, or `None` while it is current.
pub fn detail_note(feed: &Feed, id: &str) -> Option<DetailNote> {
    let cache = feed.detail(&task_ref(id));
    let has_body = cache.and_then(|cache| cache.body()).is_some();
    let as_of = cache
        .and_then(|cache| cache.pages.last())
        .map(|page| stamp(&page.as_of, feed))
        .filter(|stamp| !stamp.is_empty());
    let note = |text: String, hue| Some(DetailNote { text, hue });
    if let Some(error) = cache.and_then(|cache| cache.error.as_ref()) {
        let why = if error.code == "not_found" {
            format!("corrald has no detail for {id}.")
        } else if error.message.is_empty() {
            format!("Task detail unavailable ({}).", error.code)
        } else {
            format!("Task detail unavailable: {}", error.message)
        };
        return note(
            match (has_body, as_of) {
                (true, Some(at)) => format!("{why} Showing the detail as of {at}."),
                _ => why,
            },
            Hue::Fail,
        );
    }
    if !has_body {
        return if feed.link == Link::Down {
            note(
                "Task detail unavailable while corrald is unreachable.".into(),
                Hue::Neutral,
            )
        } else {
            note("Loading task detail…".into(), Hue::Neutral)
        };
    }
    match cache {
        Some(cache) if cache.stale || feed.link == Link::Down => note(
            as_of.map_or_else(
                || "Task detail may be out of date.".to_owned(),
                |at| format!("Task detail as of {at} · may be out of date"),
            ),
            Hue::Held,
        ),
        _ => None,
    }
}

/// Why a Blocked task stopped and what gets it moving: the engine's reason, its detail, the
/// reply box when the recommended step is a reply, and one button per other served next step,
/// the first one primary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedCard {
    pub reason: String,
    pub detail: Option<String>,
    pub reply: Option<ReplyBox>,
    pub next: Vec<Button>,
}

/// The recommended reply step in place: what the owner types goes out as that step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyBox {
    pub task: Id,
    /// The step's place in the served `next` list.
    pub index: usize,
    /// The field's spoken name.
    pub label: String,
    pub placeholder: &'static str,
    /// The send button: the step as the refusal sheet would name it ("Reply", "Reply anyway").
    pub send: String,
}

impl ReplyBox {
    /// What pressing send runs with `text`.
    pub fn press(&self, text: String) -> Press {
        Press::Answer {
            task: self.task.clone(),
            index: self.index,
            text,
        }
    }
}

/// A task's artifacts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveEvidence {
    pub title: String,
    /// Each artifact's title and its reference with its kind.
    pub rows: Vec<(String, String)>,
    pub empty: Option<&'static str>,
    pub note: Option<DetailNote>,
    /// The finder over every task's artifacts, narrowed to this one.
    pub find: Button,
}

/// Said when the task has no artifacts.
pub const NO_ARTIFACTS: &str = "No artifacts recorded for this task.";

/// The reply box's field (`field-reply`).
pub const REPLY_FIELD: &str = "field-reply";

/// The task's reply box, when its Blocked card recommends a reply.
pub fn reply_box(task: &daemon::Task) -> Option<ReplyBox> {
    blocked_card(task)?.reply
}

/// The Blocked card of a task: its reason, detail and one button per next step, labelled as
/// the operation it runs ("Dispatch anyway" for a forced one). A recommended reply is the reply
/// box instead of a button.
pub fn blocked_card(task: &daemon::Task) -> Option<BlockedCard> {
    let blocked = task.blocked()?;
    let reply = blocked
        .next
        .first()
        .filter(|next| matches!(next.op, corral_types::Op::Reply { .. }))
        .map(|next| ReplyBox {
            task: task.id.clone(),
            index: 0,
            label: format!("Reply to {}", task.id),
            placeholder: "What the worker should hear",
            send: act::label(&next.op, next.force),
        });
    Some(BlockedCard {
        reason: blocked.reason.clone(),
        detail: blocked.detail.clone(),
        next: blocked
            .next
            .iter()
            .enumerate()
            .skip(usize::from(reply.is_some()))
            .map(|(index, next)| {
                Button::new(
                    act::label(&next.op, next.force),
                    Press::Next {
                        task: task.id.clone(),
                        index,
                    },
                    if index == 0 {
                        ButtonKind::Primary
                    } else {
                        ButtonKind::Ghost
                    },
                )
            })
            .collect(),
        reply,
    })
}

/// Build a task's artifact list from its detail.
pub fn live_evidence(feed: &Feed, id: &str) -> LiveEvidence {
    let detail = loaded_task(feed, id);
    let rows: Vec<(String, String)> = detail
        .map(|detail| {
            detail
                .artifacts
                .iter()
                .map(|artifact| {
                    let source = match &artifact.kind {
                        Some(kind) => format!("{} · {kind}", artifact.reference()),
                        None => artifact.reference(),
                    };
                    (artifact.title.clone(), source)
                })
                .collect()
        })
        .unwrap_or_default();
    LiveEvidence {
        title: format!("Evidence for {id}"),
        empty: (detail.is_some() && rows.is_empty()).then_some(NO_ARTIFACTS),
        rows,
        note: detail_note(feed, id),
        find: Button::new(
            crate::model::artifacts::FIND_IN_ALL,
            Press::FindArtifacts(Id::from(id)),
            ButtonKind::Ghost,
        ),
    }
}
