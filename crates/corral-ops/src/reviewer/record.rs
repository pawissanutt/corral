//! Durable verdict recording, separated from launch and callback validation.
use super::*;
use crate::agent::effects;
use anyhow::ensure;
use corral_core::inbox::{Inbox, ReceiptLevel, Recipient};

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

/// The ReviewRun inbox of the run's last review child, when that child has an agent binding:
/// only such a child can hold questions or answer messages (F4).
fn bound_child<'a>(ctx: &'a Ctx, task: &Task, run: &str) -> Result<Option<(Inbox<'a>, String)>> {
    let parent = task.runs.iter().find(|r| r.id == run);
    let child = parent.and_then(|r| r.review_runs.last());
    let Some(child) = child.filter(|c| c.agent_host.is_some()) else {
        return Ok(None);
    };
    let review = child.id.clone();
    let recipient = Recipient::ReviewRun {
        task: task.id.clone(),
        run: run.into(),
        review: review.clone(),
    };
    Ok(Some((Inbox::new(&ctx.store, recipient)?, review)))
}

/// verdict refuses while its bound review child has a pending question, or an answer or
/// correction message the reviewer has not observed (neither observed nor terminal).
fn refuse_review_child(ctx: &Ctx, task: &Task, run: &str) -> Result<()> {
    let Some((inbox, review)) = bound_child(ctx, task, run)? else {
        return Ok(());
    };
    let container = corral_core::agent::ContainerRef::Reviewer {
        task: task.id.clone(),
        run: run.into(),
        review,
    };
    effects::refuse_pending(task, &container, "review_questions_pending")?;
    let mut unobserved = Vec::new();
    for message in inbox.messages()? {
        if matches!(
            message.kind.as_str(),
            "question_answer" | "question_correction"
        ) && inbox
            .receipt(message.seq, ReceiptLevel::Observed)?
            .is_none()
            && inbox.terminal(message.seq)?.is_none()
        {
            unobserved.push(message.id);
        }
    }
    ensure!(
        unobserved.is_empty(),
        "review_answers_unobserved: {} ({}); the reviewer must observe them before a verdict",
        unobserved.len(),
        unobserved.join(", ")
    );
    Ok(())
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
    refuse_review_child(ctx, task, &run.id)?;
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
    // Inbox before task, as every inbox writer orders them: no answer can be posted or
    // observed between the recheck below and the verdict's commit.
    let child = bound_child(ctx, task, &run.id)?;
    let _answers = child.as_ref().map(|(inbox, _)| inbox.lock()).transpose()?;
    ctx.store.update(&task.id, |t| {
        if let Some(report) = report {
            let home = effects::Home::of(report.origin);
            effects::recheck(&ctx.store, t, report.origin, &home)?;
        }
        refuse_review_child(ctx, t, &run.id)?;
        let current = t.current_run().context("run disappeared")?;
        let (changes, _) = verification::verdict_changes(ctx, t, current, &record)?;
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
        // The agent effect emits the verdict event later; sync reaps after that event.
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
    // Only signal after both verdict state and event are durable. The caller and its
    // ancestors remain protected; a subsequent sync reaps any surviving reviewer.
    crate::cleanup::reap_closed_reviews(ctx, &saved);
    Ok((record, None))
}
