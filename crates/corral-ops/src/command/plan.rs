//! `plan` command executors.
use anyhow::{Result, bail};
use corral_protocol::cmd::plan::PlanCommand;

use super::{Exec, Verb};

impl Verb for PlanCommand {
    fn execute(self, cx: &mut Exec<'_>) -> Result<()> {
        let ctx = &cx.ctx;
        match self {
            Self::Cancel {
                id,
                owner_ok,
                reason,
                json,
            } => {
                let head = ctx.store.head(&id)?;
                let task = if head.tracker && head.approved_scope.is_some() {
                    crate::move_task_with_decision(
                        ctx,
                        &id,
                        corral_core::Status::Dropped,
                        false,
                        reason.as_deref(),
                        "owner",
                        owner_ok,
                    )?
                } else {
                    crate::plan_cancel(ctx, &id)?
                };
                if json {
                    cx.out.json_pretty(&task)?;
                } else {
                    cx.out.line(format_args!(
                        "{id}: plan cancelled; history and execution resources retained"
                    ));
                }
            }
            Self::AddMember {
                tracker,
                member,
                json,
            } => {
                let added = crate::plan_add_member(ctx, &tracker, &member)?;
                if json {
                    cx.out.json_pretty(&added)?;
                } else {
                    cx.out.line(format_args!(
                        "{} added to {}; members: {}",
                        added.member,
                        added.tracker,
                        added.children.join(", ")
                    ));
                }
            }
            Self::MoveMember {
                tracker,
                member,
                to,
                gate_file,
                reason,
                json,
            } => {
                let gate = gate_file.as_ref().map(|file| {
                    (
                        file.path.as_str(),
                        file.text.as_str(),
                        reason.as_deref().unwrap_or_default(),
                    )
                });
                let moved =
                    crate::plan_move_member_with_gate_text(ctx, &tracker, &member, &to, gate)?;
                if json {
                    cx.out.json_pretty(&moved)?;
                } else {
                    cx.out.line(format_args!(
                        "{} moved from {} to {}; {} members: {}; {} members: {}",
                        moved.member,
                        moved.from,
                        moved.to,
                        moved.from,
                        moved.from_children.join(", "),
                        moved.to,
                        moved.to_children.join(", ")
                    ));
                }
            }
            Self::Policy { id, policy } => {
                let required = match policy.as_str() {
                    "required" => Some(true),
                    "off" => Some(false),
                    "inherit" => None,
                    _ => bail!("invalid plan policy"),
                };
                let task = crate::plan_policy_set(ctx, &id, required)?;
                cx.out.json_pretty(&task)?;
            }
            Self::Other => return Err(super::unknown_command("other")),
        }
        Ok(())
    }

    fn touches_runtime(&self) -> bool {
        matches!(self, Self::Cancel { .. })
    }
}
