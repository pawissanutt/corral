use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use corral_core::{
    Accept, Artifact, ArtifactKind, CandidateIdentity, Reviewer, Run, RunOutcome, Status, Task,
    Verdict,
};
use corral_git::{DiffStat, MergeStrategy};
use jiff::Timestamp;
use serde_json::json;

use crate::Ctx;
use crate::artifacts::{add_artifact, scan_artifacts};
use crate::cleanup::Cleanup;
use crate::verification;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApproveStrategy {
    FastForward,
    Rebase,
    KeepBranch,
}

/// Everything the review screen shows.
#[derive(Debug, Clone)]
pub struct ReviewInfo {
    pub task: Task,
    pub run: Run,
    pub stat: DiffStat,
    /// Unified diff, truncated to 4000 lines with a final "... (truncated)" line.
    pub diff: String,
    pub artifacts: Vec<Artifact>,
    /// True when the main checkout is on the base branch and has no uncommitted tracked changes.
    pub main_ready: bool,
}

const DIFF_LINES: usize = 4000;

pub(crate) fn current_run(task: &Task) -> Result<Run> {
    task.current_run()
        .cloned()
        .with_context(|| format!("task {} has no run", task.id))
}

pub(crate) fn main_repo(ctx: &Ctx, task: &Task, run: &Run) -> Result<PathBuf> {
    if let Some(project) = &task.project {
        return ctx.store.project_path(project);
    }
    let parent = run.worktree.parent();
    match parent.and_then(|p| p.file_name().map(|n| (p, n))) {
        Some((p, n)) if n == ".worktrees" => p.parent().map(Path::to_path_buf),
        _ => None,
    }
    .with_context(|| {
        format!(
            "task {} has no project and its worktree is not under <repo>/.worktrees",
            task.id
        )
    })
}

pub(crate) fn has_tracked_changes(repo: &Path) -> Result<bool> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["status", "--porcelain", "--untracked-files=no"])
        .output()
        .with_context(|| format!("running git status in {}", repo.display()))?;
    if !out.status.success() {
        bail!(
            "git -C {} status: {}",
            repo.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(!out.stdout.iter().all(u8::is_ascii_whitespace))
}

/// Called by the task agent (or the user): from Running/NeedsMe -> Review.
/// 1. commit_all(worktree, "<id>: <title> (corral run <run>)") so uncommitted work is not lost;
/// 2. snapshot the diff (base_sha..worktree) as a Diff artifact "Final diff +A −R" and the summary as a
///    Summary artifact (text file) via add_artifact;
/// 3. scan_artifacts for the run;
/// 4. set run.summary, run.ended, run.outcome=Done, status Review; log "done".
pub fn done(ctx: &Ctx, id: &str, summary: &str) -> Result<Task> {
    let task = ctx.store.get(id)?;
    if !matches!(task.status, Status::Running | Status::NeedsMe) {
        bail!(
            "task {} is {}; only Running or Needs me tasks can be marked done",
            task.id,
            task.status.label()
        );
    }
    let run = current_run(&task)?;
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
        .state_dir()
        .join("tmp")
        .join(format!("{}-{}", task.id, run.id));
    std::fs::create_dir_all(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    let diff_file = tmp.join("final.diff");
    let summary_file = tmp.join("summary.md");
    std::fs::write(&diff_file, &diff)
        .with_context(|| format!("writing {}", diff_file.display()))?;
    std::fs::write(&summary_file, format!("{}\n", summary.trim_end()))
        .with_context(|| format!("writing {}", summary_file.display()))?;
    let title = format!("Final diff +{} \u{2212}{}", stat.added, stat.removed);
    let snapshots = add_artifact(
        ctx,
        &task.id,
        &diff_file,
        Some(ArtifactKind::Diff),
        Some(&title),
    )
    .and_then(|_| {
        add_artifact(
            ctx,
            &task.id,
            &summary_file,
            Some(ArtifactKind::Summary),
            Some("Done summary"),
        )
    });
    if let Err(e) = std::fs::remove_dir_all(&tmp) {
        crate::messages::report(
            Some(&task.id),
            "cleanup",
            format!("could not remove {} (ignored): {e}", tmp.display()),
        );
    }
    snapshots?;
    let scanned = scan_artifacts(ctx, &task.id)?;

    let task = ctx.store.update(&task.id, |t| {
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
            r.completion.audit = Some(audit.clone());
            r.summary = Some(summary.to_string());
            r.ended = Some(Timestamp::now());
            r.outcome = Some(RunOutcome::Done);
            crate::usage::collect(r, &ctx.store);
        }
        t.status = Status::Review;
        t.queued = false;
        Ok(())
    })?;
    if audit.audited {
        let file = ctx
            .store
            .state_dir()
            .join(format!("{}-{}-completion-audit.json", task.id, run.id));
        std::fs::write(&file, serde_json::to_vec_pretty(&audit)?)
            .with_context(|| format!("writing {}", file.display()))?;
        add_artifact(
            ctx,
            &task.id,
            &file,
            Some(ArtifactKind::Evidence),
            Some("Completion audit"),
        )?;
    }
    ctx.store.log_event(
        Some(&task.id),
        "done",
        json!({ "run": run.id, "added": stat.added, "removed": stat.removed, "files": stat.files.len(), "scanned": scanned.len(), "completion_audited":audit.audited, "identity":audit.identity, "dispatch_base":run.base_sha, "integration_base":base }),
    )?;
    Ok(task)
}

fn truncate_lines(text: String, max: usize) -> String {
    if text.lines().count() <= max {
        return text;
    }
    let mut out: String = text.lines().take(max).flat_map(|l| [l, "\n"]).collect();
    out.push_str("... (truncated)\n");
    out
}

/// Review data for the current run (error unless the task has a run).
pub fn review_info(ctx: &Ctx, id: &str) -> Result<ReviewInfo> {
    let task = ctx.store.get(id)?;
    let run = current_run(&task)?;
    let artifacts: Vec<Artifact> = task
        .artifacts
        .iter()
        .filter(|a| a.run == run.id)
        .cloned()
        .collect();
    let (stat, diff) = if run.worktree.exists() {
        let base = corral_git::rev_sha(&run.worktree, &run.base)?;
        (
            corral_git::diff_stat(&run.worktree, &base)?,
            corral_git::diff_text(&run.worktree, &base)?,
        )
    } else {
        let snapshot = artifacts
            .iter()
            .rev()
            .find(|a| a.kind == ArtifactKind::Diff)
            .and_then(|a| std::fs::read_to_string(ctx.store.dir().join(&a.path)).ok())
            .unwrap_or_default();
        (DiffStat::from_diff(&snapshot), snapshot)
    };
    let main_ready = match main_repo(ctx, &task, &run) {
        Ok(repo) => {
            corral_git::current_branch(&repo).is_ok_and(|b| b == run.base)
                && !has_tracked_changes(&repo)?
        }
        Err(_) => false,
    };
    Ok(ReviewInfo {
        diff: truncate_lines(diff, DIFF_LINES),
        task,
        run,
        stat,
        artifacts,
        main_ready,
    })
}

fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

/// From Review, or Needs me after an autopilot escalation. FastForward/Rebase: `corral_git::land(project repo,
/// worktree, branch, base, ...)`; then, when `remove_worktree`, `worktree_remove`. Always releases the task's
/// owned herdr panes and empty workspaces. KeepBranch: no git changes, optional worktree removal. Status Done, logs
/// "approved" with the new sha. Deletes the merged branch after successful worktree removal unless `keep_branch`
/// is set. The review policy can disable worktree removal. Branch deletion failures are warnings only.
/// Returns a one-line human summary ("rebased corral/T-042 onto main at 1a2b3c4; worktree removed; branch deleted").
pub fn approve(
    ctx: &Ctx,
    id: &str,
    strategy: ApproveStrategy,
    remove_worktree: bool,
    keep_branch: bool,
) -> Result<String> {
    approve_landed(ctx, id, strategy, remove_worktree, keep_branch, None).map(|(_, line)| line)
}

/// `approve`, also returning the sha the base (or kept branch) ended at.
pub(crate) fn approve_landed(
    ctx: &Ctx,
    id: &str,
    strategy: ApproveStrategy,
    remove_worktree: bool,
    keep_branch: bool,
    expected: Option<&CandidateIdentity>,
) -> Result<(String, String)> {
    let task = ctx.store.get(id)?;
    if !task.awaiting_owner() {
        bail!(
            "task {} is {}; only Review tasks can be approved",
            task.id,
            task.status.label()
        );
    }
    let run = current_run(&task)?;
    let repo = main_repo(ctx, &task, &run)?;
    let _landing_lock = corral_git::landing_lock(&repo)?;
    if strategy != ApproveStrategy::KeepBranch {
        crate::hooks::reconcile_locked(ctx, &repo)?;
        if let Some(project) = task.project.as_deref()
            && let Some(reason) = crate::hooks::operational_blocker(ctx, project)?
        {
            bail!("{reason}; resolve post-land operations before advancing the base");
        }
    }
    let remove_worktree = remove_worktree
        && ctx
            .store
            .config
            .review_policy(task.project.as_deref(), task.review)
            .remove_worktree;
    let mut landed = None;
    let mut operation = None;
    let task = ctx.store.update(&task.id, |t| {
        if !t.awaiting_owner() || t.current_run().is_none_or(|r| r.id != run.id) {
            bail!("stale_retry: task or run changed before landing");
        }
        if let Some(expected) = expected {
            let current = t.current_run().context("run disappeared")?;
            let policy = verification::policy(ctx, t)?;
            if !ctx.store.config.autopilot_enabled()
                || policy.accept != Accept::Auto
                || verification::identity(ctx, t, current, &expected.attempt)? != *expected
                || (!policy.gate.is_empty() && !verification::gate_passed(ctx, t, current)?)
                || (policy.reviewer != Reviewer::None && (!verification::review_fresh(ctx, t, current)?
                    || current.review.as_ref().and_then(|r| r.verdict) != Some(Verdict::Approve)))
                || !crate::completion::evidence_failures(ctx, t, current)?.is_empty()
            {
                bail!("stale_retry: acceptance identity, base, policy or evidence changed before landing");
            }
        }
        if strategy == ApproveStrategy::FastForward {
            let base_before = corral_git::rev_sha(&repo, &run.base)?;
            let head = corral_git::rev_sha(&run.worktree, "HEAD")?;
            operation = crate::hooks::prepare(ctx, t, &run, &repo, &base_before, &head)?;
        }
    let (sha, action) = match strategy {
        ApproveStrategy::FastForward => {
            let sha = if let Some(expected) = expected {
                corral_git::land_checked(&repo, &run.worktree, &run.branch, &run.base,
                    &expected.head, &expected.integration_base)?
            } else { corral_git::land(
                &repo,
                &run.worktree,
                &run.branch,
                &run.base,
                MergeStrategy::FastForward,
            )? };
            let line = format!(
                "fast-forwarded {} to {} at {}",
                run.base,
                run.branch,
                short(&sha)
            );
            (sha, line)
        }
        ApproveStrategy::Rebase => {
            let sha = corral_git::land_prepared(
                &repo,
                &run.worktree,
                &run.branch,
                &run.base,
                MergeStrategy::Rebase,
                |base_before, head| {
                    operation = crate::hooks::prepare(ctx, t, &run, &repo, base_before, head)?;
                    Ok(())
                },
            )?;
            let line = format!(
                "rebased {} onto {} at {}",
                run.branch,
                run.base,
                short(&sha)
            );
            (sha, line)
        }
        ApproveStrategy::KeepBranch => {
            let sha = corral_git::rev_sha(&repo, &run.branch)?;
            let line = format!("kept branch {} at {} (not merged)", run.branch, short(&sha));
            (sha, line)
        }
    };
        landed = Some((sha, action));
        t.status = Status::Done;
        t.queued = false;
        Ok(())
    });
    let task = match task {
        Ok(t) => t,
        Err(error) => {
            ctx.store.log_event(Some(id), "landing_stale", json!({"run":run.id, "outcome":"stale_retry", "reason":format!("{error:#}"), "expected":expected}))?;
            return Err(error);
        }
    };
    let (sha, action) = landed.context("landing transaction produced no result")?;
    let operational_note = if let Some(op) = &mut operation {
        if let Err(error) = crate::hooks::complete_landing(ctx, op) {
            // Git advancement and Done are already durable. Never report this as a failed merge.
            crate::messages::report(
                Some(id),
                "post-land",
                format!("landed; operation recovery needed: {error:#}"),
            );
            format!("; landed, operational readiness pending recovery: {error:#}")
        } else {
            format!(
                "; {}",
                op.blocker()
                    .unwrap_or_else(|| "operational readiness succeeded".into())
            )
        }
    } else {
        String::new()
    };
    let warnings = crate::cleanup::cleanup_report(ctx, &task, Cleanup::Task);
    let mut worktree_note = "kept".to_string();
    if remove_worktree && warnings.is_empty() {
        match crate::cleanup_worktree::remove(ctx, &task, &repo, &run, false) {
            Ok(()) => worktree_note = "removed".to_string(),
            Err(e) => worktree_note = format!("kept (warning: remove failed: {e:#})"),
        }
    } else if !warnings.is_empty() {
        worktree_note = format!("kept (warning: {})", warnings.join("; "));
    }
    let removed = worktree_note == "removed";
    let (branch_deleted, reason) = if strategy == ApproveStrategy::KeepBranch {
        (false, Some("branch not merged".to_string()))
    } else if keep_branch {
        (false, Some("--keep-branch".to_string()))
    } else if !removed {
        (false, Some(format!("worktree {worktree_note}")))
    } else {
        match corral_git::delete_branch(&repo, &run.branch, &run.base) {
            Ok(()) => (true, None),
            Err(e) => (
                false,
                Some(format!("warning: could not delete {}: {e:#}", run.branch)),
            ),
        }
    };
    let branch_note = match &reason {
        Some(reason) => format!("branch kept ({reason})"),
        None => "branch deleted".to_string(),
    };
    let strategy_name = match strategy {
        ApproveStrategy::FastForward => "fast_forward",
        ApproveStrategy::Rebase => "rebase",
        ApproveStrategy::KeepBranch => "keep_branch",
    };
    ctx.store.log_event(
        Some(&task.id),
        "approved",
        json!({ "run": run.id, "strategy": strategy_name, "sha": sha, "branch": run.branch, "worktree_removed": removed, "branch_deleted": branch_deleted, "reason": reason, "acceptance_mode": if expected.is_some() {"autonomous"} else {"owner_override"}, "identity":expected }),
    )?;
    Ok((
        sha,
        format!("{action}; worktree {worktree_note}; {branch_note}{operational_note}"),
    ))
}

/// Discard the current run and remove its worktree and branch. Artifacts remain available.
pub fn discard(ctx: &Ctx, id: &str) -> Result<Task> {
    discard_with_options(ctx, id, false)
}

/// `keep_branch` retains the discarded branch, but still removes the worktree.
pub fn discard_with_options(ctx: &Ctx, id: &str, keep_branch: bool) -> Result<Task> {
    let task = ctx.store.update(id, |t| {
        if let Some(r) = t.current_run_mut() {
            r.outcome = Some(RunOutcome::Discarded);
            r.ended.get_or_insert_with(Timestamp::now);
        }
        t.status = Status::Dropped;
        t.queued = false;
        Ok(())
    })?;
    let mut warnings = crate::cleanup::cleanup_report(ctx, &task, Cleanup::Task);
    let mut removed = false;
    let mut branch_deleted = false;
    if let Some(run) = task.current_run() {
        let result = (|| -> Result<()> {
            let repo = main_repo(ctx, &task, run)?;
            if !warnings.is_empty() {
                bail!("panes could not be closed");
            }
            crate::cleanup_worktree::remove(ctx, &task, &repo, run, true)?;
            removed = true;
            if !keep_branch {
                crate::cleanup_worktree::discard_branch(&repo, run)?;
                branch_deleted = true;
            }
            Ok(())
        })();
        if let Err(error) = result {
            warnings.push(format!("{error:#}"));
        }
    }
    for warning in &warnings {
        crate::messages::report(
            Some(&task.id),
            "discard",
            format!("warning: discard {}: {warning}", task.id),
        );
    }
    ctx.store.log_event(
        Some(&task.id),
        "discarded",
        json!({ "run": task.current_run().map(|r| r.id.clone()), "worktree_removed": removed,
            "branch_deleted": branch_deleted, "keep_branch": keep_branch, "warnings": warnings }),
    )?;
    Ok(task)
}
