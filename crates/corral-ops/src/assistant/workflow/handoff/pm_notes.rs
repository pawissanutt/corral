//! Retain the stopped isolated review's actual notes, without inventing a verdict.
use anyhow::{Context, Result, ensure};
use corral_core::{
    Task,
    agent::{AgentRole, ReceiptState},
};

use crate::Ctx;

/// Called only after verified host and retained-effect settlement, before cancellation.
/// R0 artifacts are content evidence, never another owner/controller or an approval.
pub(super) fn capture(ctx: &Ctx, id: &str) -> Result<()> {
    let task = ctx.store.get(id)?;
    ensure!(task.dispatch_hold, "c3_pm_notes: admission is not held");
    let cap = usize::try_from(ctx.store.config_layers()?.config()?.artifacts.max_bytes)?;
    let _storage = crate::artifact_storage::storage_lock(ctx)?;
    for plan in &task.plans {
        for review in &plan.reviews {
            if review.reviewer != "pm" || !crate::plan_review::context::drain_supported(review) {
                continue;
            }
            let host = review
                .agent_host
                .as_ref()
                .context("c3_pm_notes: native binding missing")?;
            ensure!(
                host.binding.role == AgentRole::PlanReviewer,
                "c3_pm_notes: wrong role"
            );
            let dir = crate::scratch_reclaim::Attempt::PlanReview {
                task: id,
                plan: &plan.id,
                review: &review.id,
            }
            .dir(ctx)?;
            let path = dir.join("notes.md");
            let stable = corral_core::blob_ref::BlobHash::of(
                format!(
                    "{id}/{}/{}/g{}",
                    plan.id, review.id, host.binding.generation
                )
                .as_bytes(),
            )
            .to_string();
            let label = "isolated-pm-review-notes-checkpoint-not-a-verdict";
            let source = format!("assistant-handoff://{stable}/{label}");
            let last_write = host.receipts.iter().rev().find(|r| {
                r.op == "plan_write"
                    && r.state == ReceiptState::Committed
                    && r.identity.generation == host.binding.generation
            });
            let exists = match path.symlink_metadata() {
                Ok(meta) => {
                    ensure!(meta.is_file(), "c3_pm_notes: notes are not a regular file");
                    true
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => return Err(error).context("c3_pm_notes: inspect notes"),
            };
            if !exists {
                let archived = task.artifacts.iter().find(|a| {
                    a.source == source
                        && a.purged_at.is_none()
                        && last_write
                            .is_none_or(|r| r.result["sha256"].as_str() == Some(a.sha256.as_str()))
                });
                if let Some(archived) = archived {
                    crate::artifact_storage::artifact_read_confined(ctx, archived)?;
                }
                ensure!(
                    last_write.is_none() || archived.is_some(),
                    "c3_pm_notes_missing: written review notes have neither their file nor an exact archive"
                );
                continue;
            }
            ensure!(
                dir.symlink_metadata()?.is_dir(),
                "c3_pm_notes: output is not a retained directory"
            );
            let guard = corral_core::fs_guard::DirHandle::open(&dir)?;
            let (bytes, _) = super::content::regular(&path, cap.min(super::content::FILE_MAX))?;
            guard.revalidate()?;
            ctx.store.update(id, |current: &mut Task| {
                ensure!(current.dispatch_hold, "c3_pm_notes: admission reopened");
                let current_review = current
                    .plans
                    .iter()
                    .find(|p| p.id == plan.id)
                    .and_then(|p| p.reviews.iter().find(|r| r.id == review.id))
                    .context("c3_pm_notes: exact review disappeared")?;
                ensure!(
                    current_review.context == review.context
                        && current_review
                            .agent_host
                            .as_ref()
                            .is_some_and(|h| h.binding.generation == host.binding.generation
                                && h.current_generation().and_then(|g| g.bound.as_ref())
                                    == host.current_generation().and_then(|g| g.bound.as_ref())),
                    "c3_pm_notes: review context changed during capture"
                );
                guard.revalidate()?;
                ensure!(
                    super::content::regular(&path, cap.min(super::content::FILE_MAX))?.0 == bytes,
                    "c3_pm_notes: notes changed after settlement"
                );
                super::attachments::attach(ctx, current, &stable, label, &bytes)?;
                Ok(())
            })?;
        }
    }
    Ok(())
}
