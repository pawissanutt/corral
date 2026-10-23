//! Durable verdict recording, separated from launch and callback validation.
use super::*;
use crate::agent::effects;

pub(super) fn worktree_changes(worktree: &Path) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["status", "--porcelain"])
        .output()
        .with_context(|| format!("running git status in {}", worktree.display()))?;
    if !out.status.success() {
        bail!(
            "git -C {} status: {}",
            worktree.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Only the run's last review child with an agent binding can hold questions.
fn refuse_review_questions(task: &Task, run: &str) -> Result<()> {
    let parent = task.runs.iter().find(|r| r.id == run);
    let child = parent.and_then(|r| r.review_runs.last());
    let Some(child) = child.filter(|c| c.agent_host.is_some()) else {
        return Ok(());
    };
    let container = corral_core::agent::ContainerRef::Reviewer {
        task: task.id.clone(),
        run: run.into(),
        review: child.id.clone(),
    };
    effects::refuse_pending(task, &container, "review_questions_pending")
}

pub(super) fn record_verdict(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    mut record: ReviewRecord,
    (verdict, notes): (Verdict, String),
    ended: Timestamp,
    report: Option<&effects::Report<'_>>,
) -> Result<Recorded> {
    refuse_review_questions(task, &run.id)?;
    if let Some(report) = report {
        report.check_gate_ref(ctx, task)?;
    }
    let tmp = ctx
        .store
        .state_dir()
        .join("tmp")
        .join(format!("{}-{}-review", task.id, run.id));
    std::fs::create_dir_all(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    let file = tmp.join("review-notes.md");
    let snapshot = std::fs::write(&file, format!("{}\n", notes.trim_end()))
        .with_context(|| format!("writing {}", file.display()))
        .and_then(|()| {
            let title = format!("Review ({})", record.reviewer);
            add_artifact(
                ctx,
                &task.id,
                &file,
                Some(ArtifactKind::Report),
                Some(&title),
            )
        });
    if let Err(e) = std::fs::remove_dir_all(&tmp) {
        crate::messages::report(
            Some(&task.id),
            "cleanup",
            format!("could not remove {} (ignored): {e}", tmp.display()),
        );
    }
    snapshot?;
    record.verdict = Some(verdict);
    record.notes = Some(notes);
    record.at = ended;
    let detail = json!({ "run": run.id, "verdict": verdict.as_str(), "reviewer": record.reviewer, "sha": record.sha });
    let mut receipt = None;
    if report.is_some() {
        effects::failpoint("swept_before_commit");
    }
    ctx.store.update(&task.id, |t| {
        if let Some(report) = report {
            let home = effects::Home::of(report.origin);
            effects::recheck(&ctx.store, t, report.origin, &home)?;
        }
        refuse_review_questions(t, &run.id)?;
        let current = t.current_run().context("run disappeared")?;
        let changes = verification::review_changes(ctx, t, current, &record)?;
        if !changes.is_empty() {
            bail!("stale review: {}", serde_json::to_string(&changes)?);
        }
        let r = review_parent(t, &run.id)?;
        // Dispatch may attach the pane while a fast review is already recording its verdict.
        if r.review
            .as_ref()
            .map(|r| (r.at, r.verdict, &r.reviewer, &r.sha))
            != run
                .review
                .as_ref()
                .map(|r| (r.at, r.verdict, &r.reviewer, &r.sha))
        {
            bail!("{}: review changed while recording verdict", task.id);
        }
        if let Some(current) = &r.review {
            record.pane = current.pane.clone();
        }
        finish_review(ctx, r, ended);
        if record.verdict == Some(Verdict::Changes)
            && r.review.as_ref().is_none_or(|previous| {
                previous.verdict != Some(Verdict::Changes) || previous.sha != record.sha
            })
        {
            r.failures = r.failures.saturating_add(1);
        }
        if let Some(child) = r.review_runs.last_mut()
            && run.review.as_ref().is_some_and(|r| r.verdict.is_none())
        {
            child.review = Some(record.clone());
        }
        r.review = Some(record.clone());
        if let Some(report) = report {
            let commit = effects::Commit {
                result: json!({
                    "verdict": verdict.as_str(),
                    "event": effects::event("verdict", detail.clone(), report.origin),
                }),
                effect: Some(("verdict", run.id.clone())),
                owed: vec!["effect:capture_review", "effect:event"],
            };
            let kind = corral_core::agent::AgentReportKind::Verdict;
            receipt = Some(report.record(t, kind, Some(verdict), commit)?);
        }
        Ok(())
    })?;
    if report.is_some() {
        return Ok((record, receipt));
    }
    let saved = ctx.store.get(&task.id)?;
    if let Some(run) = saved.current_run() {
        crate::watch::capture_review(ctx, &saved, run);
    }
    ctx.store.log_event(
        Some(&task.id),
        "verdict",
        json!({ "run": run.id, "verdict": verdict.as_str(), "reviewer": record.reviewer, "sha": record.sha, "notes": record.notes }),
    )?;
    Ok((record, None))
}
