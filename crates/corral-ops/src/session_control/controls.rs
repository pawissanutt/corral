//! Capability projection is entirely store-derived; it never opens a bridge or probes a pane.
use anyhow::Result;
use corral_core::agent::HostHealth;
use corral_protocol::rows::{
    AssistantLocation, AssistantRow, AssistantState, ControlState, SessionControls,
};

use super::Target;
use crate::Ctx;

fn control(refusal: Option<&str>) -> ControlState {
    ControlState {
        available: refusal.is_none(),
        refusal: refusal.map(str::to_owned),
    }
}

pub(crate) fn for_target(ctx: &Ctx, target: &Target) -> Result<SessionControls> {
    let paused = super::pause::is_paused(ctx, &target.record_key)?;
    let ended = target.ended.then_some("ended");
    let bridge = ended
        .or_else(|| (!target.bridge).then_some("no_bridge"))
        .or_else(|| (target.health != HostHealth::Healthy).then_some("host_lost"));
    let pane_guard = (target.pane.is_some() && !target.owned).then_some("not_owned");
    let pause_role = (!matches!(target.role.as_str(), "worker" | "planner" | "pm"))
        .then_some("pause_unsupported");
    let send = ended.or_else(|| {
        if target.bridge {
            None
        } else if target.role == "worker" && target.replyable {
            pane_guard.or_else(|| target.adopted.then_some("adopted_pane"))
        } else {
            Some("no_bridge")
        }
    });
    let relaunch = if target.role == "pm" && target.ended {
        None
    } else {
        ended
            .or_else(|| (target.container.is_none()).then_some("no_bridge"))
            .or_else(|| (target.health == HostHealth::Healthy).then_some("host_live"))
            .or(pane_guard)
            .or_else(|| (target.adopted && target.role != "pm").then_some("adopted_pane"))
    };
    Ok(SessionControls {
        cancel: control(bridge),
        pause: control(
            ended
                .or(pause_role)
                .or(bridge)
                .or_else(|| paused.then_some("already_paused")),
        ),
        resume: control(ended.or_else(|| (!paused).then_some("not_paused"))),
        stop: control(ended.or(pane_guard)),
        relaunch: control(relaunch),
        send: control(send),
        keys: control(
            ended
                .or_else(|| target.pane.is_none().then_some("no_pane"))
                .or(pane_guard)
                .or_else(|| target.adopted.then_some("adopted_pane"))
                .or_else(|| target.bridge.then_some("bridge_present")),
        ),
        take_over: control(Some("not_assistant")),
    })
}

pub(crate) fn for_assistant(row: &AssistantRow) -> SessionControls {
    let ended = (row.state == AssistantState::Ended).then_some("ended");
    let paused = row.state == AssistantState::Paused;
    SessionControls {
        cancel: control(ended.or_else(|| {
            (row.state != AssistantState::Working || row.turn.is_none()).then_some("no_turn")
        })),
        pause: control(
            ended
                .or_else(|| paused.then_some("already_paused"))
                .or_else(|| (row.state == AssistantState::Failed).then_some("host_lost")),
        ),
        resume: control(ended.or_else(|| (!paused).then_some("not_paused"))),
        stop: control(ended),
        send: control(ended),
        relaunch: control(Some("assistant_session")),
        keys: control(Some("assistant_session")),
        take_over: control(
            ended.or_else(|| (row.live_in != AssistantLocation::Pane).then_some("no_pane")),
        ),
    }
}
