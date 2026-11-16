//! Review actions share the CLI's real landing, revisioned verdict and disposal services.
use super::{Exec, Verb};
use anyhow::Result;
use corral_protocol::cmd::review::ReviewCommand;

impl Verb for ReviewCommand {
    fn execute(self, cx: &mut Exec<'_>) -> Result<()> {
        match self {
            Self::Approve { id, sha } => {
                let line = crate::review::approve_checked(
                    &cx.ctx,
                    &id,
                    &sha,
                    crate::ApproveStrategy::Rebase,
                    true,
                    cx.invoker,
                )?;
                cx.out.line(format_args!("{line}"));
            }
            Self::RequestChanges { id, text, escalate } => {
                if escalate {
                    super::task::dispatch_outcome(
                        &mut cx.out,
                        crate::request_changes_escalated(&cx.ctx, &id, &text)?,
                    );
                } else {
                    let task = crate::request_changes(&cx.ctx, &id, &text)?;
                    cx.out
                        .line(format_args!("{} -> {}", task.id, task.status.label()));
                }
            }
            Self::Verdict {
                id,
                sha,
                verdict,
                notes,
            } => {
                let verdict = match verdict.as_str() {
                    "approve" => corral_core::Verdict::Approve,
                    "changes" => corral_core::Verdict::Changes,
                    "escalate" => corral_core::Verdict::Escalate,
                    _ => anyhow::bail!("unknown verdict (approve, changes, escalate)"),
                };
                let record = crate::reviewer::verdict_checked(
                    &cx.ctx,
                    &id,
                    &sha,
                    verdict,
                    notes.as_deref().unwrap_or_default(),
                )?;
                cx.out.json(&record)?;
            }
            Self::Discard { id } => {
                let reason = "Owner confirmed candidate discard";
                if cx.invoker.is_app() {
                    // Exact Confirm-class proof is checked by the authenticated daemon adapter.
                    cx.invoker.device()?;
                    cx.ctx
                        .store
                        .owner_decision(&id, "drop", reason, "app-confirmation")?;
                }
                let task = crate::discard_with_decision(
                    &cx.ctx,
                    &id,
                    false,
                    Some(reason),
                    cx.invoker.decision_author(&cx.ctx.store)?,
                )?;
                cx.out.line(format_args!("{} discarded", task.id));
            }
            Self::Other => return Err(super::unknown_command("other")),
        }
        Ok(())
    }

    fn touches_runtime(&self) -> bool {
        !matches!(self, Self::Other)
    }
}
