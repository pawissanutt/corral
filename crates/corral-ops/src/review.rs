use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use corral_core::{
    Artifact, ArtifactKind, CandidateIdentity, CoworkReview, GateRecord, GateResult, ReviewPolicy,
    Reviewer, Run, RunOutcome, Status, Task, Verdict,
};
use corral_git::{DiffStat, MergeStrategy};
use jiff::Timestamp;
use serde_json::json;

use crate::Ctx;
use crate::agent::effects;
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
    /// True when the run's owning target checkout is ready for landing.
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
    corral_git::work::record();
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
    done_with_result(ctx, id, summary, None)
}

/// Finish a run with activity/verification in `summary` and an optional cumulative task result.
/// Omitting `result` preserves the previous narrative, not its verification validity.
pub fn done_with_result(ctx: &Ctx, id: &str, summary: &str, result: Option<&str>) -> Result<Task> {
    done_inner(ctx, id, summary, result, None).map(|(task, _)| task)
}

/// The Pi-native `done` op: the receipt and AgentReport commit in done's final update.
pub fn done_request(
    ctx: &Ctx,
    id: &str,
    summary: &str,
    result: Option<&str>,
    report: &effects::Report<'_>,
) -> Result<effects::Receipted> {
    effects::swept(ctx, report.origin, effects::Home::of(report.origin), || {
        done_inner(ctx, id, summary, result, Some(report)).map(|(_, receipt)| receipt)
    })
}

fn done_inner(
    ctx: &Ctx,
    id: &str,
    summary: &str,
    result: Option<&str>,
    report: Option<&effects::Report<'_>>,
) -> Result<(Task, Option<corral_core::agent::RequestReceipt>)> {
    let task = ctx.store.get(id)?;
    if crate::plan_pilot::handles(&task) {
        bail!("plan runs finish with `corral plan submit`, not `corral done`");
    }
    if !matches!(task.status, Status::Running | Status::NeedsMe) {
        bail!(
            "task {} is {}; only Running or Blocked tasks can be marked done",
            task.id,
            task.status.label()
        );
    }
    let run = current_run(&task)?;
    refuse_open_questions(&task, &run.id)?;
    if run
        .gate
        .as_ref()
        .is_some_and(|g| g.result == GateResult::Stale)
    {
        bail!("gate_stale: rebase onto current main and run the gate once on the new base");
    }
    if let Some(report) = report {
        report.check_gate_ref(ctx, &task)?;
    }
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
    let thumbnails = crate::capture_html_thumbnails(ctx, &task.id)?;

    let admission = crate::delivery::owner::done_admission(ctx, &task)?;
    let detail = json!({ "run": run.id, "added": stat.added, "removed": stat.removed, "files": stat.files.len(), "scanned": scanned.len(), "thumbnails": thumbnails, "completion_audited":audit.audited, "identity":audit.identity, "dispatch_base":run.base_sha, "integration_base":base });
    let mut receipt = None;
    if report.is_some() {
        effects::failpoint("swept_before_commit");
    }
    let task = ctx.store.update(&task.id, |t| {
        if let Some(report) = report {
            let home = effects::Home::of(report.origin);
            effects::recheck(&ctx.store, t, report.origin, &home)?;
        }
        refuse_open_questions(t, &run.id)?;
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
            crate::reviewer::supersede_review(ctx, r, Timestamp::now());
            r.autopilot = None;
            r.blocked_excerpt = None;
            r.idle_since = None;
            r.completion.audit = Some(audit.clone());
            r.summary = Some(summary.to_string());
            r.ended = Some(Timestamp::now());
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
        if let Some(report) = report {
            let mut owed = vec!["effect:event"];
            if audit.audited {
                owed.insert(0, "effect:done_audit");
            }
            let event = effects::event("done", detail.clone(), report.origin);
            let commit = effects::Commit {
                result: json!({"run": run.id, "outcome": "review", "event": event}),
                effect: Some(("run_done", run.id.clone())),
                owed,
            };
            let kind = corral_core::agent::AgentReportKind::Done;
            receipt = Some(report.record(t, kind, None, commit)?);
        }
        Ok(())
    })?;
    drop(admission);
    if report.is_some() {
        return Ok((task, receipt));
    }
    if audit.audited {
        audit_artifact(ctx, &task.id, &run.id, &audit, false)?;
    }
    ctx.store.log_event(Some(&task.id), "done", detail)?;
    Ok((task, None))
}

fn refuse_open_questions(task: &Task, run: &str) -> Result<()> {
    let container = corral_core::agent::ContainerRef::Worker {
        task: task.id.clone(),
        run: run.into(),
    };
    effects::refuse_pending(task, &container, "open_questions")
}

pub(crate) fn audit_artifact(
    ctx: &Ctx,
    id: &str,
    run: &str,
    audit: &corral_core::CompletionAudit,
    named: bool,
) -> Result<()> {
    let file = ctx
        .store
        .state_dir()
        .join(format!("{id}-{run}-completion-audit.json"));
    std::fs::write(&file, serde_json::to_vec_pretty(audit)?)
        .with_context(|| format!("writing {}", file.display()))?;
    let (kind, title) = (Some(ArtifactKind::Evidence), Some("Completion audit"));
    if named {
        crate::artifacts::add_artifact_to_run(ctx, id, run, &file, kind, title).map(drop)
    } else {
        add_artifact(ctx, id, &file, kind, title).map(drop)
    }
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
    let main_ready = match crate::plan_target::validate_run(&ctx.store, &task, &run) {
        Ok(Some(_)) => true,
        Ok(None) => match main_repo(ctx, &task, &run) {
            Ok(repo) => {
                corral_git::current_branch(&repo).is_ok_and(|b| b == run.base)
                    && !has_tracked_changes(&repo)?
            }
            Err(_) => false,
        },
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

/// Every landing on main, and on any base that is not an owned Plan target, requires a fresh
/// receipt of `gate_tier::MAIN_LANDING_TIER` (full) by constant: no tier policy lowers it. A
/// Plan member lands on its own member binding.
pub(crate) fn landing_requirement(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
) -> Result<crate::gate_tier::GateRequirement> {
    if run.plan_target.is_some() {
        let landing = crate::gate_tier::GatePhase::Landing;
        return crate::gate_tier::requirement_for(ctx, task, run, landing);
    }
    Ok(crate::gate_tier::GateRequirement::full())
}

pub(crate) fn landing_gate_fresh(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    gate: &GateRecord,
) -> Result<bool> {
    let requirement = landing_requirement(ctx, task, run)?;
    verification::gate_fresh_for(ctx, task, run, gate, &requirement)
}

/// A fresh, passing receipt of the landing tier with a valid log.
pub(crate) fn landing_gate_passed(ctx: &Ctx, task: &Task, run: &Run) -> Result<bool> {
    match &run.gate {
        Some(gate) if gate.result == GateResult::Pass => {
            Ok(crate::gate::gate_log_passed(ctx, task, run)?
                && landing_gate_fresh(ctx, task, run, gate)?)
        }
        _ => Ok(false),
    }
}

/// A review must approve the candidate: the policy names a reviewer, or the owner finished the
/// current co-work run with review required, which binds whatever the reviewer resolves to.
fn review_needed(policy: &ReviewPolicy, task: &Task) -> bool {
    policy.reviewer != Reviewer::None
        || (task.cowork_run()).is_some_and(|cowork| cowork.review == CoworkReview::Required)
}

/// The reduced tier an approval is bound to while the current receipt is a failed full one.
fn quick_approval_on_full_fail(run: &Run) -> Option<&str> {
    let gate = run
        .gate
        .as_ref()
        .filter(|g| matches!(g.result, GateResult::Fail | GateResult::Stale))?;
    gate.identity.as_ref().filter(|i| i.gate_tier.is_none())?;
    let review = run.review.as_ref()?;
    let binding = review.identity.as_ref()?.gate_tier.as_deref()?;
    (review.verdict == Some(Verdict::Approve)).then_some(binding.tier.as_str())
}

/// Owner acceptance can override a completed check's outcome, but not its candidate identity.
/// Keep receipts as stale evidence after a rebase; retries must refresh every configured check.
fn require_manual_receipts(ctx: &Ctx, task: &Task) -> Result<()> {
    let run = task.current_run().context("run disappeared")?;
    let policy = verification::policy(ctx, task)?;
    if task.reviewer.is_some()
        || ctx
            .store
            .reopen()?
            .config
            .has_review_policy(task.project.as_deref(), task.review)
    {
        verification::require_gate(&policy, task)?;
    }
    if !policy.gate.is_empty()
        && !match &run.gate {
            Some(gate) if gate.result != GateResult::Running => {
                landing_gate_fresh(ctx, task, run, gate)?
                    && crate::gate::gate_log_valid(ctx, task, run)?
            }
            _ => false,
        }
    {
        bail!(
            "stale_retry: gate missing, pending or stale; run `corral gate {}` then `corral approve {}`",
            task.id,
            task.id
        );
    }
    if review_needed(&policy, task)
        && (!verification::review_fresh(ctx, task, run)?
            || run.review.as_ref().and_then(|r| r.verdict).is_none())
    {
        if policy.reviewer == Reviewer::None {
            bail!(
                "review_required: {0} was finished with review required, but no reviewer resolves for it; run `corral review-run {0} --reviewer <profile>`, then `corral approve {0}` after its verdict",
                task.id
            );
        }
        if let Some(tier) = quick_approval_on_full_fail(run) {
            bail!(
                "stale_retry: the approval is bound to the {tier} receipt and promotion needs a full PASS, but the full landing gate failed; run `corral review-run {}` to review the failed full receipt, then `corral approve {}` after its verdict",
                task.id,
                task.id
            );
        }
        bail!(
            "stale_retry: semantic review missing, pending or stale; run `corral review-run {}` then `corral approve {}` after its verdict",
            task.id,
            task.id
        );
    }
    Ok(())
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
    ctx.store.check_runtime()?;
    let _lock = ctx
        .store
        .try_lock(&format!("autopilot-{id}"))?
        .with_context(|| {
            format!("approval already in progress; run `corral approve {id}` after it finishes")
        })?;
    if ctx
        .store
        .get(id)?
        .current_run()
        .and_then(|r| r.gate.as_ref())
        .is_some_and(|g| g.result == GateResult::Stale)
    {
        bail!("gate_stale: rebase onto current main and run the gate once on the new base");
    }
    crate::verdict_carry::prepare_approval(ctx, id, strategy)?;
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
    ctx.store.check_runtime()?;
    // `approved` is logged after the merge; record when this approval began.
    let started = Timestamp::now();
    let task = ctx.store.get(id)?;
    let run = current_run(&task)?;
    let repo = main_repo(ctx, &task, &run)?;
    let _landing_lock = corral_git::landing_lock(&repo)?;
    crate::plan_target::recover_run(ctx, &task, &run)?;
    let task = ctx.store.get(id)?;
    let mut target = crate::plan_target::validate_run(&ctx.store, &task, &run)?;
    if let Some(landed) = target
        .as_ref()
        .and_then(|target| target.landed(id, &run.id))
    {
        return Ok(landed);
    }
    if !task.awaiting_owner() {
        bail!(
            "task {} is {}; only Review tasks can be approved",
            task.id,
            task.status.label()
        );
    }
    if run
        .gate
        .as_ref()
        .is_some_and(|g| g.result == GateResult::Stale)
    {
        bail!("gate_stale: rebase onto current main and run the gate once on the new base");
    }
    let checkout = target
        .as_ref()
        .map_or(repo.as_path(), |target| target.checkout.as_path())
        .to_path_buf();
    let authority_revision = if expected.is_some() {
        Some(ctx.store.effective_operating_policy()?.revision)
    } else {
        None
    };
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
    let mut previous_sha = None;
    // Peers under review keep going: their approval carries over a clean replay onto the new base.
    let task = ctx.store.dispatch_update(&task.id, |store, t| {
        if !t.awaiting_owner() || t.current_run().is_none_or(|r| r.id != run.id) {
            bail!("stale_retry: task or run changed before landing");
        }
        crate::plan_target::validate_run(store, t, &run)?;
        if let Some(expected) = expected {
            crate::operating::require(store, t, corral_core::OperatingActionKind::Accept)?;
            if Some(store.effective_operating_policy()?.revision) != authority_revision {
                bail!("stale_retry: landing authorization revision changed");
            }
            let current = t.current_run().context("run disappeared")?;
            let policy = verification::policy(ctx, t)?;
            if verification::identity(ctx, t, current, expected)? != *expected
                || (!policy.gate.is_empty() && !landing_gate_passed(ctx, t, current)?)
                || (review_needed(&policy, t) && (!verification::review_fresh(ctx, t, current)?
                    || current.review.as_ref().and_then(|r| r.verdict) != Some(Verdict::Approve)))
                || !crate::completion::evidence_failures(ctx, t, current)?.is_empty()
            {
                bail!("stale_retry: acceptance identity, base, policy or evidence changed before landing");
            }
        }
        if strategy == ApproveStrategy::FastForward {
            if expected.is_none() {
                require_manual_receipts(ctx, t)?;
            }
            let base_before = corral_git::rev_sha(&repo, &run.base)?;
            previous_sha = Some(base_before.clone());
            let head = corral_git::rev_sha(&run.worktree, "HEAD")?;
            if let Some(target) = &mut target {
                target.prepare_landing(t, &run, &head, "fast_forward", expected, crate::plan_target::LandingOptions { remove_worktree, keep_branch, started })?;
            } else {
                operation = crate::hooks::prepare(ctx, t, &run, &repo, &base_before, &head)?;
            }
        }
    let (sha, action) = match strategy {
        ApproveStrategy::FastForward => {
            let sha = if let Some(expected) = expected {
                corral_git::land_checked(&checkout, &run.worktree, &run.branch, &run.base,
                    &expected.head, &expected.integration_base)?
            } else { corral_git::land(
                &checkout,
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
                &checkout,
                &run.worktree,
                &run.branch,
                &run.base,
                MergeStrategy::Rebase,
                |base_before, head| {
                    previous_sha = Some(base_before.to_string());
                    // This callback runs after rebase but before main advances. The old
                    // receipts must never authorize the new HEAD, even on a later retry.
                    if let Some(expected) = expected
                        && verification::identity(ctx, t, &run, expected)? != *expected
                    {
                        bail!("stale_retry: rebase changed the accepted candidate; run a new gate and semantic review");
                    }
                    require_manual_receipts(ctx, t)?;
                    if let Some(target) = &mut target {
                        target.prepare_landing(t, &run, head, "rebase", expected, crate::plan_target::LandingOptions { remove_worktree, keep_branch, started })?;
                    } else {
                        operation = crate::hooks::prepare(ctx, t, &run, &repo, base_before, head)?;
                    }
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
        if strategy != ApproveStrategy::KeepBranch {
            t.landed = Some(corral_core::LandingReceipt {
                run: run.id.clone(),
                sha: sha.clone(),
            });
        }
        landed = Some((sha, action));
        // A legacy/concurrent change request may have reopened the run. Landing
        // owns the terminal transition: never persist Done with a working run.
        let current = t.current_run_mut().context("run disappeared before finalization")?;
        let now = Timestamp::now();
        crate::reviewer::supersede_review(ctx, current, now);
        current.ended.get_or_insert(now);
        current.outcome = Some(RunOutcome::Done);
        current.agent_state = Some("done".into());
        current.pending_reply = None;
        current.blocked_excerpt = None;
        current.idle_since = None;
        current.herdr_idle_since = None;
        current.idle_no_report = false;
        current.stuck = None;
        t.status = Status::Done;
        t.queued = false;
        Ok(())
    });
    let task = match task {
        Ok((t, ())) => t,
        Err(error) => {
            ctx.store.log_event(Some(id), "landing_stale", json!({"run":run.id, "outcome":"stale_retry", "reason":format!("{error:#}"), "expected":expected}))?;
            return Err(error);
        }
    };
    let (sha, action) = landed.context("landing transaction produced no result")?;
    if strategy != ApproveStrategy::KeepBranch
        && let Some(target) = &mut target
    {
        target.reconcile(ctx)?;
        return target
            .landed(id, &run.id)
            .context("Plan landing receipt was not published");
    }
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
        json!({ "run": run.id, "strategy": strategy_name, "sha": sha, "previous_sha":previous_sha, "base":run.base, "project":task.project, "branch": run.branch, "worktree_removed": removed, "branch_deleted": branch_deleted, "reason": reason, "acceptance_mode": if expected.is_some() {"autonomous"} else {"owner_override"}, "identity":expected, "started":started }),
    )?;
    if strategy != ApproveStrategy::KeepBranch {
        crate::assistant::scratch::record_landing(ctx, &task, &run, previous_sha.as_deref(), &sha);
    }
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
    discard_with_decision(ctx, id, keep_branch, None, "owner")
}

/// Discard with the supplied rationale and actual decision author, independently of consent.
pub fn discard_with_decision(
    ctx: &Ctx,
    id: &str,
    keep_branch: bool,
    reason: Option<&str>,
    actor: &str,
) -> Result<Task> {
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
            "branch_deleted": branch_deleted, "keep_branch": keep_branch, "warnings": warnings,
            "task_updated": task.updated, "reason": reason, "actor": actor }),
    )?;
    crate::assistant::scratch::release_discarded(ctx, &task);
    Ok(task)
}
