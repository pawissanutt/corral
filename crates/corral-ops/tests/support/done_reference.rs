//! The pre-T-591 full-task done algorithm, retained as a differential oracle.
//! Artifact publication helpers are shared; the legacy reads and final full-task update are not.
use crate::Ctx;
use anyhow::{Context, Result, bail};
use corral_core::{ArtifactKind, RunOutcome, Status, Task};
use serde_json::json;

pub(super) fn legacy(ctx: &Ctx, id: &str, summary: &str, result: Option<&str>) -> Result<Task> {
    let task = ctx.store.get(id)?;
    if crate::plan_pilot::handles(&task) {
        bail!("plan runs finish with `corral plan submit`, not `corral done`");
    }
    if !matches!(task.status, Status::Running | Status::NeedsMe) {
        bail!(
            "task {} is {}; only Running or Needs me tasks can be marked done",
            task.id,
            task.status.label()
        );
    }
    let run = crate::review::current_run(&task)?;
    refuse(&task, &run.id)?;
    crate::delivery::owner::done_gate(ctx, &task)?;
    crate::resume::require_checkpoint(&task, &run)?;
    if run.checkpoint_required && !corral_git::is_clean(&run.worktree)? {
        bail!("commit_before_checkpoint: commit work, then save a checkpoint before done");
    }
    let audit = crate::completion::completion_check_task(ctx, &task, summary)?;
    if !audit.passed {
        bail!("completion audit failed: {}", audit.failures.join("; "));
    }
    if !audit.audited {
        corral_git::commit_all(
            &run.worktree,
            &format!("{}: {} (corral run {})", task.id, task.title, run.id),
        )?;
    }
    let base = corral_git::rev_sha(&run.worktree, &run.base)?;
    let stat = corral_git::diff_stat(&run.worktree, &base)?;
    let diff = corral_git::diff_text(&run.worktree, &base)?;
    let tmp = ctx
        .store
        .scratch_tmp()?
        .join(format!("{}-{}", task.id, run.id));
    std::fs::create_dir_all(&tmp)?;
    let diff_file = tmp.join("final.diff");
    let summary_file = tmp.join("summary.md");
    std::fs::write(&diff_file, &diff)?;
    std::fs::write(&summary_file, format!("{}\n", summary.trim_end()))?;
    let title = format!("Final diff +{} −{}", stat.added, stat.removed);
    crate::artifacts::add_artifact(ctx, id, &diff_file, Some(ArtifactKind::Diff), Some(&title))?;
    crate::artifacts::add_artifact(
        ctx,
        id,
        &summary_file,
        Some(ArtifactKind::Summary),
        Some("Done summary"),
    )?;
    std::fs::remove_dir_all(&tmp)?;
    let scanned = crate::artifacts::scan_artifacts(ctx, id)?;
    let thumbnails = crate::capture_html_thumbnails(ctx, id)?;
    let admission = crate::delivery::owner::done_admission(ctx, &task)?;
    let detail = json!({ "run": run.id, "added": stat.added, "removed": stat.removed, "files": stat.files.len(), "scanned": scanned.len(), "thumbnails": thumbnails, "completion_audited":audit.audited, "identity":audit.identity, "dispatch_base":run.base_sha, "integration_base":base });
    let task = ctx.store.update(id, |t| {
        refuse(t, &run.id)?;
        if !matches!(t.status, Status::Running | Status::NeedsMe)
            || t.current_run().is_none_or(|r| r.id != run.id)
        {
            bail!(
                "task {} changed while finishing run {} (now {}); not marking it done",
                t.id,
                run.id,
                t.status.label()
            );
        }
        crate::resume::require_checkpoint(t, t.current_run().context("missing_run")?)?;
        if audit.audited {
            let current = crate::completion::completion_check_task(ctx, t, summary)?;
            if !current.passed
                || current.identity != audit.identity
                || current.manifest_digest != audit.manifest_digest
                || current.evidence_keys != audit.evidence_keys
            {
                bail!(
                    "completion audit stale at transition: {}",
                    current.failures.join("; ")
                );
            }
        }
        if let Some(r) = t.current_run_mut() {
            crate::reviewer::supersede_review(ctx, r, corral_core::clock::now());
            r.autopilot = None;
            r.blocked_excerpt = None;
            r.idle_since = None;
            r.completion.audit = Some(audit.clone());
            r.summary = Some(summary.to_string());
            r.ended = Some(corral_core::clock::now());
            r.outcome = Some(RunOutcome::Done);
            crate::usage::collect(r, &ctx.store);
        }
        if let Some(result) = result {
            t.result = Some(corral_core::TaskResult {
                text: result.to_owned(),
                run: run.id.clone(),
                spec_digest: crate::completion::completion_spec_digest(t)?,
            });
        }
        t.status = Status::Review;
        t.queued = false;
        ctx.store.log_event(Some(&t.id), "done", detail.clone())?;
        Ok(())
    })?;
    drop(admission);
    if audit.audited {
        crate::review::audit_artifact(ctx, id, &run.id, &audit, false)?;
    }
    Ok(task)
}

fn refuse(task: &Task, run: &str) -> Result<()> {
    let container = corral_core::agent::ContainerRef::Worker {
        task: task.id.clone(),
        run: run.into(),
    };
    crate::agent::effects::refuse_pending(task, &container, "open_questions")
}

#[test]
fn done_matches_the_legacy_full_task_transition() {
    for n in [1, 10] {
        for stale in [false, true] {
            for result in [None, Some("Cumulative result")] {
                let (fx, id) = super::launch_resume::obligated(n, false);
                if stale {
                    super::launch_resume::stale(&fx, &id);
                }
                if n == 10 && !stale && result.is_some() {
                    fx.ctx
                        .store
                        .update(&id, |task| {
                            let mut older =
                                super::action_budget_growth::bare_historic(1, fx.temp.path());
                            older.id = "R0".into();
                            task.runs.insert(0, older);
                            let run = task.current_run_mut().unwrap();
                            let mut child =
                                super::action_budget_growth::bare_historic(2, fx.temp.path());
                            child.id = "R1-V1".into();
                            child.ended = None;
                            child.outcome = None;
                            child.review.as_mut().unwrap().verdict = None;
                            run.review = child.review.clone();
                            run.review_runs.push(child);
                            Ok(())
                        })
                        .unwrap();
                }
                let before = fx.ctx.store.get(&id).unwrap();
                let expected = legacy(&fx.ctx, &id, "finished the change", result)
                    .map(|t| t.status)
                    .map_err(|e| format!("{e:#}"));
                let expected_task = fx.ctx.store.get(&id).unwrap();
                fx.ctx
                    .store
                    .update(&id, |t| {
                        *t = before.clone();
                        Ok(())
                    })
                    .unwrap();
                let actual = crate::done_with_result(&fx.ctx, &id, "finished the change", result)
                    .map(|t| t.status)
                    .map_err(|e| format!("{e:#}"));
                assert_eq!(actual, expected);
                let actual_task = fx.ctx.store.get(&id).unwrap();
                assert_eq!(
                    serde_json::to_value(actual_task).unwrap(),
                    serde_json::to_value(expected_task).unwrap(),
                    "x{n} stale={stale} result={result:?}"
                );
            }
        }
    }
}
