//! What the owner does on the Agents tab and how it goes out: select a job, open or leave an
//! agent's conversation (the agent that takes a message becomes the composer's receiver while it
//! shows), steer an agent (the receiver and the composer's focus), cancel an agent or stop a job
//! after one confirmation, send a job to the background, change an agent's model, open a long
//! text in full, and page a job's output. Every host operation goes out as `session.child` or
//! `session.job` under a key whose reply is read back and said as a notice; nothing is simulated.
//! Also the tab's rows in the session's action catalog and its keys.

use corral_protocol::session_host::{
    ChildAction, JobAction, SessionActed, SessionChildAct, SessionJobAct, SessionJobActed,
};

use crate::model::Id;
use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::hue::Hue;

use super::super::actions::ActionGroup;
use super::super::dock::refusal;
use super::super::keys::{KeyAction, KeyBinding, bind};
use super::super::{Action, ActionRow, Cx, Focus, State, SubView, Tab, Ui};
use super::{FleetButton, asks, card_of, child, fleet, jobs, ordered, selected_child};

/// The composer's field, which a steer gives the keys to.
const COMPOSER_FIELD: &str = "field-message";

/// A cancel or stop waiting for its one confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asking {
    Cancel(String),
    Stop(String),
}

/// A request this tab sent, read back under its key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FleetRequest {
    Cancel(String),
    Model {
        child: String,
        model: String,
    },
    /// A job, with its name (its command's first line) for the notices.
    Stop {
        job: String,
        name: String,
    },
    Background {
        job: String,
        name: String,
    },
}

impl FleetRequest {
    /// The agent or job it is about.
    fn subject(&self) -> &str {
        match self {
            Self::Cancel(id) | Self::Stop { job: id, .. } | Self::Background { job: id, .. } => id,
            Self::Model { child, .. } => child,
        }
    }

    /// What a notice calls it.
    fn what(&self) -> String {
        match self {
            Self::Cancel(child) => format!("Cancelling {child}"),
            Self::Model { child, model } => format!("Moving {child} to {model}"),
            Self::Stop { name, .. } => format!("Stopping {name}"),
            Self::Background { name, .. } => format!("Sending {name} to the background"),
        }
    }
}

/// The one confirmation of a cancel or stop.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfirmView {
    /// `Cancel a1?`.
    pub title: String,
    pub detail: String,
    /// The destructive choice, named as what it does.
    pub yes: (String, Action),
    /// The safe choice, which has the keys first.
    pub no: (String, Action),
}

/// What the Agents tab does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FleetAction {
    /// Select a background job (the agent selection clears).
    SelectJob {
        session: Id,
        job: String,
    },
    /// Open an agent's conversation; one that takes a message becomes the receiver.
    Open {
        session: Id,
        child: String,
    },
    /// Back to the list; the opened agent gives the receiver back to the session's own agent.
    Leave(Id),
    /// Make the agent the receiver and give the composer the keys.
    Steer {
        session: Id,
        child: String,
    },
    /// Ask once, then cancel the agent.
    Cancel {
        session: Id,
        child: String,
    },
    /// Ask once, then stop the job.
    Stop {
        session: Id,
        job: String,
    },
    /// Answer the waiting confirmation.
    Confirm {
        session: Id,
        yes: bool,
    },
    Background {
        session: Id,
        job: String,
    },
    /// Open an agent's model choices, or close them.
    Models {
        session: Id,
        child: Option<String>,
    },
    SetModel {
        session: Id,
        child: String,
        model: String,
    },
    /// Open or clip a long text (`brief:<child>`, `report:<child>`, `edits:<child>`).
    More {
        session: Id,
        key: String,
    },
    /// Read the job output before the shown bytes.
    Older {
        session: Id,
        job: String,
        token: String,
    },
    /// Back to the job's newest output.
    Newest {
        session: Id,
        job: String,
    },
}

/// Whether a request about `subject` (an agent or a job) is on its way.
pub(super) fn sending(cx: &Cx<'_>, subject: &str) -> bool {
    cx.ui.fleet.pending.iter().any(|(key, request)| {
        request.subject() == subject
            && cx
                .feed
                .operations
                .get(key)
                .is_none_or(|operation| operation.loading)
    })
}

fn steerable(feed: &Feed, session: &Id, child: &str) -> bool {
    feed.live
        .get(session.as_str())
        .and_then(|live| live.fleet.as_ref())
        .is_some_and(|fleet| fleet.children.iter().any(|c| c.id == child && c.steerable))
}

fn sent(state: &mut State, session: &Id, request: FleetRequest) -> Id {
    let key = state.ids.next_id();
    state
        .ui_mut(session)
        .fleet
        .pending
        .insert(key.clone(), request);
    key
}

fn child_act(state: &mut State, session: &Id, child: String, action: ChildAction) -> Vec<Effect> {
    let request = match &action {
        ChildAction::SetModel { model } => FleetRequest::Model {
            child: child.clone(),
            model: model.clone(),
        },
        _ => FleetRequest::Cancel(child.clone()),
    };
    let key = sent(state, session, request);
    vec![Effect::SessionChild {
        key,
        act: SessionChildAct {
            id: session.to_string(),
            child,
            action,
        },
    }]
}

fn job_act(
    state: &mut State,
    session: &Id,
    job: String,
    action: JobAction,
    feed: &Feed,
) -> Vec<Effect> {
    let name = jobs::named(feed, session, &job);
    let request = match action {
        JobAction::Background => FleetRequest::Background {
            job: job.clone(),
            name,
        },
        _ => FleetRequest::Stop {
            job: job.clone(),
            name,
        },
    };
    let key = sent(state, session, request);
    vec![Effect::SessionJob {
        key,
        act: SessionJobAct {
            id: session.to_string(),
            job,
            action,
        },
    }]
}

pub(in crate::model::session) fn act(
    state: &mut State,
    action: FleetAction,
    feed: &Feed,
) -> Vec<Effect> {
    match action {
        FleetAction::SelectJob { session, job } => {
            let ui = state.ui_mut(&session);
            ui.agent = None;
            ui.fleet.job = Some(job);
            ui.fleet.picking = None;
            Vec::new()
        }
        FleetAction::Open { session, child } => {
            let ui = state.ui_mut(&session);
            ui.tab = Tab::Agents;
            ui.sub = Some(SubView::Agent(child.clone()));
            ui.agent = Some(child.clone());
            ui.focus = Focus::Main;
            if steerable(feed, &session, &child) {
                return state.act(
                    Action::Receiver {
                        session,
                        to: Some(child),
                    },
                    feed,
                );
            }
            Vec::new()
        }
        FleetAction::Leave(session) => {
            let ui = state.ui_mut(&session);
            let opened = match ui.sub.take() {
                Some(SubView::Agent(child)) => Some(child),
                other => {
                    ui.sub = other.filter(|sub| matches!(sub, SubView::Usage));
                    None
                }
            };
            if opened.is_some() && ui.to == opened {
                return state.act(Action::Receiver { session, to: None }, feed);
            }
            Vec::new()
        }
        FleetAction::Steer { session, child } => {
            let mut effects = state.act(
                Action::Receiver {
                    session: session.clone(),
                    to: Some(child),
                },
                feed,
            );
            state.ui_mut(&session).focus = Focus::Composer;
            effects.push(Effect::Focus(COMPOSER_FIELD));
            effects
        }
        FleetAction::Cancel { session, child } => {
            state.ui_mut(&session).fleet.asking = Some(Asking::Cancel(child));
            Vec::new()
        }
        FleetAction::Stop { session, job } => {
            state.ui_mut(&session).fleet.asking = Some(Asking::Stop(job));
            Vec::new()
        }
        FleetAction::Confirm { session, yes } => {
            let asked = state.ui_mut(&session).fleet.asking.take();
            match asked {
                Some(Asking::Cancel(child)) if yes => {
                    child_act(state, &session, child, ChildAction::Cancel)
                }
                Some(Asking::Stop(job)) if yes => {
                    job_act(state, &session, job, JobAction::Stop, feed)
                }
                _ => Vec::new(),
            }
        }
        FleetAction::Background { session, job } => {
            job_act(state, &session, job, JobAction::Background, feed)
        }
        FleetAction::Models { session, child } => {
            state.ui_mut(&session).fleet.picking = child;
            Vec::new()
        }
        FleetAction::SetModel {
            session,
            child,
            model,
        } => {
            state.ui_mut(&session).fleet.picking = None;
            child_act(state, &session, child, ChildAction::SetModel { model })
        }
        FleetAction::More { session, key } => {
            let opened = &mut state.ui_mut(&session).fleet.opened;
            if !opened.remove(&key) {
                opened.insert(key);
            }
            Vec::new()
        }
        FleetAction::Older {
            session,
            job,
            token,
        } => vec![Effect::Detail(jobs::page_request(
            &session,
            &job,
            Some(token),
        ))],
        FleetAction::Newest { session, job } => {
            vec![Effect::Detail(jobs::page_request(&session, &job, None))]
        }
    }
}

/// Reads each settled reply of this page's requests once and says how it went.
pub(in crate::model::session) fn settle(ui: &mut Ui, feed: &Feed) {
    let settled: Vec<(Id, FleetRequest)> = ui
        .fleet
        .pending
        .iter()
        .filter(|(key, _)| {
            feed.operations
                .get(*key)
                .is_some_and(|operation| !operation.loading)
        })
        .map(|(key, request)| (key.clone(), request.clone()))
        .collect();
    for (key, request) in settled {
        ui.fleet.pending.remove(&key);
        let Some(operation) = feed.operations.get(&key) else {
            continue;
        };
        let what = request.what();
        if let Some(why) = refusal(operation) {
            ui.notices
                .push(Hue::Fail, format!("{what} was refused: {why}"), feed.now);
            continue;
        }
        let (hue, words) = match &request {
            FleetRequest::Cancel(_) | FleetRequest::Model { .. } => {
                match operation.decode::<SessionActed>() {
                    Some(acted) if !acted.ok => (
                        Hue::Fail,
                        format!(
                            "{what} was refused: {}",
                            acted
                                .reason
                                .unwrap_or_else(|| "the host gave no reason".into())
                        ),
                    ),
                    _ => match &request {
                        FleetRequest::Model { child, model } => (
                            Hue::Neutral,
                            format!("{child} switches to {model} at its next turn"),
                        ),
                        _ => (Hue::Neutral, format!("{} is cancelled", request.subject())),
                    },
                }
            }
            FleetRequest::Stop { name, .. } | FleetRequest::Background { name, .. } => {
                match operation.decode::<SessionJobActed>() {
                    Some(acted) => {
                        let mut words = format!("{name}: {}", acted.state.replace('_', " "));
                        if let Some(reason) = acted.reason {
                            words.push_str(&format!(" · {reason}"));
                        }
                        (Hue::Neutral, words)
                    }
                    None => (Hue::Neutral, format!("{what}: the host answered")),
                }
            }
        };
        ui.notices.push(hue, words, feed.now);
    }
}

/// The confirmation waiting on this device, when one does.
pub(super) fn confirm_view(cx: &Cx<'_>) -> Option<ConfirmView> {
    let session = cx.id.clone();
    let answer = |yes| {
        Action::Fleet(FleetAction::Confirm {
            session: session.clone(),
            yes,
        })
    };
    Some(match cx.ui.fleet.asking.as_ref()? {
        Asking::Cancel(child) => ConfirmView {
            title: format!("Cancel {child}?"),
            detail: "Its edits so far stay in the worktree.".into(),
            yes: (format!("Cancel {child}"), answer(true)),
            no: ("Keep it running".into(), answer(false)),
        },
        Asking::Stop(_) => ConfirmView {
            title: "Stop this job?".into(),
            detail: "Its output so far is kept.".into(),
            yes: ("Stop job".into(), answer(true)),
            no: ("Keep it running".into(), answer(false)),
        },
    })
}

/// The controls of what the tab has selected or opened: an agent's, or a job's.
fn controls(cx: &Cx<'_>) -> Vec<FleetButton> {
    match &cx.ui.sub {
        Some(SubView::Agent(id)) => {
            return fleet(cx)
                .and_then(|fleet| fleet.children.iter().find(|child| child.id == *id))
                .map_or_else(Vec::new, |row| child::buttons(cx, row, true));
        }
        Some(SubView::Job(id)) => return jobs::output(cx, id).buttons,
        Some(SubView::Usage) | None => {}
    }
    if let Some(row) = selected_child(cx) {
        return child::buttons(cx, row, false);
    }
    cx.ui
        .fleet
        .job
        .as_deref()
        .and_then(|job| jobs::detail(cx, job))
        .map_or_else(Vec::new, |detail| detail.buttons)
}

/// The tab's rows in the session's action catalog: an answer for every agent that asks, then
/// the selected agent's or job's controls, an unavailable one with its reason.
pub(in crate::model::session) fn actions(cx: &Cx<'_>) -> Option<ActionGroup> {
    let fleet = fleet(cx);
    let mut rows: Vec<ActionRow> = fleet
        .map(ordered)
        .unwrap_or_default()
        .into_iter()
        .filter(|child| asks(child))
        .filter_map(|child| {
            let card = card_of(cx, &child.id)?;
            Some(super::super::actions::row(
                &format!("answer-{}", child.id),
                format!("Answer {}…", child.id),
                None,
                Action::Dock(super::super::DockAction::Bring {
                    session: cx.id.clone(),
                    card,
                }),
            ))
        })
        .collect();
    if cx.ui.tab == Tab::Agents {
        rows.extend(controls(cx).into_iter().map(|button| ActionRow {
            id: format!("act-{}", button.id.trim_start_matches("btn-")),
            label: button.label,
            command: None,
            key: button.key,
            unavailable: button.unavailable,
            action: button.action,
        }));
    }
    (!rows.is_empty()).then_some(ActionGroup {
        title: "Agents",
        rows,
    })
}

/// The keys of the Agents tab, by stroke and the cap its controls show. They are bare keys, a
/// view's: taken only while no field is edited.
const KEYS: [(&str, &str, &str); 6] = [
    ("o", "o", "open conversation or output"),
    ("a", "a", "answer"),
    ("s", "s", "steer"),
    ("x", "x", "cancel or stop (asks)"),
    ("m", "m", "change model"),
    ("b", "b", "send a job to the background"),
];

/// The walk through the agents and then the jobs, by stroke.
const WALK: [(&str, &str, &str, isize); 4] = [
    ("j", "j", "next agent or job", 1),
    ("down", "↓", "next agent or job", 1),
    ("k", "k", "previous agent or job", -1),
    ("up", "↑", "previous agent or job", -1),
];

pub(in crate::model::session) fn keys() -> Vec<KeyBinding> {
    WALK.iter()
        .map(|&(key, cap, label, _)| bind(key, cap, label, KeyAction::Part))
        .chain(
            KEYS.iter()
                .map(|&(key, cap, label)| bind(key, cap, label, KeyAction::Part)),
        )
        .collect()
}

/// The agent or job a step of `delta` from the selected one lands on, through the agents in
/// the tab's order and then the jobs.
fn walk(cx: &Cx<'_>, delta: isize) -> Option<Action> {
    let session = cx.id.clone();
    let mut order: Vec<Action> = fleet(cx)
        .map(ordered)
        .unwrap_or_default()
        .into_iter()
        .map(|child| Action::SelectAgent {
            session: session.clone(),
            child: Some(child.id.clone()),
        })
        .collect();
    order.extend(jobs::all(cx).iter().map(|job| {
        Action::Fleet(FleetAction::SelectJob {
            session: session.clone(),
            job: job.id.clone(),
        })
    }));
    if order.is_empty() {
        return None;
    }
    let at = order.iter().position(|action| match action {
        Action::SelectAgent { child, .. } => cx.ui.agent.is_some() && *child == cx.ui.agent,
        Action::Fleet(FleetAction::SelectJob { job, .. }) => {
            cx.ui.agent.is_none() && cx.ui.fleet.job.as_deref() == Some(job.as_str())
        }
        _ => false,
    });
    let last = order.len() as isize - 1;
    let next = match at {
        Some(at) => (at as isize + delta).clamp(0, last),
        None if delta > 0 => 0,
        None => last,
    };
    order.into_iter().nth(usize::try_from(next).ok()?)
}

pub(in crate::model::session) fn press(key: &str, cx: &Cx<'_>) -> Option<Action> {
    if cx.ui.tab != Tab::Agents {
        return None;
    }
    if let Some(&(.., delta)) = WALK.iter().find(|(stroke, ..)| *stroke == key) {
        return walk(cx, delta);
    }
    let cap = KEYS.iter().find(|(stroke, ..)| *stroke == key)?.1;
    controls(cx)
        .into_iter()
        .find(|button| button.key == Some(cap) && button.unavailable.is_none())
        .map(|button| button.action)
}
