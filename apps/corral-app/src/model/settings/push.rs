//! Settings › Notifications in a daemon session: this browser's Web Push opt-in, as corrald
//! reports it. Push is off until the owner turns it on here; each device opts in for itself,
//! and only kinds corrald can send are offered. Native apps post no system notifications.

use corral_protocol::device::{PushKind, PushStatus};

use crate::model::daemon::{Feed, Link};
use crate::model::devices::DeviceOp;
use crate::model::hue::Hue;

/// The kinds corrald can push, in the order the switches list them.
pub fn kinds() -> [PushKind; 5] {
    [
        PushKind::NeedsYou,
        PushKind::PlanAwaitingAcceptance,
        PushKind::Stuck,
        PushKind::Escalated,
        PushKind::DaemonUnhealthy,
    ]
}

pub fn label(kind: &PushKind) -> &'static str {
    match kind {
        PushKind::NeedsYou => "Needs you",
        PushKind::PlanAwaitingAcceptance => "Plan awaiting acceptance",
        PushKind::Stuck => "Stuck work",
        PushKind::Escalated => "Escalated",
        PushKind::DaemonUnhealthy => "corrald unhealthy",
    }
}

pub fn detail(kind: &PushKind) -> &'static str {
    match kind {
        PushKind::NeedsYou => "a question or decision waits for you in Inbox",
        PushKind::PlanAwaitingAcceptance => "a plan is ready for you to accept or send back",
        PushKind::Stuck => "a task or agent stopped making progress",
        PushKind::Escalated => "the PM escalated something to you",
        PushKind::DaemonUnhealthy => "corrald or one of its runtimes stopped working",
    }
}

pub fn slug(kind: &PushKind) -> &'static str {
    match kind {
        PushKind::NeedsYou => "needs-you",
        PushKind::PlanAwaitingAcceptance => "plan",
        PushKind::Stuck => "stuck",
        PushKind::Escalated => "escalated",
        PushKind::DaemonUnhealthy => "daemon",
    }
}

/// One kind's switch: on when push is on for it; `enabled` when it may be flipped now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KindRow {
    pub kind: PushKind,
    pub on: bool,
    pub enabled: bool,
}

/// What Notifications shows: where push stands, the main switch, the kinds and the delivery
/// facts corrald reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushView {
    pub line: String,
    pub hue: Hue,
    /// The main control's label and the operation it runs, when push may be switched now.
    pub action: Option<(&'static str, DeviceOp)>,
    pub kinds: Vec<KindRow>,
    pub facts: Vec<(&'static str, String)>,
}

/// The phone root list's subtitle for Notifications.
pub fn subtitle(feed: &Feed) -> &'static str {
    match &feed.push {
        Some(status) if status.enabled => "push on for this device",
        Some(status) if status.available => "push off",
        Some(_) => "push not available",
        None => "push status unknown",
    }
}

/// `supported`: this app can receive Web Push here (a browser with the Push API); `web`: it runs
/// in a browser at all.
pub fn view(feed: &Feed, supported: bool, web: bool) -> PushView {
    let mut facts = Vec::new();
    let unavailable = |line: String| PushView {
        line,
        hue: Hue::Neutral,
        action: None,
        kinds: kinds()
            .into_iter()
            .map(|kind| KindRow {
                kind,
                on: false,
                enabled: false,
            })
            .collect(),
        facts: Vec::new(),
    };
    if !web {
        return unavailable(
            "This app doesn't post system notifications. Web Push reaches a paired browser that turns it on in its own Settings › Notifications; what needs you also shows in Inbox and on its badge.".into(),
        );
    }
    if !supported {
        return unavailable(
            "This browser can't receive Web Push: it needs service workers and the Push API (on an iPhone, open the app from the Home Screen). What needs you still shows in Inbox and on its badge.".into(),
        );
    }
    let Some(status) = &feed.push else {
        return unavailable(if feed.link == Link::Up {
            "corrald hasn't reported push for this device, so it can't be turned on from here."
                .into()
        } else {
            "Push settings show once corrald is reachable.".into()
        });
    };
    if !status.available {
        return unavailable(
            "corrald has no Web Push delivery set up, so push can't be turned on. What needs you shows in Inbox and on its badge.".into(),
        );
    }
    // The push service accepting a message is not proof the phone showed it.
    if let Some(at) = &status.last_delivered {
        facts.push(("Last sent (push service accepted)", at.clone()));
    }
    if let Some(error) = &status.last_error {
        facts.push(("Last send error", error.clone()));
    }
    let live = feed.link == Link::Up;
    let (line, hue, action) = if status.enabled {
        (
            "Push is on for this browser for the kinds switched on below.".to_owned(),
            Hue::Ok,
            ("Turn push off", DeviceOp::PushUnsubscribe),
        )
    } else {
        (
            "Push is off. Turning it on asks this browser for permission, then corrald pushes the kinds switched on below to this browser only.".to_owned(),
            Hue::Neutral,
            (
                "Turn push on",
                DeviceOp::PushSubscribe {
                    kinds: kinds().into_iter().collect(),
                },
            ),
        )
    };
    PushView {
        line,
        hue,
        action: live.then_some(action),
        kinds: kinds()
            .into_iter()
            .map(|kind| KindRow {
                on: status.enabled && status.kinds.contains(&kind),
                enabled: live && status.enabled,
                kind,
            })
            .collect(),
        facts,
    }
}

/// The kinds after flipping `kind`; turning the last kind off turns push off.
pub fn flip(status: &PushStatus, kind: &PushKind) -> DeviceOp {
    let mut chosen: Vec<PushKind> = status.kinds.clone();
    if let Some(at) = chosen.iter().position(|k| k == kind) {
        chosen.remove(at);
    } else {
        chosen.push(*kind);
    }
    if chosen.is_empty() {
        DeviceOp::PushUnsubscribe
    } else {
        DeviceOp::PushSubscribe { kinds: chosen }
    }
}
