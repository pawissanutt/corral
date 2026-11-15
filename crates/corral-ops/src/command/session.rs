//! Session controls shared by daemon and local command execution.
use anyhow::{Result, ensure};
use corral_protocol::cmd::session::SessionCommand;

use super::{Exec, Verb};
use crate::session_control::{cancel, keys, pause, relaunch, send, stop};

impl Verb for SessionCommand {
    fn touches_runtime(&self) -> bool {
        true
    }

    fn execute(self, cx: &mut Exec<'_>) -> Result<()> {
        let delegated_owner = cx.invoker.owner_action.is_some();
        ensure!(
            (delegated_owner
                || (!cx.invoker.agent_entry()
                    && !cx.invoker.managed_marker()
                    && !cx.invoker.pm_marker()))
                && cx.invoker.resolve(&cx.ctx.store)?.0 == corral_core::Actor::Owner,
            "owner_only: session controls require the owner, not a PM, worker, reviewer or assistant"
        );
        let (value, json, description) = match self {
            Self::Cancel {
                id,
                generation,
                turn,
                json,
            } => {
                let target = crate::session_control::resolve(&cx.ctx, &id)?;
                target.require_task_family()?;
                (
                    cancel::cancel(&cx.ctx, &target, generation, turn)?,
                    json,
                    format!("{id}: cancelled"),
                )
            }
            Self::Pause {
                id,
                generation,
                json,
            } => {
                let target = crate::session_control::resolve(&cx.ctx, &id)?;
                target.require_task_family()?;
                (
                    pause::pause(&cx.ctx, &target, generation)?,
                    json,
                    format!("{id}: paused"),
                )
            }
            Self::Resume { id, json } => {
                let target = crate::session_control::resolve(&cx.ctx, &id)?;
                target.require_task_family()?;
                (
                    pause::resume(&cx.ctx, &target)?,
                    json,
                    format!("{id}: resumed"),
                )
            }
            Self::Stop {
                id,
                generation,
                json,
            } => {
                let target = crate::session_control::resolve(&cx.ctx, &id)?;
                target.require_task_family()?;
                (
                    stop::stop(&cx.ctx, &target, generation)?,
                    json,
                    format!("{id}: stopped"),
                )
            }
            Self::Relaunch { id, replace, json } => {
                let target = crate::session_control::resolve(&cx.ctx, &id)?;
                target.require_task_family()?;
                (
                    relaunch::relaunch(&cx.ctx, &target, replace)?,
                    json,
                    format!("{id}: relaunched"),
                )
            }
            Self::PmOpen { json } => (relaunch::pm_open(&cx.ctx)?, json, "pm: opened".to_owned()),
            Self::Send {
                id,
                text,
                policy,
                json,
            } => {
                let target = crate::session_control::resolve(&cx.ctx, &id)?;
                target.require_task_family()?;
                (
                    send::send(&cx.ctx, &target, &text, &policy)?,
                    json,
                    format!("{id}: sent"),
                )
            }
            Self::Keys {
                id,
                keys,
                text,
                json,
            } => {
                let target = crate::session_control::resolve(&cx.ctx, &id)?;
                target.require_task_family()?;
                (
                    keys::keys(&cx.ctx, &target, &keys, text.as_deref())?,
                    json,
                    format!("{id}: keys sent"),
                )
            }
            Self::Other => return Err(super::unknown_command("session other")),
        };
        if json {
            cx.out.json(&value)?;
        } else {
            cx.out.line(format_args!("{description}"));
        }
        Ok(())
    }
}
