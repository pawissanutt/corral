//! Independent aggregate review: the policy-selected reviewer is launched against one frozen
//! attempt, candidate and evidence digest, and only that review's callback can complete it.
//! A caller-supplied reviewer name is never a review.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use corral_core::{
    AggregateAttempt, AggregateReview, ArtifactKind, GateResult, HerdrRef, ResolvedProfile,
    Reviewer, Store, Task, Verdict,
};
use jiff::Timestamp;
use serde_json::json;

use super::{capture, current, evidence_failures, fresh_candidate, gate};
use crate::Ctx;
use crate::verification::digest;

const PM: &str = "pm";

/// A launched reviewer's callback, bound to its exact attempt and review.
pub struct AggregateVerdict<'a> {
    pub attempt: &'a str,
    pub review: &'a str,
    pub verdict: Verdict,
    pub notes: &'a str,
}

pub(super) fn evidence_digest(attempt: &AggregateAttempt) -> Result<String> {
    Ok(digest(&serde_json::to_vec(&attempt.evidence)?))
}

/// The reviewer the effective review policy selects for this Plan; None when review is off.
fn expected(store: &Store, tracker: &Task) -> Result<Option<(String, ResolvedProfile)>> {
    let config = &store.config;
    let mut policy = config.task_review_policy(tracker);
    if policy.reviewer == Reviewer::None {
        return Ok(None);
    }
    if policy.reviewer == Reviewer::CrossVendor {
        policy.require_cross_family = true;
    }
    let vendor = if policy.cross_family_required() {
        Some(implementer_vendor(store, tracker)?)
    } else {
        None
    };
    let (name, resolved) =
        crate::review_routing::resolve(config, &policy.reviewer, vendor.as_deref())?;
    policy.check_reviewer_family(vendor.as_deref(), resolved.vendor.as_deref())?;
    Ok(Some((name, resolved)))
}

/// Cross-vendor selection needs the one vendor that implemented every member.
fn implementer_vendor(store: &Store, tracker: &Task) -> Result<String> {
    let mut vendors = std::collections::BTreeSet::new();
    for id in &tracker.children {
        let child = store.get(id)?;
        let run = child
            .current_run()
            .with_context(|| format!("{id} has no run"))?;
        vendors.insert(match &run.vendor {
            Some(vendor) => vendor.clone(),
            None => store.config.run_vendor(run)?,
        });
    }
    ensure!(
        vendors.len() == 1,
        "cross-vendor aggregate review needs one known implementer vendor, found {vendors:?}"
    );
    vendors.pop_first().context("no implementer vendor")
}

/// The configured review receipt both acceptance origins need. The owner may override a
/// completed review's outcome but not its absence; automatic acceptance needs an independent
/// approval even when no reviewer is configured.
pub(super) fn require(
    store: &Store,
    tracker: &Task,
    attempt: &AggregateAttempt,
    automatic: bool,
) -> Result<()> {
    let checked = (|| -> Result<()> {
        let configured = expected(store, tracker)?;
        let Some((name, _)) = configured else {
            ensure!(!automatic, "no aggregate reviewer is configured");
            return Ok(());
        };
        let review = attempt
            .reviews
            .last()
            .filter(|review| review.completed())
            .with_context(|| {
                format!(
                    "aggregate review missing or pending for {}; run `corral plan aggregate-review {} --attempt {}` and wait for its verdict",
                    attempt.id, tracker.id, attempt.id
                )
            })?;
        bound(attempt, review, &name)?;
        if automatic {
            ensure!(
                review.verdict == Some(Verdict::Approve),
                "aggregate review {} did not approve",
                review.id
            );
            ensure!(
                review.reviewer != PM,
                "the PM is not an independent aggregate reviewer"
            );
        }
        Ok(())
    })();
    if automatic {
        checked.with_context(|| {
            format!(
                "automatic Plan acceptance requires an independent approving aggregate review of {}",
                attempt.id
            )
        })
    } else {
        checked
    }
}

/// A review judges exactly the frozen candidate, evidence and configured reviewer.
fn bound(attempt: &AggregateAttempt, review: &AggregateReview, reviewer: &str) -> Result<()> {
    ensure!(
        review.identity == attempt.identity,
        "aggregate review {} is stale: candidate identity changed; review again",
        review.id
    );
    ensure!(
        review.evidence_digest == evidence_digest(attempt)?,
        "aggregate review {} is stale: aggregate evidence changed after it was requested; review again",
        review.id
    );
    ensure!(
        review.reviewer == reviewer,
        "aggregate review {} was by {}, not the configured reviewer {reviewer}; review again",
        review.id,
        review.reviewer
    );
    Ok(())
}

/// Verifiable inputs shared by launch and callback: settled gate, intact evidence, fresh candidate.
fn check_inputs(
    ctx: &Ctx,
    store: &Store,
    tracker: &Task,
    attempt: &AggregateAttempt,
) -> Result<()> {
    ensure!(
        attempt
            .gate
            .as_ref()
            .is_none_or(|gate| gate.result != GateResult::Running),
        "gate_incomplete"
    );
    let failures = evidence_failures(ctx, tracker, attempt);
    ensure!(
        failures.is_empty(),
        "aggregate evidence is invalid: {}",
        failures.join("; ")
    );
    let _landing = corral_git::landing_lock(&capture::repo(store, tracker)?)?;
    fresh_candidate(ctx, tracker, attempt)
}

fn files(ctx: &Ctx, plan: &str, review: &str) -> (PathBuf, PathBuf) {
    (
        ctx.store
            .prompts_dir()
            .join(format!("{plan}-{review}-aggregate-review.md")),
        ctx.store
            .state_dir()
            .join("reviews")
            .join(format!("{plan}-{review}-aggregate-notes.md")),
    )
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn prompt(
    ctx: &Ctx,
    tracker: &Task,
    attempt: &AggregateAttempt,
    review: &AggregateReview,
    cwd: &Path,
    notes: &Path,
) -> Result<String> {
    let verdict_cmd = format!(
        "{} --root {} plan aggregate-verdict {} approve|changes|escalate --attempt {} --review {} --notes-file {}",
        quote(&crate::prompt::corral_bin()?),
        quote(ctx.store.root()),
        tracker.id,
        attempt.id,
        review.id,
        quote(notes),
    );
    let identity = &attempt.identity;
    let candidate = if attempt.code {
        let range = if identity.dispatch_base.is_empty() {
            String::new()
        } else {
            format!(
                "; the Plan branch changes are `git -C {} diff {}..{}`",
                quote(cwd),
                identity.dispatch_base,
                identity.head
            )
        };
        format!(
            "Combined candidate {} checked out in {} on main {}{range}.",
            identity.head,
            cwd.display(),
            identity.integration_base
        )
    } else {
        "No member changed code; judge the accepted artifacts.".into()
    };
    let gate = attempt.gate.as_ref().map_or_else(
        || "not required (no code)".to_string(),
        |record| {
            format!(
                "{} (log {})",
                record.result.as_str(),
                gate::paths(ctx, &tracker.id, &attempt.id).log.display()
            )
        },
    );
    let members: String = attempt
        .members
        .iter()
        .map(|member| {
            format!(
                "- {} {}: receipt {}{}\n",
                member.task,
                member.run,
                member.receipt_sha,
                if member.code { "" } else { " (no code)" }
            )
        })
        .collect();
    let mut text = format!(
        "# Aggregate Plan review: {} {}\n\nTarget: aggregate attempt {}, review {}. Judge the Plan as a \
         whole: do the combined changes and the recorded aggregate evidence satisfy every approved \
         acceptance criterion together? A member's local PASS is not aggregate proof. Do not edit \
         files, merge, accept, dispatch or create tasks.\n\n## Plan\n\n{}\n\n## Approved scope\n\n```json\n{}\n```\n\n\
         ## Accepted members\n\n{members}\n## Candidate\n\n{candidate}\nAggregate gate: {gate}\n\n\
         ## Aggregate evidence (worker assertions, not machine PASS)\n\n```json\n{}\n```\n\n\
         Write concrete notes to {} and finish with:\n\n    {verdict_cmd}\n",
        tracker.id,
        tracker.title,
        attempt.id,
        review.id,
        tracker.body.trim_end(),
        serde_json::to_string_pretty(&tracker.approved_scope)?,
        serde_json::to_string_pretty(&attempt.evidence)?,
        notes.display(),
    );
    text.push_str(&crate::comments::owner_comments(tracker));
    Ok(text)
}

/// Close a dedicated reviewer workspace, never the shared PM. Cleanup never undoes a verdict.
fn close(ctx: &Ctx, plan: &str, review: &AggregateReview) {
    let Some(herdr) = review.herdr.as_ref().filter(|_| review.reviewer != PM) else {
        return;
    };
    let closed = (|| -> Result<()> {
        if ctx.herdr.pane_get(&herdr.pane_id)?.is_some() {
            ctx.herdr.pane_close(&herdr.pane_id)?;
        }
        if ctx
            .herdr
            .workspace_list()?
            .iter()
            .any(|(id, _)| *id == herdr.workspace_id)
        {
            ctx.herdr.workspace_close(&herdr.workspace_id)?;
        }
        Ok(())
    })();
    if let Err(error) = closed {
        crate::messages::report(
            Some(plan),
            "aggregate review cleanup",
            format!("could not close reviewer {}: {error:#}", herdr.pane_id),
        );
    }
}

fn stored_review<'a>(
    task: &'a mut Task,
    attempt: &str,
    review: &str,
) -> Result<&'a mut AggregateReview> {
    task.aggregate_attempts
        .last_mut()
        .filter(|stored| stored.id == attempt)
        .and_then(|stored| stored.reviews.last_mut())
        .filter(|stored| stored.id == review)
        .with_context(|| format!("aggregate review {review} was replaced"))
}

/// Launch the policy-selected reviewer on the current attempt and its evidence. The pending
/// record is published before launch; an earlier pending review is superseded and its
/// callback refused from then on.
///
/// # Errors
/// Refuses when no reviewer is configured, the gate is running, evidence is invalid or the
/// candidate is stale, and reports launch failures after abandoning the review.
pub fn plan_aggregate_review(ctx: &Ctx, id: &str, attempt: &str) -> Result<AggregateReview> {
    let _attribution = ctx.store.attribute(corral_core::Actor::Reviewer);
    ctx.store.check_runtime()?;
    crate::session_scope::check_launch(ctx)?;
    let _ownership = ctx
        .store
        .try_lock("session-ownership")?
        .context("pane inventory is being reconciled; retry aggregate review")?;
    let store = ctx.store.reopen()?;
    store.config.validate_review()?;
    let tracker = store.get(id)?;
    let chosen = current(&tracker, attempt)?.clone();
    check_inputs(ctx, &store, &tracker, &chosen)?;
    let (name, resolved) = expected(&store, &tracker)?.with_context(|| {
        format!(
            "no aggregate reviewer is configured for {id}; set [review] reviewer or the task's reviewer"
        )
    })?;
    let repo = capture::repo(&store, &tracker)?;
    let cwd = if chosen.code {
        repo.join(".worktrees").join(format!("{id}-verify"))
    } else {
        repo
    };
    let review = AggregateReview {
        id: format!("{}-R{}", chosen.id, chosen.reviews.len() + 1),
        reviewer: name.clone(),
        identity: chosen.identity.clone(),
        evidence_digest: evidence_digest(&chosen)?,
        requested: Timestamp::now(),
        herdr: None,
        verdict: None,
        notes: None,
        at: None,
        abandoned: None,
    };
    let (prompt_file, notes) = files(ctx, id, &review.id);
    for dir in [prompt_file.parent(), notes.parent()].into_iter().flatten() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    std::fs::write(
        &prompt_file,
        prompt(ctx, &tracker, &chosen, &review, &cwd, &notes)?,
    )
    .with_context(|| format!("writing {}", prompt_file.display()))?;
    match std::fs::remove_file(&notes) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("clearing replaced aggregate review notes"),
    }
    let mut superseded = Vec::new();
    ctx.store.update(id, |task| {
        ensure!(
            current(task, &chosen.id)? == &chosen,
            "aggregate attempt changed; retry"
        );
        let stored = task
            .aggregate_attempts
            .last_mut()
            .context("aggregate attempt missing")?;
        for previous in stored.reviews.iter_mut().filter(|r| r.pending()) {
            previous.abandoned = Some(format!("superseded by {}", review.id));
            superseded.push(previous.clone());
        }
        stored.reviews.push(review.clone());
        Ok(())
    })?;
    for previous in &superseded {
        close(ctx, id, previous);
    }
    ctx.store.log_event(
        Some(id),
        "aggregate_review_requested",
        json!({"attempt": chosen.id, "review": review.id, "reviewer": name, "superseded": superseded.iter().map(|r| &r.id).collect::<Vec<_>>(), "prompt": prompt_file}),
    )?;
    let launched = launch(ctx, &tracker, &review, &resolved, &prompt_file, &cwd);
    if let Err(error) = &launched {
        let reason = format!("launch failed: {error:#}");
        let saved = ctx.store.update(id, |task| {
            if let Ok(stored) = stored_review(task, &chosen.id, &review.id)
                && stored.pending()
            {
                stored.abandoned = Some(reason.clone());
            }
            Ok(())
        })?;
        if let Some(stored) = saved
            .aggregate_attempts
            .last()
            .and_then(|attempt| attempt.reviews.last())
        {
            close(ctx, id, stored);
        }
        ctx.store.log_event(
            Some(id),
            "aggregate_review_failed",
            json!({"attempt": chosen.id, "review": review.id, "error": reason}),
        )?;
    }
    let herdr = launched?;
    ctx.store.log_event(
        Some(id),
        "aggregate_review_started",
        json!({"attempt": chosen.id, "review": review.id, "reviewer": name, "pane": herdr.pane_id}),
    )?;
    ctx.store
        .get(id)?
        .aggregate_attempts
        .iter()
        .find(|stored| stored.id == chosen.id)
        .and_then(|stored| stored.reviews.iter().find(|r| r.id == review.id))
        .cloned()
        .context("aggregate review disappeared")
}

/// Attach the pane before sending: a synchronous reviewer can call back immediately.
fn launch(
    ctx: &Ctx,
    tracker: &Task,
    review: &AggregateReview,
    resolved: &ResolvedProfile,
    prompt_file: &Path,
    cwd: &Path,
) -> Result<HerdrRef> {
    let title = format!("{} aggregate review", tracker.id);
    let herdr = if review.reviewer == PM {
        let pane_id = match crate::pm::live_pm_pane(ctx)? {
            Some(pane) => pane,
            None => crate::pm::open_pm(ctx)?,
        };
        let info = ctx
            .herdr
            .pane_get(&pane_id)?
            .context("PM pane disappeared")?;
        HerdrRef {
            workspace_id: info.workspace_id,
            pane_id,
            socket: Some(ctx.herdr.socket_path()?),
        }
    } else {
        let created = ctx.herdr.workspace_create(cwd, &title, false)?;
        HerdrRef {
            workspace_id: created.workspace_id,
            pane_id: created.pane_id,
            socket: Some(ctx.herdr.socket_path()?),
        }
    };
    let attempt = review.identity.attempt.clone();
    ctx.store.update(&tracker.id, |task| {
        let stored = stored_review(task, &attempt, &review.id)?;
        ensure!(
            stored.pending(),
            "aggregate review {} ended before launch",
            review.id
        );
        stored.herdr = Some(herdr.clone());
        Ok(())
    })?;
    if review.reviewer == PM {
        let message = format!(
            "Review aggregate attempt {attempt} of {} ({}): read the review prompt at {} and follow it. \
             Do not edit files; finish with its exact plan aggregate-verdict command, including --review {}.",
            tracker.id,
            tracker.title,
            prompt_file.display(),
            review.id
        );
        let delivery = ctx.herdr.agent_prompt(&herdr.pane_id, &message)?;
        if !delivery.submitted {
            bail!(
                "PM aggregate review prompt was not submitted: {}",
                delivery
                    .error
                    .as_deref()
                    .unwrap_or("submission was not confirmed")
            );
        }
    } else {
        ctx.herdr.pane_report_metadata(
            &herdr.pane_id,
            &title,
            &[
                ("task", tracker.id.as_str()),
                ("attempt", attempt.as_str()),
                ("review", review.id.as_str()),
                ("target", "aggregate-review"),
            ],
        )?;
        let command = crate::prompt::render_template(
            resolved,
            &[
                ("prompt_file", prompt_file.display().to_string()),
                ("name", title.clone()),
                ("task", tracker.id.clone()),
                ("worktree", cwd.display().to_string()),
            ],
        )?;
        ctx.herdr.pane_run(&herdr.pane_id, &command)?;
    }
    Ok(herdr)
}

/// Record the launched reviewer's verdict. Accepted only for the current, launched, pending
/// review of the current attempt whose candidate, evidence and configured reviewer still match.
///
/// # Errors
/// Refuses empty notes and unstarted, replaced, duplicate, stale or wrong-reviewer callbacks.
pub fn plan_aggregate_verdict(
    ctx: &Ctx,
    id: &str,
    input: &AggregateVerdict<'_>,
) -> Result<AggregateReview> {
    let _attribution = ctx.store.attribute(corral_core::Actor::Reviewer);
    ctx.store.check_runtime()?;
    ensure!(
        !input.notes.trim().is_empty(),
        "an aggregate verdict needs nonempty notes (--notes-file)"
    );
    let store = ctx.store.reopen()?;
    let tracker = store.get(id)?;
    let attempt = current(&tracker, input.attempt)
        .context("stale aggregate verdict")?
        .clone();
    let review = attempt
        .reviews
        .last()
        .filter(|review| review.id == input.review)
        .with_context(|| {
            format!(
                "stale aggregate verdict: {} is not the current review of {}",
                input.review, attempt.id
            )
        })?
        .clone();
    if let Some(reason) = &review.abandoned {
        bail!("stale aggregate verdict: {} was {reason}", review.id);
    }
    ensure!(
        review.verdict.is_none(),
        "stale aggregate verdict: {} already has a verdict",
        review.id
    );
    ensure!(
        review.herdr.is_some(),
        "stale aggregate verdict: {} was never launched",
        review.id
    );
    let configured = expected(&store, &tracker)?
        .map(|(name, _)| name)
        .context("stale aggregate verdict: no aggregate reviewer is configured")?;
    bound(&attempt, &review, &configured).context("stale aggregate verdict")?;
    check_inputs(ctx, &store, &tracker, &attempt).context("stale aggregate verdict")?;
    let tmp = ctx
        .store
        .state_dir()
        .join("tmp")
        .join(format!("{id}-{}-aggregate-review", review.id));
    std::fs::create_dir_all(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
    let file = tmp.join("aggregate-review-notes.md");
    let snapshot = std::fs::write(&file, format!("{}\n", input.notes.trim_end()))
        .with_context(|| format!("writing {}", file.display()))
        .and_then(|()| {
            crate::artifacts::add_artifact(
                ctx,
                id,
                &file,
                Some(ArtifactKind::Report),
                Some(&format!("Plan review {} ({})", review.id, review.reviewer)),
            )
        });
    if let Err(error) = std::fs::remove_dir_all(&tmp) {
        crate::messages::report(
            Some(id),
            "cleanup",
            format!("could not remove {} (ignored): {error}", tmp.display()),
        );
    }
    snapshot?;
    let saved = ctx.store.update(id, |task| {
        ensure!(
            current(task, &attempt.id).is_ok_and(|stored| stored.evidence == attempt.evidence),
            "stale aggregate verdict: attempt changed while recording"
        );
        let stored = stored_review(task, &attempt.id, &review.id)?;
        ensure!(
            *stored == review,
            "stale aggregate verdict: {} changed while recording",
            review.id
        );
        stored.verdict = Some(input.verdict);
        stored.notes = Some(input.notes.into());
        stored.at = Some(Timestamp::now());
        Ok(())
    })?;
    let recorded = saved
        .aggregate_attempts
        .last()
        .and_then(|attempt| attempt.reviews.last())
        .cloned()
        .context("aggregate review disappeared")?;
    ctx.store.log_event(
        Some(id),
        "aggregate_verdict",
        json!({"attempt": attempt.id, "review": review.id, "reviewer": review.reviewer, "verdict": input.verdict.as_str()}),
    )?;
    close(ctx, id, &recorded);
    Ok(recorded)
}
