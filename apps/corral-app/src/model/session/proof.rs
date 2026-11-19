//! The Proof tab of a task-bound session: the gate and review checks of the run's task, as the
//! engine records them (`DetailRef::Review`), read as Review reads them. This module holds the
//! tab's summary, its badge (`proof?` after a failed read) and its reading and failed states,
//! and says the tab does not apply to a session with no task run; the gate and review rounds are
//! this module's to add, with their state in `ProofUi` and their actions in `ProofAction`.

use corral_protocol::detail::DetailRef;

use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::hue::Hue;

use super::actions::ActionGroup;
use super::keys::KeyBinding;
use super::read_state::ReadState;
use super::tabs::{Badge, PartBody, load, part};
use super::{Action, Cx, State};

/// The Proof tab's state on this device, per session.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProofUi {}

impl ProofUi {
    pub const EMPTY: Self = Self {};
}

/// What the Proof tab does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProofAction {}

#[derive(Debug, Clone, PartialEq)]
pub struct ProofTab {
    pub part: PartBody,
}

/// The run's task's checks; `Err` when the session has no task run.
pub fn target(cx: &Cx<'_>) -> Result<DetailRef, String> {
    match cx.task.filter(|_| cx.role.task_bound()) {
        Some(task) => Ok(DetailRef::Review {
            task: task.to_string(),
        }),
        None => Err("this session has no task run".into()),
    }
}

/// `Gate and review checks of T-12`.
pub fn summary(cx: &Cx<'_>) -> String {
    match cx.task {
        Some(task) => format!("Gate and review checks of {task}"),
        None => "no task".into(),
    }
}

pub fn proof_tab(cx: &Cx<'_>) -> ProofTab {
    ProofTab {
        part: part(cx, target(cx), None, summary(cx), Hue::Neutral),
    }
}

/// `proof?` once a read of the checks failed.
pub fn badge(cx: &Cx<'_>) -> Option<Badge> {
    let target = target(cx).ok()?;
    matches!(
        ReadState::of(cx.feed.detail(&target), cx.feed),
        ReadState::Failed(_)
    )
    .then(|| Badge {
        text: "proof?".into(),
        hue: Hue::Held,
    })
}

/// What the tab reads while it shows.
pub fn fetches(cx: &Cx<'_>) -> Vec<Action> {
    load(cx, target(cx), None).into_iter().collect()
}

pub(super) fn act(_: &mut State, action: ProofAction, _: &Feed) -> Vec<Effect> {
    match action {}
}

/// The tab's rows in the session's action catalog.
pub fn actions(_: &Cx<'_>) -> Option<ActionGroup> {
    None
}

/// The tab's own keys.
pub fn keys() -> Vec<KeyBinding> {
    Vec::new()
}

pub fn press(_: &str, _: &Cx<'_>) -> Option<Action> {
    None
}
