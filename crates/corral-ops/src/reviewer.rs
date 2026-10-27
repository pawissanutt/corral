//! Reviewer runs and structured verdicts: `review_run` hands the current run to an independent reviewer (a
//! profile in its own pane, or the PM agent), `verdict` records the reviewer's machine-readable judgment.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use corral_core::{
    ArtifactKind, GateResult, HerdrRef, ProfileSwitch, ResolvedProfile, ReviewRecord, Reviewer,
    Run, RunMode, Status, Task, Verdict,
};
use jiff::Timestamp;
use serde_json::json;

use crate::artifacts::add_artifact;
use crate::gate::{gate_paths, open_run_pane};
use crate::pm::{live_pm_pane, open_pm};
use crate::prompt::{corral_bin, fill, render_template};
use crate::review::current_run;
use crate::{Ctx, verification};

const OWNER: &str = "owner";
const PM: &str = "pm";

mod watchdog;
pub use watchdog::reconcile_review_at;
pub(crate) use watchdog::reconcile_review_sampled_at;
mod callback;
mod fallback;
pub(crate) use fallback::continue_review;
mod history;
mod instruction_guard;
mod stronger;
pub use stronger::{StrongerOutcome, request_stronger_review};
mod record;
use record::{record_verdict, worktree_changes};
mod submission;
pub use callback::token as review_callback_token;
use history::finish_review;
pub(crate) use history::supersede_review;
pub(crate) use submission::current_assessment;

fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

/// Where a reviewer writes its notes: outside the worktree, so writing them never dirties it.
pub fn review_notes_path(ctx: &Ctx, task: &str, run: &str) -> PathBuf {
    ctx.store
        .state_dir()
        .join("reviews")
        .join(format!("{task}-{run}-notes.md"))
}

mod prompt_text;
pub use prompt_text::review_prompt;

fn pane_alive(ctx: &Ctx, pane: &str) -> Result<bool> {
    Ok(ctx.herdr.pane_get(pane)?.is_some())
}

fn launch_profile(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    resolved: &ResolvedProfile,
    prompt_file: &Path,
    launch: &mut crate::review_budget::ReviewLaunch<'_>,
) -> Result<String> {
    let title = format!("{} review", task.id);
    let herdr = open_run_pane(ctx, task, run, "review")?;
    let saved = ctx.store.get(&task.id)?;
    let child = saved
        .current_run()
        .and_then(|r| r.review_runs.last())
        .context("review session disappeared before launch")?;
    let resolved = crate::role_session::command(ctx, child, resolved)?;
    let command =
        match crate::agent::hooks::review::command(ctx, task, run, &resolved, &herdr.pane_id) {
            Some(command) => command?,
            None => {
                let mut values = vec![
                    ("prompt_file", prompt_file.display().to_string()),
                    ("name", title.clone()),
                    ("task", task.id.clone()),
                    ("worktree", run.worktree.display().to_string()),
                ];
                if let Some(notice) = crate::round_notice::latest_review(&saved) {
                    values.push((crate::prompt::NOTICE_KEY, notice));
                }
                render_template(&resolved, &values)?
            }
        };
    attach_pane(ctx, task, run, &herdr.pane_id, true, launch)?;
    let mut child = String::new();
    ctx.store.update(&task.id, |task| {
        let review = review_parent(task, &run.id)?
            .review_runs
            .last_mut()
            .context("review child disappeared before launch")?;
        review.herdr = Some(herdr.clone());
        child = review.id.clone();
        Ok(())
    })?;
    let receipt = crate::launch_exit::receipt(ctx, &task.id, &format!("{}-{child}", run.id));
    let command = crate::launch_exit::arm(&command, &receipt)?;
    let pane = herdr.pane_id;
    ctx.herdr
        .pane_report_metadata(&pane, &title, &[("task", task.id.as_str())])?;
    submission::submit(ctx, task, run, launch, None, || {
        ctx.herdr.pane_run(&pane, &command)
    })?;
    Ok(pane)
}

fn attach_pane(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    pane: &str,
    close_pane: bool,
    launch: &mut crate::review_budget::ReviewLaunch<'_>,
) -> Result<()> {
    launch.attach(pane, close_pane)?;
    ctx.store.update(&task.id, |task| {
        let parent = review_parent(task, &run.id)?;
        if let Some(record) = &mut parent.review {
            record.pane = Some(pane.into());
        }
        if let Some(child) = parent.review_runs.last_mut() {
            child.review = parent.review.clone();
        }
        Ok(())
    })?;
    Ok(())
}

fn prompt_pm(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    review_id: &str,
    prompt_file: &Path,
    launch: &mut crate::review_budget::ReviewLaunch<'_>,
) -> Result<String> {
    let pane = match live_pm_pane(ctx)? {
        Some(p) => p,
        None => open_pm(ctx)?,
    };
    attach_pane(ctx, task, run, &pane, false, launch)?;
    let corral = corral_bin()?.display().to_string();
    let notes = review_notes_path(ctx, &task.id, &run.id);
    let prepared = ctx.store.get(&task.id)?;
    let attempt = prepared
        .current_run()
        .filter(|r| r.id == run.id)
        .and_then(|r| r.review.as_ref())
        .and_then(callback::token)
        .context("PM review attempt disappeared")?;
    let message = format!(
        "Review run {} of {} ({}): read the review prompt at {} and follow it. Do not edit files; finish with \
         `{corral} verdict {} approve|changes|escalate --attempt {} --notes-file {}`.",
        run.id,
        task.id,
        task.title,
        prompt_file.display(),
        task.id,
        attempt,
        notes.display()
    );
    let info = ctx
        .herdr
        .pane_get(&pane)?
        .context("PM pane disappeared before review")?;
    // Save the exact request before contacting the composer, including on transport errors.
    ctx.store.update(&task.id, |t| {
        let snapshot = review_parent(t, &run.id)?
            .review_runs
            .last_mut()
            .filter(|r| r.id == review_id)
            .context("review changed before prompting PM")?;
        snapshot.pending_reply = Some(message.clone());
        snapshot.herdr = Some(HerdrRef {
            workspace_id: info.workspace_id.clone(),
            pane_id: pane.clone(),
            socket: Some(ctx.herdr.socket_path()?),
        });
        Ok(())
    })?;
    let delivery = submission::submit(ctx, task, run, launch, Some(&message), || {
        ctx.herdr.agent_prompt(&pane, &message)
    })
    .with_context(|| format!("PM review prompt retained for {}", task.id))?;
    ctx.store.log_event(
        Some(&task.id),
        "review_prompt_delivery",
        json!({
            "run": run.id, "review_run": review_id, "pane": pane,
            "via": delivery.via, "submitted": delivery.submitted,
            "attempts": delivery.attempts, "error": delivery.error,
        }),
    )?;
    if !delivery.submitted {
        bail!(
            "PM review prompt was not submitted; retained for review-run {}: {}",
            task.id,
            delivery
                .error
                .as_deref()
                .unwrap_or("submission was not confirmed")
        );
    }
    ctx.store.update(&task.id, |t| {
        if let Some(snapshot) = review_parent(t, &run.id)?
            .review_runs
            .last_mut()
            .filter(|r| r.id == review_id)
        {
            snapshot.pending_reply = None;
        }
        Ok(())
    })?;
    Ok(pane)
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
    review_run_origin(ctx, id, reviewer, crate::operating::Origin::Owner)
}

pub(crate) fn review_run_automatic(
    ctx: &Ctx,
    id: &str,
    reviewer: Option<&str>,
) -> Result<ReviewRecord> {
    review_run_origin(ctx, id, reviewer, crate::operating::Origin::Automatic)
}

fn review_run_origin(
    ctx: &Ctx,
    id: &str,
    reviewer: Option<&str>,
    origin: crate::operating::Origin,
) -> Result<ReviewRecord> {
    let _attribution = ctx.store.attribute(corral_core::Actor::Reviewer);
    ctx.store.check_runtime()?;
    let mut fresh = ctx.clone();
    fresh.store.reload_config_if_changed()?;
    let ctx = &fresh;
    ctx.store.config.validate_review()?;
    let mut launch = crate::review_budget::ReviewLaunch::reserve_origin(
        ctx,
        id,
        crate::review_budget::Kind::Reviewer,
        false,
        origin,
    )?;
    let automatic = matches!(origin, crate::operating::Origin::Automatic);
    if launch.restored() {
        let task = ctx.store.get(id)?;
        let run = current_run(&task)?;
        let routed = run.review.as_ref().and_then(|r| r.route.as_deref());
        let profiles: Vec<&str> = routed
            .map(|route| route.profile.as_str())
            .into_iter()
            .chain(run.review_runs.last().map(|child| child.agent.as_str()))
            .collect();
        instruction_guard::check(ctx, &task, &run, &profiles, automatic)?;
        launch.resume()?;
        let record = current_run(&ctx.store.get(id)?)?
            .review
            .context("resumed review missing")?;
        launch.commit()?;
        return Ok(record);
    }
    let task = ctx.store.get(id)?;
    if task.status != Status::Review {
        bail!(
            "{} is {}; only Review tasks can be reviewed",
            task.id,
            task.status.label()
        );
    }
    let run = current_run(&task)?;
    let mut policy = ctx.store.config.task_review_policy(&task);
    let explicit = reviewer.is_some();
    let reviewer = crate::review_routing::choice(&ctx.store.config, &task, reviewer);
    if reviewer == Reviewer::CrossVendor {
        policy.require_cross_family = true;
    }
    let vendor = if policy.cross_family_required() {
        Some(implementer_vendor(ctx, &task, &run)?)
    } else {
        run.vendor.clone()
    };
    if !run.worktree.is_dir() {
        bail!("{}: worktree {} is gone", task.id, run.worktree.display());
    }
    let identity = match launch.bound_candidate() {
        Some(candidate) => candidate,
        None => verification::review_identity(
            ctx,
            &task,
            &run,
            run.gate
                .as_ref()
                .and_then(|g| g.identity.as_ref())
                .map(|i| i.attempt.as_str())
                .unwrap_or(&format!("review-{}", Timestamp::now().as_nanosecond())),
        )?,
    };
    let selected = crate::review_contracts::implementation_route(
        ctx,
        &task,
        &run,
        &reviewer,
        explicit,
        vendor.as_deref(),
        &identity,
    )?;
    let (name, mut resolved) = (selected.name, selected.resolved);
    if reviewer != Reviewer::Pm {
        let exclude = crate::availability::review_exclusion(ctx, &task, &run);
        resolved =
            crate::availability::bind_review(ctx, &task, &run, resolved, exclude.as_deref())?;
    }
    policy.check_reviewer_family(vendor.as_deref(), resolved.vendor.as_deref())?;
    crate::agent::hooks::review::preflight(ctx, &resolved)?;
    instruction_guard::check(
        ctx,
        &task,
        &run,
        &[selected.route.profile.as_str(), resolved.profile.as_str()],
        automatic,
    )?;
    if !corral_git::is_clean(&run.worktree)? {
        bail!(
            "{}: worktree {} has uncommitted changes; commit them so the review belongs to a commit",
            task.id,
            run.worktree.display()
        );
    }
    let sha = corral_git::rev_sha(&run.worktree, "HEAD")?;
    let mut record = ReviewRecord {
        route: Some(Box::new(selected.route)),
        evidence_digest: Some(verification::evidence_digest(&run)?),
        identity: Some(identity),
        verdict: None,
        process_lost: false,
        reviewer: name.clone(),
        notes: None,
        sha: sha.clone(),
        at: Timestamp::now(),
        pane: None,
    };
    let review_id = launch
        .bound_attempt()
        .unwrap_or_else(|| format!("{}-review{}", run.id, run.review_runs.len() + 1));
    launch.bind(
        record
            .identity
            .as_ref()
            .context("missing reviewer identity")?,
        &review_id,
        Some(&review_id),
    )?;
    crate::watch::capture_review(ctx, &task, &run);
    let captured = ctx.store.get(id)?;
    let inherited = callback::inherited(&run, record.identity.as_ref());
    if let Some(route) = record.route.as_mut() {
        route.attempt = Some(review_id.clone());
        route.callback = Some(inherited.unwrap_or_else(|| review_id.clone()));
    }
    let route = record.route.as_deref().context("review route missing")?;
    let (packet_digest, packet_section) = crate::review_packet::write(
        ctx,
        &captured,
        &current_run(&captured)?,
        &review_id,
        &record,
        route,
    )?;
    if let Some(route) = record.route.as_mut() {
        route.packet_digest = Some(packet_digest);
    }
    if let Some(prev) = &run.review
        && prev.reviewer != PM
        && let Some(p) = prev.pane.as_deref()
        && pane_alive(ctx, p)?
    {
        ctx.herdr.pane_close(p)?;
    }
    if let Some(p) = stronger::superseded_pane(&run)
        && pane_alive(ctx, p)?
    {
        ctx.herdr.pane_close(p)?;
    }
    let prompts = ctx.store.prompts_dir();
    std::fs::create_dir_all(&prompts).with_context(|| format!("creating {}", prompts.display()))?;
    let prompt_file = prompts.join(format!("{}-{}-review.md", task.id, run.id));
    let notes = review_notes_path(ctx, &task.id, &run.id);
    match std::fs::remove_file(&notes) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).context("clearing replaced reviewer notes"),
    }
    if let Some(dir) = notes.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    // Persist the window and active record before dispatch: the agent can answer immediately.
    let saved = ctx.store.update(&task.id, |t| {
        let r = review_parent(t, &run.id)?;
        if r.review_runs.len() != run.review_runs.len() || r.review != run.review {
            bail!("review changed before preparing accounting child");
        }
        finish_review(ctx, r, record.at);
        r.idle_since = None;
        r.blocked_excerpt = None;
        if let Some(previous) = r.review_runs.last_mut() {
            previous.pending_reply = None;
        }
        let mut child = Run {
            execution: None,
            delivery: crate::delivery::owner::review_launch_mode(&resolved.harness),
            inbox: Default::default(),
            completion: Default::default(),
            id: review_id.clone(),
            progress: None,
            checkpoints: vec![],
            checkpoint_required: false,
            agent: resolved.profile.clone(),
            profile: None,
            harness: Some(resolved.harness.clone()),
            model: resolved.model.clone(),
            effort: resolved.effort.clone(),
            route: None,
            vendor: resolved.vendor.clone(),
            failures: 0,
            switches: vec![],
            retry: None,
            mode: RunMode::Interactive,
            worktree: match reviewer {
                Reviewer::Pm => ctx.store.root().to_path_buf(),
                Reviewer::Profile(_) | Reviewer::CrossVendor | Reviewer::Auto => {
                    run.worktree.clone()
                }
                Reviewer::None => bail!("no reviewer configured"),
            },
            branch: run.branch.clone(),
            base: run.base.clone(),
            base_sha: run.base_sha.clone(),
            plan_target: run.plan_target.clone(),
            herdr: None,
            started: record.at,
            ended: None,
            outcome: None,
            summary: None,
            blocked_excerpt: None,
            agent_state: None,
            seen_pane: false,
            seen_working: false,
            gate: None,
            idle_since: None,
            idle_no_report: false,
            changes_round: None,
            herdr_idle_since: None,
            stuck: None,
            pending_reply: None,
            obligations: Default::default(),
            review: Some(record.clone()),
            review_runs: Vec::new(),
            usage: None,
            usage_note: None,
            autopilot: None,
            gate_rules: corral_core::GateRules::Legacy,
            promotion_origin: None,
            agent_host: None,
            session: None,
        };
        if reviewer != Reviewer::Pm {
            child.session = Some(crate::role_session::reviewer(ctx, &task, &run, &child)?);
        }
        if let Some(previous) = r.review_runs.last_mut()
            && let Ok(session) = crate::role_session::capture(ctx, &task, previous, true)
        {
            previous.session = Some(session);
        }
        r.review_runs.push(child);
        r.review = Some(record.clone());
        Ok(())
    })?;
    let review_id = saved
        .current_run()
        .and_then(|r| r.review_runs.last())
        .context("review snapshot was not saved")?
        .id
        .clone();
    crate::watch::start_review(
        ctx,
        &saved,
        saved
            .current_run()
            .and_then(|r| r.review_runs.last())
            .context("review accounting child disappeared")?,
    )?;
    let mut prompt = review_prompt(
        ctx,
        &saved,
        saved.current_run().context("review run disappeared")?,
    )? + &packet_section;
    if let Some(notice) = crate::round_notice::latest_review(&saved) {
        prompt = format!("{notice}\n\n{prompt}");
    }
    std::fs::write(&prompt_file, prompt)
        .with_context(|| format!("writing {}", prompt_file.display()))?;
    let launched = match &reviewer {
        Reviewer::Profile(_) | Reviewer::CrossVendor | Reviewer::Auto => {
            launch_profile(ctx, &task, &run, &resolved, &prompt_file, &mut launch)
        }
        Reviewer::Pm => prompt_pm(ctx, &task, &run, &review_id, &prompt_file, &mut launch),
        Reviewer::None => bail!("no reviewer configured"),
    };
    if let Err(error) = &launched
        && crate::operating::refused(error)
    {
        return Err(launched.err().context("review refusal disappeared")?);
    }
    let saved = ctx.store.update(&task.id, |t| {
        let r = review_parent(t, &run.id)?;
        // Never overwrite an immediate verdict, or a replacement review's record.
        if r.review_runs.last().is_some_and(|s| s.id == review_id) {
            match &launched {
                Ok(pane) => {
                    if let Some(record) = &mut r.review {
                        record.pane = Some(pane.clone());
                    }
                }
                Err(_) => {
                    finish_review(ctx, r, Timestamp::now());
                    if r.review.as_ref().is_some_and(|r| r.verdict.is_none()) {
                        r.review = None;
                    }
                }
            }
        }
        Ok(())
    })?;
    let pane = launched?;
    if let Some(child) = saved.current_run().and_then(|r| r.review_runs.last()) {
        crate::role_session::observe(ctx, &saved, child, true);
        crate::role_session::log(ctx, &saved, child)?;
    }
    let record = saved
        .current_run()
        .and_then(|r| r.review.clone())
        .context("review record was not saved")?;
    launch.commit()?;
    if reviewer == Reviewer::CrossVendor {
        let switch = ProfileSwitch {
            from: run.agent.clone(),
            to: name.clone(),
            reason: "cross-vendor review".into(),
        };
        ctx.store.update(&task.id, |t| {
            let r = t
                .current_run_mut()
                .filter(|r| r.id == run.id)
                .context("run changed before recording reviewer selection")?;
            r.switches.push(switch.clone());
            Ok(())
        })?;
        ctx.store.log_event(
            Some(&task.id),
            "profile_switched",
            json!({
                "run": run.id, "from": switch.from, "to": switch.to, "reason": switch.reason,
            }),
        )?;
    }
    ctx.store.log_event(
        Some(&task.id),
        "review_started",
        json!({ "run": run.id, "reviewer": name, "pane": pane, "sha": sha, "prompt": prompt_file }),
    )?;
    Ok(record)
}

/// Resolve under the task lock and preserve the first successful historical vendor snapshot.
pub(crate) fn implementer_vendor(ctx: &Ctx, task: &Task, run: &Run) -> Result<String> {
    if let Some(vendor) = &run.vendor {
        return Ok(vendor.clone());
    }
    let saved = ctx.store.update(&task.id, |t| {
        let r = review_parent(t, &run.id)?;
        r.vendor = Some(ctx.store.config.run_vendor(r)?);
        Ok(())
    })?;
    ctx.store
        .config
        .run_vendor(&current_run(&saved)?)
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
/// since the review started (stale review), or when `notes` is empty. Main moving forward is not stale: the
/// verdict stays bound to the base it reviewed (event "verdict_base_moved") for autopilot to replay and carry.
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
    verdict_with_clock(ctx, id, verdict, notes, None, None, || at).map(|(record, _)| record)
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
    let now = Timestamp::now;
    verdict_with_clock(ctx, id, verdict, notes, expected_attempt, None, now).map(|(r, _)| r)
}

/// The Pi-native `verdict` op on the bound review child: the receipt and VerdictReport commit
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
        let now = Timestamp::now;
        verdict_with_clock(ctx, id, verdict, notes, expected_attempt, Some(report), now)
            .map(|(_, receipt)| receipt)
    })
}

type Recorded = (ReviewRecord, Option<corral_core::agent::RequestReceipt>);

fn verdict_with_clock(
    ctx: &Ctx,
    id: &str,
    verdict: Verdict,
    notes: &str,
    expected_attempt: Option<&str>,
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
                && expected_attempt.is_none_or(|attempt| callback::matches(run, review, attempt))
        })
    });
    let result = verdict_inner(ctx, id, verdict, notes, expected_attempt, report, now);
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
    expected_attempt: Option<&str>,
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
    if !run.worktree.is_dir() {
        bail!("{}: worktree {} is gone", task.id, run.worktree.display());
    }
    let head = corral_git::rev_sha(&run.worktree, "HEAD")?;
    let started = run.review.clone().filter(|r| r.verdict.is_none());
    if let Some(attempt) = expected_attempt
        && !started
            .as_ref()
            .is_some_and(|r| callback::matches(&run, r, attempt))
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
        .unwrap_or_else(|| format!("owner-{}", Timestamp::now().as_nanosecond()));
    let record = started.clone().unwrap_or(ReviewRecord {
        evidence_digest: Some(verification::evidence_digest(&run)?),
        identity: Some(verification::review_identity(ctx, &task, &run, &attempt)?),
        verdict: None,
        process_lost: false,
        reviewer: OWNER.to_string(),
        notes: None,
        sha: head.clone(),
        at: Timestamp::now(),
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
            (Verdict::Escalate, escalation),
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
            (changes, moved) = verification::verdict_changes(ctx, &task, &run, s)?;
            // The packet records the new base's gate freshness; the assessment alone must hold.
            let packet = moved.is_none();
            if changes.is_empty()
                && let Some(route) = s.route.as_deref()
            {
                changes = crate::review_packet::route_changes(ctx, &task, &run, s, route, packet)?;
            }
            if !changes.is_empty() {
                bail!("stale review: {}", serde_json::to_string(&changes)?);
            }
            match s.route.as_deref() {
                Some(route) => {
                    let moved = moved.as_ref();
                    crate::review_packet::enforce(ctx, &task, &run, route, (verdict, notes), moved)?
                }
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
            .unwrap_or_else(|| format!("owner-{}", Timestamp::now().as_nanosecond()));
        record.identity = Some(verification::review_identity(ctx, &task, &run, &attempt)?);
    }
    let recorded = record_verdict(ctx, &task, &run, record, (verdict, notes), now(), report)?;
    if let Some(base) = moved {
        let detail = json!({"run": run.id, "verdict": verdict.as_str(),
            "reviewed_base": base.expected, "current_base": base.actual});
        ctx.store
            .log_event(Some(&task.id), "verdict_base_moved", detail)?;
    }
    Ok(recorded)
}
