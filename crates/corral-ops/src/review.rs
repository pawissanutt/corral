use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use corral_core::{Artifact, ArtifactKind, Run, RunOutcome, Status, Task};
use corral_git::{DiffStat, MergeStrategy};
use jiff::Timestamp;
use serde_json::json;

use crate::Ctx;
use crate::artifacts::{add_artifact, scan_artifacts};

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

fn current_run(task: &Task) -> Result<Run> {
    task.current_run()
        .cloned()
        .with_context(|| format!("task {} has no run", task.id))
}

fn main_repo(ctx: &Ctx, task: &Task, run: &Run) -> Result<PathBuf> {
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

fn close_pane(ctx: &Ctx, run: &Run) {
    if let Some(h) = &run.herdr
        && let Err(e) = ctx.herdr.pane_close(&h.pane_id)
    {
        eprintln!(
            "corral: could not close herdr pane {} (ignored): {e:#}",
            h.pane_id
        );
    }
}

fn has_tracked_changes(repo: &Path) -> Result<bool> {
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
    corral_git::commit_all(
        &run.worktree,
        &format!("{}: {} (corral run {})", task.id, task.title, run.id),
    )?;
    let stat = corral_git::diff_stat(&run.worktree, &run.base_sha)?;
    let diff = corral_git::diff_text(&run.worktree, &run.base_sha)?;

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
        eprintln!("corral: could not remove {} (ignored): {e}", tmp.display());
    }
    snapshots?;
    let scanned = scan_artifacts(ctx, &task.id)?;

    let task = ctx.store.update(&task.id, |t| {
        if let Some(r) = t.current_run_mut() {
            r.summary = Some(summary.to_string());
            r.ended = Some(Timestamp::now());
            r.outcome = Some(RunOutcome::Done);
        }
        t.status = Status::Review;
        t.queued = false;
        Ok(())
    })?;
    ctx.store.log_event(
        Some(&task.id),
        "done",
        json!({ "run": run.id, "added": stat.added, "removed": stat.removed, "files": stat.files.len(), "scanned": scanned.len() }),
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
        (
            corral_git::diff_stat(&run.worktree, &run.base_sha)?,
            corral_git::diff_text(&run.worktree, &run.base_sha)?,
        )
    } else {
        let snapshot = artifacts
            .iter()
            .rev()
            .find(|a| a.kind == ArtifactKind::Diff)
            .and_then(|a| std::fs::read_to_string(ctx.store.dir().join(&a.path)).ok())
            .unwrap_or_default();
        (DiffStat::default(), snapshot)
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

/// From Review only. FastForward/Rebase: `corral_git::land(project repo, worktree, branch, base, ...)`; then,
/// when `remove_worktree`, `worktree_remove` and close the run's herdr pane/workspace if it still exists.
/// KeepBranch: no git changes, optional worktree removal. Status Done, logs "approved" with the new sha.
/// Returns a one-line human summary ("rebased corral/T-042 onto main at 1a2b3c4; worktree removed").
pub fn approve(
    ctx: &Ctx,
    id: &str,
    strategy: ApproveStrategy,
    remove_worktree: bool,
) -> Result<String> {
    let task = ctx.store.get(id)?;
    if task.status != Status::Review {
        bail!(
            "task {} is {}; only Review tasks can be approved",
            task.id,
            task.status.label()
        );
    }
    let run = current_run(&task)?;
    let repo = main_repo(ctx, &task, &run)?;
    let (sha, action) = match strategy {
        ApproveStrategy::FastForward => {
            let sha = corral_git::land(
                &repo,
                &run.worktree,
                &run.branch,
                &run.base,
                MergeStrategy::FastForward,
            )?;
            let line = format!(
                "fast-forwarded {} to {} at {}",
                run.base,
                run.branch,
                short(&sha)
            );
            (sha, line)
        }
        ApproveStrategy::Rebase => {
            let sha = corral_git::land(
                &repo,
                &run.worktree,
                &run.branch,
                &run.base,
                MergeStrategy::Rebase,
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
    let mut worktree_note = "kept".to_string();
    if remove_worktree && run.worktree.exists() {
        match corral_git::worktree_remove(&repo, &run.worktree, false) {
            Ok(()) => worktree_note = "removed".to_string(),
            Err(e) => worktree_note = format!("kept (remove failed: {e:#})"),
        }
    }
    let removed = worktree_note == "removed";
    if remove_worktree {
        close_pane(ctx, &run);
    }
    let task = ctx.store.update(&task.id, |t| {
        t.status = Status::Done;
        t.queued = false;
        Ok(())
    })?;
    let strategy_name = match strategy {
        ApproveStrategy::FastForward => "fast_forward",
        ApproveStrategy::Rebase => "rebase",
        ApproveStrategy::KeepBranch => "keep_branch",
    };
    ctx.store.log_event(
        Some(&task.id),
        "approved",
        json!({ "run": run.id, "strategy": strategy_name, "sha": sha, "branch": run.branch, "worktree_removed": removed }),
    )?;
    Ok(format!("{action}; worktree {worktree_note}"))
}

/// Marks the current run Discarded, closes its pane if alive, keeps branch/worktree/artifacts,
/// status Dropped, logs "discarded".
pub fn discard(ctx: &Ctx, id: &str) -> Result<Task> {
    let task = ctx.store.update(id, |t| {
        if let Some(r) = t.current_run_mut() {
            r.outcome = Some(RunOutcome::Discarded);
            r.ended.get_or_insert_with(Timestamp::now);
        }
        t.status = Status::Dropped;
        t.queued = false;
        Ok(())
    })?;
    if let Some(run) = task.current_run() {
        close_pane(ctx, run);
    }
    ctx.store.log_event(
        Some(&task.id),
        "discarded",
        json!({ "run": task.current_run().map(|r| r.id.clone()) }),
    )?;
    Ok(task)
}
