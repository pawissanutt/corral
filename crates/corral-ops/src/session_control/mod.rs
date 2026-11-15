//! Session identity and owner controls shared by CLI and daemon command execution.
//! Identity comes from the Agents projection; no command accepts a pane as an identity.
use anyhow::{Context, Result, ensure};
use corral_core::TaskRef;
use corral_core::agent::{ContainerRef, HostHealth, pm_state};
use corral_core::inbox::Recipient;
use corral_protocol::rows::{AgentRow, AgentStatus};

use crate::Ctx;

pub(crate) mod cancel;
pub(crate) mod controls;
pub(crate) mod keys;
pub(crate) mod pause;
pub(crate) mod relaunch;
pub(crate) mod send;
pub(crate) mod stop;

/// A stored snapshot, not proof that a host or pane is still live. Stop/Send/Keys must
/// re-resolve at execution and check `generation` before any host effect. `id` is the
/// public AgentRow id; `record_key` is ContainerRef::key() (or the gate row id) and is the
/// durable pause identity. Never substitute one grammar for the other. `owned` guards
/// every pane action; `adopted` forbids closing or typing into the owner's pane even
/// when owned. Bridge actions do not require a pane. `ended` is container lifecycle,
/// not merely a lost host. No field is obtained by probing a pane or a bridge.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Target {
    pub id: String,
    pub role: String,
    pub task: Option<String>,
    pub container: Option<ContainerRef>,
    pub pane: Option<String>,
    pub generation: Option<u32>,
    pub health: HostHealth,
    pub bridge: bool,
    pub ended: bool,
    pub adopted: bool,
    pub owned: bool,
    pub turn: Option<u64>,
    pub replyable: bool,
    pub record_key: String,
}

impl Target {
    pub(crate) fn from_row(
        ctx: &Ctx,
        row: &AgentRow,
        container: Option<ContainerRef>,
        replyable: bool,
    ) -> Result<Self> {
        let mut target = Self {
            id: row.id.clone(),
            role: row.role.clone(),
            task: row.task.clone(),
            container,
            pane: row.pane.clone(),
            generation: row.generation,
            health: HostHealth::Unbound,
            bridge: false,
            ended: row.status == AgentStatus::Ended,
            adopted: false,
            owned: false,
            turn: row.turn,
            replyable,
            record_key: row.id.clone(),
        };
        if let Some(container) = &target.container {
            target.record_key = container.key();
            if let Some(task) = container.task() {
                let head = ctx.store.head(task).context("reading session task")?;
                target.owned = crate::session_scope::owns_head(ctx, &head)
                    .context("checking session pane ownership")?;
                if let Some(host) = head.agent_head(container).context("reading session host")? {
                    target.generation = Some(host.binding.generation).filter(|n| *n != 0);
                    target.health = host.binding.health;
                    target.bridge = target.generation.is_some();
                    target.ended |= host.binding.closing;
                    if let Some(n) = target.generation {
                        target.turn = head
                            .latest_agent_turn(container, n)?
                            .filter(|turn| turn.ended.is_none())
                            .map(|turn| turn.seq);
                    }
                }
            } else if let ContainerRef::Assistant { session } = container {
                let saved = ctx.store.get_assistant_container(session)?;
                target.generation = Some(saved.state.binding.generation).filter(|n| *n != 0);
                target.health = saved.state.binding.health;
                target.bridge = target.generation.is_some();
                target.adopted = target.pane.is_some();
                let view = ctx.store.session_view()?;
                let socket = ctx.herdr.socket_path()?;
                target.owned = crate::session_scope::owns_inventory(ctx)?
                    && saved
                        .herdr_socket
                        .as_ref()
                        .or(view.binding.as_ref().map(|b| &b.socket))
                        == Some(&socket);
            } else {
                target.owned = crate::session_scope::owns_inventory(ctx)
                    .context("checking PM pane ownership")?;
                target.ended |= stop::pm_stopped(ctx).context("reading PM owner-stop fence")?;
                if row.runtime.as_deref() == Some("corral-pi")
                    && let Some(pm) =
                        pm_state::load_selected(&ctx.store, &pm_state::Selection::work())
                            .context("reading PM session host")?
                {
                    target.adopted = pm.adopted();
                    target.generation = Some(pm.state.binding.generation).filter(|n| *n != 0);
                    target.health = pm.state.binding.health;
                    target.bridge = target.generation.is_some();
                    target.ended |= pm.state.binding.closing;
                    target.turn = pm
                        .state
                        .turns
                        .iter()
                        .filter(|t| Some(t.generation) == target.generation && t.ended.is_none())
                        .max_by_key(|t| t.seq)
                        .map(|t| t.seq);
                }
            }
        } else if let Some(task) = target.task.as_deref() {
            target.owned = crate::session_scope::owns_head(ctx, &ctx.store.head(task)?)?;
        }
        if let Some(recipient) = target.recipient() {
            target.ended |= stop::recipient_stopped(ctx, &recipient)?;
        }
        Ok(target)
    }

    pub(crate) fn require_task_family(&self) -> Result<()> {
        ensure!(
            self.role != "assistant",
            "assistant_session: use assistant.session or assistant.stop for {}",
            self.id
        );
        Ok(())
    }

    pub(crate) fn check_generation(&self, generation: Option<u32>) -> Result<()> {
        ensure!(
            generation.is_none() || generation == self.generation,
            "conflict: session {} is generation {:?}, not {:?}",
            self.id,
            self.generation,
            generation
        );
        Ok(())
    }

    pub(crate) fn require_bridge(&self) -> Result<()> {
        self.require_task_family()?;
        ensure!(!self.ended, "ended: session {} has ended", self.id);
        ensure!(
            self.bridge,
            "no_bridge: session {} has no corral-pi bridge",
            self.id
        );
        ensure!(
            self.health == HostHealth::Healthy,
            "host_lost: session {} has no healthy host",
            self.id
        );
        Ok(())
    }

    pub(crate) fn recipient(&self) -> Option<Recipient> {
        match self.container.as_ref()? {
            ContainerRef::Pm => Some(Recipient::Pm),
            ContainerRef::Assistant { session } => Some(Recipient::Assistant {
                session: session.clone(),
            }),
            container => crate::questions::recipient(container),
        }
    }
}

pub(crate) fn resolve(ctx: &Ctx, id: &str) -> Result<Target> {
    let (row, container, replyable) = crate::app_read::agents::control_identity(ctx, id)?;
    Target::from_row(ctx, &row, container, replyable)
}

/// Central admission for automated starts and replacements. Owner lifecycle commands
/// deliberately bypass this gate. Terminate's durable owner-stop policy belongs here
/// alongside pause, so supervisors cannot accidentally resurrect an owner-ended host.
pub(crate) fn automation_may_start(ctx: &Ctx, recipient: &Recipient) -> Result<bool> {
    Ok(!automated_input_must_wait(ctx, recipient)?)
}

/// Delivery and restart share one fence; stop is not a resumable pause.
pub(crate) fn automated_input_must_wait(ctx: &Ctx, recipient: &Recipient) -> Result<bool> {
    Ok(pause::recipient_paused(ctx, recipient)? || stop::recipient_stopped(ctx, recipient)?)
}

/// Bounded confirmation effects use the same identity and ownership facts as execution.
/// The generation itself is bound into the previewed command, not a task revision.
pub fn stop_preview(
    ctx: &Ctx,
    id: &str,
    generation: Option<u32>,
) -> Result<(Option<u32>, Vec<String>)> {
    let target = resolve(ctx, id)?;
    target.require_task_family()?;
    target.check_generation(generation)?;
    ensure!(!target.ended, "ended: session {} has ended", target.id);
    if target.pane.is_some() {
        ensure!(
            target.owned,
            "not_owned: session pane belongs to another herdr server"
        );
    }
    let mut effects = vec![format!(
        "Stop session {} host generation {}",
        target.id,
        target
            .generation
            .map_or_else(|| "none (foreign harness)".into(), |n| n.to_string())
    )];
    if let Some(pane) = &target.pane {
        effects.push(if target.adopted {
            format!("Leave adopted owner pane {pane} open; ask only the host to exit")
        } else {
            format!("Close owned pane {pane}")
        });
    } else {
        effects.push("No pane to close".into());
    }
    effects.push(match target.role.as_str() {
        "worker" => "End run as stopped by owner; task waits in Needs me without a dispatch slot; keep worktree and branch",
        "planner" => "End plan run as stopped by owner; keep task worktree and branch",
        "reviewer" => "Cancel review child with no verdict; keep task worktree and branch",
        "plan_reviewer" => "Cancel plan review with no verdict; keep task worktree and branch",
        "gate" => "Cancel gate with no result; keep task worktree and branch",
        "pm" => "Ask PM host to exit; do not relaunch until the owner launches it again",
        _ => "End session; retain its transcript",
    }.into());
    Ok((target.generation, effects))
}

#[cfg(all(test, target_os = "linux"))]
#[path = "generation_tests.rs"]
mod generation_tests;
