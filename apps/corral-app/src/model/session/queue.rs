//! The queue dock: the owner's queued input in delivery order (steers first), each with Edit
//! (withdraw it into the draft), ↑ and ↓ (move it among the owner's items, as the host allows)
//! and Remove (withdraw it unsent), through `session.queue.withdraw` and `session.queue.move`.
//! Input from other sources is one read-only line. A move the host would refuse is drawn inert,
//! and an item whose change is in flight takes no other until the host answers. The queue is its
//! one header line until it unfolds: by itself only while no card waits and it holds at most
//! three items, else when the owner presses it (or Alt+↓ gives it the keys).

use corral_protocol::session_host::{
    QueueMoved, QueueWithdrawn, SessionQueueKey, SessionQueueMove, WithdrawOutcome,
};
use corral_protocol::session_state::{InputOrigin, QueuedInput, SendPolicy};

use crate::model::Id;
use crate::model::daemon::{Feed, Operation};
use crate::model::effect::{Effect, QueueRequest};
use crate::model::hue::Hue;

use super::dock::{DockAction, Pending, refusal};
use super::{Action, Can, Cx, SessionRole, State, Ui};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueDock {
    /// `Queue · 2 waiting`.
    pub header: String,
    /// The rows show; folded, the header line stands for them.
    pub open: bool,
    /// Folds or unfolds the queue.
    pub toggle: Action,
    pub waiting: u32,
    /// ` · all run when the agent is next idle`, ` · editing is off in this session`.
    pub clause: Option<String>,
    /// `+3 more` beyond the items the host listed.
    pub more: Option<String>,
    pub items: Vec<QueueItem>,
    /// `+ 2 automated inputs · 1 terminal input · read-only`.
    pub foreign: Option<String>,
    /// Whether the host lets the owner edit, move and remove queued input here.
    pub control: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueItem {
    /// `queue-<n>`.
    pub id: String,
    /// The host's queue key (the input id of a socket input).
    pub key: String,
    /// 1-based, in delivery order.
    pub index: usize,
    /// `→ parent`, `→ a2`.
    pub to: String,
    pub steer: bool,
    /// One line, as the host lists it (at most 200 bytes).
    pub text: String,
    /// A change to it is in flight (`…`): its controls wait.
    pub busy: bool,
    /// Edit, ↑, ↓, × Remove; none while queue control is off.
    pub controls: Vec<QueueControl>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueControl {
    /// `btn-queue-<edit|up|down|remove>-<n>`.
    pub id: String,
    /// `Edit`, `↑`, `↓`, `× Remove`.
    pub label: &'static str,
    /// `Move up`, `Remove`: the phone's menu words.
    pub words: &'static str,
    /// Drawn in Fail ink (Remove).
    pub danger: bool,
    /// None: dimmed and inert (the host would refuse it, or a change is in flight).
    pub action: Option<Action>,
}

fn receiver(cx: &Cx<'_>) -> &'static str {
    if *cx.role == SessionRole::Build {
        "parent"
    } else {
        "agent"
    }
}

fn allows(item: &QueuedInput, action: &str) -> bool {
    item.actions.iter().any(|allowed| allowed == action)
}

/// Whether a change to the item `key` is in flight.
fn busy(ui: &Ui, feed: &Feed, key: &str) -> bool {
    ui.dock.pending.iter().any(|(sent, pending)| {
        matches!(pending, Pending::Withdraw { key: item, .. } | Pending::Move { key: item } if item == key)
            && feed
                .operations
                .get(sent)
                .is_none_or(|operation| operation.loading)
    })
}

fn controls(cx: &Cx<'_>, n: usize, item: &QueuedInput, busy: bool) -> Vec<QueueControl> {
    let session = cx.id.clone();
    let key = item.key.clone();
    let control = |slug: &str,
                   label: &'static str,
                   words: &'static str,
                   allowed: bool,
                   action: DockAction| QueueControl {
        id: format!("btn-queue-{slug}-{n}"),
        label,
        words,
        danger: slug == "remove",
        action: (allowed && !busy).then_some(Action::Dock(action)),
    };
    let withdraw = allows(item, "withdraw");
    vec![
        control(
            "edit",
            "Edit",
            "Edit",
            withdraw,
            DockAction::Withdraw {
                session: session.clone(),
                key: key.clone(),
                edit: true,
            },
        ),
        control(
            "up",
            "↑",
            "Move up",
            allows(item, "move_up"),
            DockAction::Move {
                session: session.clone(),
                key: key.clone(),
                delta: -1,
            },
        ),
        control(
            "down",
            "↓",
            "Move down",
            allows(item, "move_down"),
            DockAction::Move {
                session: session.clone(),
                key: key.clone(),
                delta: 1,
            },
        ),
        control(
            "remove",
            "× Remove",
            "Remove",
            withdraw,
            DockAction::Withdraw {
                session,
                key,
                edit: false,
            },
        ),
    ]
}

/// Input not the owner's, one count per source: `2 automated inputs · 1 terminal input`.
fn foreign(items: &[QueuedInput]) -> Option<String> {
    let count = |origin: InputOrigin| items.iter().filter(|item| item.origin == origin).count();
    let parts: Vec<String> = [
        (InputOrigin::Auto, "automated input"),
        (InputOrigin::Pane, "terminal input"),
        (InputOrigin::Other, "input from another source"),
    ]
    .into_iter()
    .filter_map(|(origin, words)| {
        let n = count(origin);
        (n > 0).then(|| format!("{n} {words}{}", if n == 1 { "" } else { "s" }))
    })
    .collect();
    (!parts.is_empty()).then(|| format!("+ {} · read-only", parts.join(" · ")))
}

/// At most this many items show unfolded by themselves.
const SHOWN_UNASKED: usize = 3;

/// Whether the queue of `session` shows its rows: the owner's choice, else only while no card
/// waits and it holds at most three items, so a waiting card keeps the page's room.
pub(super) fn open(ui: &Ui, feed: &Feed, session: &str) -> bool {
    let state = feed.live.get(session).and_then(|live| live.state.as_ref());
    ui.dock.queue_open.unwrap_or_else(|| {
        state.is_none_or(|state| {
            state.cards.is_empty()
                && usize::try_from(state.queue_total).is_ok_and(|n| n <= SHOWN_UNASKED)
        })
    })
}

/// The queue dock of the session `cx` reads; none while nothing waits.
pub fn queue_dock(cx: &Cx<'_>) -> Option<QueueDock> {
    let state = cx.state?;
    if state.queue_total == 0 && state.queue.is_empty() {
        return None;
    }
    let (control, off) = match cx.can("queue") {
        Can::Yes | Can::Unknown => (true, None),
        Can::Unavailable(why) => (false, Some(format!(" · editing is off: {why}"))),
        Can::Absent => (false, Some(" · editing is off in this session".to_owned())),
    };
    let owner: Vec<&QueuedInput> = state
        .queue
        .iter()
        .filter(|item| item.origin == InputOrigin::Owner)
        .collect();
    let clause = off.or_else(|| {
        let follow_ups = owner.iter().all(|item| item.policy == SendPolicy::FollowUp);
        (follow_ups && !owner.is_empty()).then(|| {
            if owner.iter().all(|item| item.to.is_some()) {
                " · each runs when its receiver is next idle".to_owned()
            } else {
                " · all run when the agent is next idle".to_owned()
            }
        })
    });
    let listed = u32::try_from(state.queue.len()).unwrap_or(u32::MAX);
    let mut ordered = owner;
    ordered.sort_by_key(|item| item.policy != SendPolicy::Steer);
    let items = ordered
        .into_iter()
        .enumerate()
        .map(|(n, item)| {
            let busy = busy(cx.ui, cx.feed, &item.key);
            QueueItem {
                id: format!("queue-{n}"),
                key: item.key.clone(),
                index: n + 1,
                to: format!("→ {}", item.to.as_deref().unwrap_or(receiver(cx))),
                steer: item.policy == SendPolicy::Steer,
                text: item.text.lines().next().unwrap_or_default().to_owned(),
                busy,
                controls: if control {
                    controls(cx, n, item, busy)
                } else {
                    Vec::new()
                },
            }
        })
        .collect();
    Some(QueueDock {
        header: format!("Queue · {} waiting", state.queue_total),
        open: open(cx.ui, cx.feed, cx.id.as_str()),
        toggle: Action::Dock(DockAction::ToggleQueue(cx.id.clone())),
        waiting: state.queue_total,
        clause,
        more: (state.queue_total > listed).then(|| format!("+{} more", state.queue_total - listed)),
        items,
        foreign: foreign(&state.queue),
        control,
    })
}

fn listed<'f>(feed: &'f Feed, session: &str, key: &str) -> Option<&'f QueuedInput> {
    feed.live
        .get(session)
        .and_then(|live| live.state.as_ref())
        .and_then(|state| state.queue.iter().find(|item| item.key == key))
}

/// Withdraws a queued input: back into the draft (`edit`), or away unsent.
pub(super) fn withdraw(
    state: &mut State,
    session: &Id,
    key: String,
    edit: bool,
    feed: &Feed,
) -> Vec<Effect> {
    if busy(state.ui(session.as_str()), feed, &key) {
        return Vec::new();
    }
    let (listed, whole) = listed(feed, session.as_str(), &key).map_or_else(
        || (String::new(), false),
        |item| {
            let whole = u64::try_from(item.text.len()).is_ok_and(|len| len >= item.bytes);
            (item.text.clone(), whole)
        },
    );
    let sent = state.ids.next_id();
    state.ui_mut(session).dock.pending.insert(
        sent.clone(),
        Pending::Withdraw {
            key: key.clone(),
            edit,
            listed,
            whole,
        },
    );
    vec![Effect::SessionQueue {
        key: sent,
        request: QueueRequest::Withdraw(SessionQueueKey {
            id: session.to_string(),
            key,
        }),
    }]
}

/// Moves a queued input one place among the owner's items.
pub(super) fn move_item(
    state: &mut State,
    session: &Id,
    key: String,
    delta: i8,
    feed: &Feed,
) -> Vec<Effect> {
    if busy(state.ui(session.as_str()), feed, &key) {
        return Vec::new();
    }
    let sent = state.ids.next_id();
    state
        .ui_mut(session)
        .dock
        .pending
        .insert(sent.clone(), Pending::Move { key: key.clone() });
    vec![Effect::SessionQueue {
        key: sent,
        request: QueueRequest::Move(SessionQueueMove {
            id: session.to_string(),
            key,
            delta: delta.signum(),
        }),
    }]
}

/// Puts `text` above the draft, a blank line between them.
fn into_draft(ui: &mut Ui, text: &str) {
    ui.draft = if ui.draft.is_empty() {
        text.to_owned()
    } else {
        format!("{text}\n\n{}", ui.draft)
    };
}

/// Reads a queue change's reply once: an edited item's text goes back into the draft, a
/// removed one is said unsent, and a lost race says the input already left the queue.
pub(super) fn settled(ui: &mut Ui, pending: Pending, operation: &Operation, now: u64) {
    match pending {
        Pending::Withdraw {
            edit,
            listed,
            whole,
            ..
        } => {
            if let Some(why) = refusal(operation) {
                ui.notices
                    .push(Hue::Fail, format!("Not withdrawn: {why}"), now);
                return;
            }
            let Some(reply) = operation.decode::<QueueWithdrawn>() else {
                return;
            };
            if reply.outcome != WithdrawOutcome::Withdrawn {
                ui.notices
                    .push(Hue::Held, "That input already left the queue", now);
                return;
            }
            if !edit {
                ui.notices
                    .push(Hue::Neutral, "Removed queued input · not sent", now);
                return;
            }
            match reply.text {
                Some(text) => into_draft(ui, &text),
                None => {
                    into_draft(ui, &listed);
                    if !whole {
                        ui.notices.push(
                            Hue::Held,
                            "The host returned only the start of that input; the rest is not in the draft",
                            now,
                        );
                    }
                }
            }
        }
        Pending::Move { .. } => {
            if let Some(why) = refusal(operation) {
                ui.notices.push(Hue::Fail, format!("Not moved: {why}"), now);
            } else if operation
                .decode::<QueueMoved>()
                .is_some_and(|reply| !reply.moved)
            {
                ui.notices.push(Hue::Held, "That input could not move", now);
            }
        }
        Pending::Card { .. } | Pending::Probe { .. } | Pending::Action { .. } => {}
    }
}
