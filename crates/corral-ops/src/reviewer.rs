//! Reviewer runs and structured verdicts: `review_run` hands the current run to an independent reviewer (a
//! profile in its own pane, or the PM agent), `verdict` records the reviewer's machine-readable judgment.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use corral_core::{
    ArtifactKind, GateResult, HerdrRef, ProfileSwitch, ResolvedProfile, ReviewRecord, Reviewer,
    Run, RunHead, RunMode, Status, Task, TaskRef, Verdict,
};
use jiff::Timestamp;
use serde_json::json;

use crate::artifacts::add_artifact;
use crate::gate::{gate_paths, open_run_pane};
use crate::pm::{live_pm_pane, open_pm};
use crate::prompt::{corral_bin, fill, render_template};
use crate::review::current_run;
use crate::{Ctx, verification};
use corral_core::TaskMut as _;

const OWNER: &str = "owner";
const PM: &str = "pm";

mod watchdog;
pub use watchdog::reconcile_review_at;
pub(crate) use watchdog::reconcile_review_sampled_at;
#[cfg(test)]
pub(crate) use watchdog::{Observation, commit_observation};
mod callback;
mod fallback;
pub(crate) use fallback::continue_review;
mod history;
mod instruction_guard;
mod stronger;
pub use stronger::{StrongerOutcome, request_stronger_review};
mod evidence;
pub(crate) use evidence::{Evidence, current_target, plan_roots};
mod record;
use record::{record_verdict, worktree_changes};
mod submission;
pub use callback::token as review_callback_token;
use history::finish_review;
pub(crate) use history::{supersede_review, supersede_review_head};
pub(crate) use submission::current_assessment_ref;

fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

mod attempt_dir;
pub use attempt_dir::review_notes_path;
use attempt_dir::{NOTES, notes_of};
pub(crate) use attempt_dir::{dir_attempt, review_dir};

mod prompt_text;
pub use prompt_text::review_prompt;
mod launch;

fn pane_alive(ctx: &Ctx, pane: &str) -> Result<bool> {
    Ok(ctx.herdr.pane_get(pane)?.is_some())
}

/// Starts a review of the current run of a Review task. `reviewer`: "pm" or a profile; default: the effective
/// review policy's reviewer. The worktree must be clean so the review belongs to HEAD.
/// - profile: a "review" tab in the run's herdr workspace (else a new workspace) on the run's worktree runs that
///   profile with the rendered review prompt (`prompts/<id>-<run>-review.md`);
/// - pm: the PM agent gets a request to follow that prompt (`agent_prompt`, falling back to send-text).
///
/// Saves an accounting child in `review_runs` and the active review before dispatch, then attaches the
/// pane and logs "review_started" only after submission. Failed PM prompts remain in the child
/// run's `pending_reply`. Known authorization refusals retain the exact prepared attempt.
/// A previous reviewer pane is closed first (never the PM pane).
pub fn review_run(ctx: &Ctx, id: &str, reviewer: Option<&str>) -> Result<ReviewRecord> {
    launch::review_run_origin(ctx, id, reviewer, crate::operating::Origin::Owner)
}

pub(crate) fn review_run_automatic(
    ctx: &Ctx,
    id: &str,
    reviewer: Option<&str>,
) -> Result<ReviewRecord> {
    launch::review_run_origin(ctx, id, reviewer, crate::operating::Origin::Automatic)
}

/// Resolve under the task lock and preserve the first successful historical vendor snapshot.
pub(crate) fn implementer_vendor<T: TaskRef + ?Sized>(
    ctx: &Ctx,
    task: &T,
    run: &RunHead,
) -> Result<String> {
    if let Some(vendor) = &run.vendor {
        return Ok(vendor.clone());
    }
    let saved = ctx.store.update_head(task.id(), |t| {
        let current = t
            .current_run_head()?
            .filter(|r| r.id == run.id)
            .ok_or_else(|| anyhow!("run {} is no longer current", run.id))?;
        let vendor = ctx.store.config.run_vendor_head(&current)?;
        t.edit_run(&run.id, |r| {
            r.vendor = Some(vendor);
            Ok(())
        })
    })?;
    let current = saved
        .current_run_head()?
        .ok_or_else(|| anyhow!("run {} is no longer current", run.id))?;
    ctx.store
        .config
        .run_vendor_head(&current)
        .map_err(Into::into)
}

fn review_parent<'a>(task: &'a mut Task, run_id: &str) -> Result<&'a mut Run> {
    task.current_run_mut()
        .filter(|r| r.id == run_id)
        .ok_or_else(|| anyhow!("run {run_id} is no longer current"))
}

/// Records a reviewer's verdict on the current run of a Review task: `review: {verdict, reviewer, notes, sha,
/// at}`, the notes as a Report artifact "Review (<reviewer>)", event "verdict". The reviewer is the one
/// `review_run` started, else "owner".
/// Refused when the worktree has uncommitted changes (a reviewer must not edit files): records verdict escalate
/// "reviewer modified the worktree" instead and returns an error. Refused without recording when HEAD moved
/// since the review started (stale review), or when `notes` is empty.
pub fn verdict(ctx: &Ctx, id: &str, verdict: Verdict, notes: &str) -> Result<ReviewRecord> {
    verdict_bound(ctx, id, verdict, notes, None)
}

/// Record a manual verdict at an explicit timestamp.
pub fn verdict_at(
    ctx: &Ctx,
    id: &str,
    verdict: Verdict,
    notes: &str,
    at: Timestamp,
) -> Result<ReviewRecord> {
    verdict_with_clock(ctx, id, verdict, notes, (None, &[]), None, || at).map(|(record, _)| record)
}

/// Reviewer callbacks must carry the attempt from their prompt. An unbound call is
/// an explicit manual verdict and is never attributed to a superseded reviewer.
pub fn verdict_bound(
    ctx: &Ctx,
    id: &str,
    verdict: Verdict,
    notes: &str,
    expected_attempt: Option<&str>,
) -> Result<ReviewRecord> {
    verdict_with_evidence(ctx, id, verdict, notes, expected_attempt, &[])
}

/// [`verdict_bound`] that also promotes `evidence` (`--evidence PATH`) inside the verdict's
/// update; a refused set (outside the attempt directory and the reviewed worktree, a symlink,
/// over `review_evidence_max`) records no verdict.
pub fn verdict_with_evidence(
    ctx: &Ctx,
    id: &str,
    verdict: Verdict,
    notes: &str,
    expected_attempt: Option<&str>,
    evidence: &[PathBuf],
) -> Result<ReviewRecord> {
    let now = corral_core::clock::now;
    verdict_with_clock(
        ctx,
        id,
        verdict,
        notes,
        (expected_attempt, evidence),
        None,
        now,
    )
    .map(|(r, _)| r)
}

/// The corral-pi `verdict` op on the bound review child: the receipt and VerdictReport commit
/// in `record_verdict`'s update. A dirty worktree's recorded escalation is the committed result.
pub fn verdict_request(
    ctx: &Ctx,
    id: &str,
    verdict: Verdict,
    notes: &str,
    expected_attempt: Option<&str>,
    report: &crate::agent::effects::Report<'_>,
) -> Result<crate::agent::effects::Receipted> {
    let home = crate::agent::effects::Home::of(report.origin);
    crate::agent::effects::swept(ctx, report.origin, home, || {
        let now = corral_core::clock::now;
        verdict_with_clock(
            ctx,
            id,
            verdict,
            notes,
            (expected_attempt, &[]),
            Some(report),
            now,
        )
        .map(|(_, receipt)| receipt)
    })
}

type Recorded = (ReviewRecord, Option<corral_core::agent::RequestReceipt>);

fn verdict_with_clock(
    ctx: &Ctx,
    id: &str,
    verdict: Verdict,
    notes: &str,
    (expected_attempt, evidence): (Option<&str>, &[PathBuf]),
    report: Option<&crate::agent::effects::Report<'_>>,
    now: impl FnOnce() -> Timestamp,
) -> Result<Recorded> {
    // An unbound verdict is an explicit manual action, not a reviewer callback.
    let _attribution = expected_attempt.map(|_| ctx.store.attribute(corral_core::Actor::Reviewer));
    ctx.store.check_runtime()?;
    let task = ctx.store.get(id)?;
    let pending = task.current_run().and_then(|run| {
        run.review.as_ref().filter(|review| {
            review.verdict.is_none()
                && expected_attempt.is_none_or(|attempt| callback::matches(review, attempt))
        })
    });
    let bound = (expected_attempt, evidence);
    let result = verdict_inner(ctx, id, verdict, notes, bound, report, now);
    if let Err(error) = &result
        && let Some(review) = pending
    {
        watchdog::record_failure(ctx, &task, review, error)?;
    }
    result
}

fn verdict_inner(
    ctx: &Ctx,
    id: &str,
    verdict: Verdict,
    notes: &str,
    (expected_attempt, evidence): (Option<&str>, &[PathBuf]),
    report: Option<&crate::agent::effects::Report<'_>>,
    now: impl FnOnce() -> Timestamp,
) -> Result<Recorded> {
    if notes.trim().is_empty() {
        bail!("a verdict needs notes (--notes or --notes-file)");
    }
    let task = ctx.store.get(id)?;
    if task.status != Status::Review {
        bail!(
            "{} is {}; verdicts are only given in Review",
            task.id,
            task.status.label()
        );
    }
    let run = current_run(&task)?;
    let run_head = RunHead::of(&run)?;
    if !run.worktree.is_dir() {
        bail!("{}: worktree {} is gone", task.id, run.worktree.display());
    }
    let head = corral_git::rev_sha(&run.worktree, "HEAD")?;
    let attempt_dir = match run.review_runs.last() {
        Some(_) => notes_of(ctx, &task.id, &run_head)?
            .parent()
            .map(Path::to_path_buf),
        None => None,
    };
    let roots: Vec<&Path> = [Some(run.worktree.as_path()), attempt_dir.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    let evidence = Evidence::check(ctx, evidence, &roots)?;
    let started = run.review.clone().filter(|r| r.verdict.is_none());
    if let Some(attempt) = expected_attempt
        && !started
            .as_ref()
            .is_some_and(|r| callback::matches(r, attempt))
    {
        let actual = started.as_ref().and_then(callback::token);
        bail!(
            "stale review: {}",
            json!([{"field":"attempt", "expected":attempt, "actual":actual}])
        );
    }
    let attempt = run
        .gate
        .as_ref()
        .and_then(|g| g.identity.as_ref())
        .map(|i| i.attempt.clone())
        .unwrap_or_else(|| format!("owner-{}", jiff::Timestamp::now().as_nanosecond()));
    let record = started.clone().unwrap_or(ReviewRecord {
        cancelled: false,
        evidence_digest: Some(verification::evidence_digest(&run_head)?),
        identity: Some(verification::review_identity(
            ctx, &task, &run_head, &attempt,
        )?),
        verdict: None,
        process_lost: false,
        reviewer: OWNER.to_string(),
        notes: None,
        sha: head.clone(),
        at: corral_core::clock::now(),
        pane: None,
        route: None,
    });

    let changes = worktree_changes(&run.worktree)?;
    if !changes.trim().is_empty() {
        let escalation = format!(
            "escalate: reviewer modified the worktree\n\nUncommitted changes in {}:\n\n```\n{}```\n\n\
             The reviewer's own verdict was {} with these notes:\n\n{}",
            run.worktree.display(),
            changes,
            verdict.as_str(),
            notes.trim_end()
        );
        let recorded = record_verdict(
            ctx,
            &task,
            &run,
            record,
            (Verdict::Escalate, escalation, &evidence),
            now(),
            report,
        )?;
        if report.is_some() {
            return Ok(recorded);
        }
        bail!(
            "{}: worktree {} has uncommitted changes; a reviewer must not edit files. Recorded verdict \
             escalate: reviewer modified the worktree",
            task.id,
            run.worktree.display()
        );
    }
    let mut moved = None;
    let (verdict, notes) = match started.as_ref() {
        Some(s) => {
            let mut changes;
            (changes, moved) = verification::verdict_changes(ctx, &task, &run_head, s)?;
            if changes.is_empty()
                && let Some(route) = s.route.as_deref()
            {
                changes = crate::review_packet::route_changes_at(
                    ctx,
                    &task,
                    &run_head,
                    s,
                    route,
                    true,
                    moved.as_ref(),
                )?;
            }
            if !changes.is_empty() {
                bail!("stale review: {}", serde_json::to_string(&changes)?);
            }
            match s.route.as_deref() {
                Some(route) => crate::review_packet::enforce(
                    ctx,
                    &task,
                    &run_head,
                    route,
                    (verdict, notes),
                    moved.as_ref(),
                )?,
                None => (verdict, notes.to_string()),
            }
        }
        None => (verdict, notes.to_string()),
    };
    let mut record = record;
    if record.identity.is_none() {
        let attempt = run
            .gate
            .as_ref()
            .and_then(|g| g.identity.as_ref())
            .map(|i| i.attempt.clone())
            .unwrap_or_else(|| format!("owner-{}", jiff::Timestamp::now().as_nanosecond()));
        record.identity = Some(verification::review_identity(
            ctx, &task, &run_head, &attempt,
        )?);
    }
    let recorded = record_verdict(
        ctx,
        &task,
        &run,
        record,
        (verdict, notes, &evidence),
        now(),
        report,
    )?;
    if let Some(base) = moved {
        ctx.store.log_event(
            Some(&task.id),
            "verdict_base_moved",
            json!({
                "run": run.id, "verdict": verdict.as_str(),
                "reviewed_base": base.expected, "current_base": base.actual,
            }),
        )?;
    }
    Ok(recorded)
}
