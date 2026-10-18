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

use crate::Ctx;
use crate::artifacts::add_artifact;
use crate::gate::{gate_paths, open_run_pane};
use crate::pm::{live_pm_pane, open_pm};
use crate::prompt::{corral_bin, fill, render_template};
use crate::review::current_run;
use crate::verification;

const OWNER: &str = "owner";
const PM: &str = "pm";

const DEFAULT_TEMPLATE: &str = include_str!("../assets/review.md");

mod watchdog;
pub use watchdog::reconcile_review_at;
mod history;
mod submission;
use history::finish_review;
pub(crate) use history::supersede_review;

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
use prompt_text::{artifacts_text, gate_text};

/// The review prompt for the task's run, as markdown: `.corral/prompts/review.md` when present, else the
/// built-in template. Placeholders: {{id}} {{title}} {{run}} {{branch}} {{base}} {{worktree}} {{spec}}
/// {{summary}} {{gate}} {{diff_cmd}} {{artifacts}} {{agents_md}} {{notes_file}} {{verdict_cmd}}.
pub fn review_prompt(ctx: &Ctx, task: &Task, run: &Run) -> Result<String> {
    let override_file = ctx.store.prompts_dir().join("review.md");
    let template = match std::fs::read_to_string(&override_file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DEFAULT_TEMPLATE.to_string(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", override_file.display())),
    };
    let corral = crate::prompt::shell_path(&corral_bin()?);
    let project_dir = match task.project.as_deref() {
        Some(p) => ctx.store.project_path(p)?,
        None => run.worktree.clone(),
    };
    let spec = if task.body.trim().is_empty() {
        "(the task has no spec beyond its title)".to_string()
    } else {
        task.body.trim_end().to_string()
    };
    let values = [
        ("id", task.id.clone()),
        ("title", task.title.clone()),
        ("run", run.id.clone()),
        ("branch", run.branch.clone()),
        ("base", run.base.clone()),
        ("worktree", run.worktree.display().to_string()),
        ("spec", spec),
        (
            "summary",
            run.summary
                .clone()
                .unwrap_or_else(|| "(no summary)".to_string()),
        ),
        ("gate", gate_text(ctx, task, run)),
        ("diff_cmd", format!("{corral} review {} --diff", task.id)),
        ("artifacts", artifacts_text(ctx, task, run)),
        (
            "agents_md",
            format!(
                "Follow {}/AGENTS.md (and any deeper AGENTS.md) if it exists.",
                project_dir.display()
            ),
        ),
        (
            "notes_file",
            review_notes_path(ctx, &task.id, &run.id)
                .display()
                .to_string(),
        ),
        ("verdict_cmd", {
            let attempt = run
                .review
                .as_ref()
                .and_then(|r| r.identity.as_ref())
                .or_else(|| run.gate.as_ref().and_then(|g| g.identity.as_ref()))
                .map(|i| format!(" --attempt {}", i.attempt))
                .unwrap_or_default();
            format!("{corral} verdict {}{attempt}", task.id)
        }),
    ];
    let mut prompt = fill(&template, &values);
    prompt.push_str(&crate::lessons::lessons_prompt(ctx, task, run)?);
    prompt.push_str(&crate::comments::owner_comments(task));
    if task.completion.is_some() {
        prompt.push_str(&format!(
            "\n## Criterion evidence (worker assertions, not machine PASS)\n\n```json\n{}\n```\n",
            serde_json::to_string_pretty(&run.completion)?
        ));
    }
    Ok(prompt)
}

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
    let command = render_template(
        resolved,
        &[
            ("prompt_file", prompt_file.display().to_string()),
            ("name", title.clone()),
            ("task", task.id.clone()),
            ("worktree", run.worktree.display().to_string()),
        ],
    )?;
    let herdr = open_run_pane(ctx, task, run, "review")?;
    attach_pane(ctx, task, run, &herdr.pane_id, true, launch)?;
    ctx.store.update(&task.id, |task| {
        review_parent(task, &run.id)?
            .review_runs
            .last_mut()
            .context("review child disappeared before launch")?
            .herdr = Some(herdr.clone());
        Ok(())
    })?;
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
        .and_then(|r| r.identity.as_ref())
        .context("PM review attempt disappeared")?;
    let message = format!(
        "Review run {} of {} ({}): read the review prompt at {} and follow it. Do not edit files; finish with \
         `{corral} verdict {} approve|changes|escalate --attempt {} --notes-file {}`.",
        run.id,
        task.id,
        task.title,
        prompt_file.display(),
        task.id,
        attempt.attempt,
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
    if launch.restored() {
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
    let reviewer = crate::review_routing::choice(&ctx.store.config, &task, reviewer);
    if reviewer == Reviewer::CrossVendor {
        policy.require_cross_family = true;
    }
    let vendor = if policy.cross_family_required() {
        Some(implementer_vendor(ctx, &task, &run)?)
    } else {
        run.vendor.clone()
    };
    let (name, resolved) =
        crate::review_routing::resolve(&ctx.store.config, &reviewer, vendor.as_deref())?;
    policy.check_reviewer_family(vendor.as_deref(), resolved.vendor.as_deref())?;
    if !run.worktree.is_dir() {
        bail!("{}: worktree {} is gone", task.id, run.worktree.display());
    }
    if !corral_git::is_clean(&run.worktree)? {
        bail!(
            "{}: worktree {} has uncommitted changes; commit them so the review belongs to a commit",
            task.id,
            run.worktree.display()
        );
    }
    let sha = corral_git::rev_sha(&run.worktree, "HEAD")?;
    let record = ReviewRecord {
        evidence_digest: Some(verification::evidence_digest(&run)?),
        identity: Some(match launch.bound_candidate() {
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
        }),
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
    if let Some(prev) = &run.review
        && prev.reviewer != PM
        && let Some(p) = prev.pane.as_deref()
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
        r.review_runs.push(Run {
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
                Reviewer::Profile(_) | Reviewer::CrossVendor => run.worktree.clone(),
                Reviewer::None => bail!("no reviewer configured"),
            },
            branch: run.branch.clone(),
            base: run.base.clone(),
            base_sha: run.base_sha.clone(),
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
            herdr_idle_since: None,
            stuck: None,
            pending_reply: None,
            obligations: Default::default(),
            review: Some(record.clone()),
            review_runs: Vec::new(),
            usage: None,
            usage_note: None,
            autopilot: None,
        });
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
    std::fs::write(
        &prompt_file,
        review_prompt(
            ctx,
            &saved,
            saved.current_run().context("review run disappeared")?,
        )?,
    )
    .with_context(|| format!("writing {}", prompt_file.display()))?;
    let launched = match &reviewer {
        Reviewer::Profile(_) | Reviewer::CrossVendor => {
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

fn worktree_changes(worktree: &Path) -> Result<String> {
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

fn record_verdict(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    mut record: ReviewRecord,
    verdict: Verdict,
    notes: String,
) -> Result<ReviewRecord> {
    let ended = Timestamp::now();
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
    ctx.store.update(&task.id, |t| {
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
        Ok(())
    })?;
    let saved = ctx.store.get(&task.id)?;
    if let Some(run) = saved.current_run() {
        crate::watch::capture_review(ctx, &saved, run);
    }
    ctx.store.log_event(
        Some(&task.id),
        "verdict",
        json!({ "run": run.id, "verdict": verdict.as_str(), "reviewer": record.reviewer, "sha": record.sha, "notes": record.notes }),
    )?;
    Ok(record)
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

/// Reviewer callbacks must carry the attempt from their prompt. An unbound call is
/// an explicit manual verdict and is never attributed to a superseded reviewer.
pub fn verdict_bound(
    ctx: &Ctx,
    id: &str,
    verdict: Verdict,
    notes: &str,
    expected_attempt: Option<&str>,
) -> Result<ReviewRecord> {
    // An unbound verdict is an explicit manual action, not a reviewer callback.
    let _attribution = expected_attempt.map(|_| ctx.store.attribute(corral_core::Actor::Reviewer));
    ctx.store.check_runtime()?;
    let task = ctx.store.get(id)?;
    let pending = task.current_run().and_then(|run| {
        run.review.as_ref().filter(|review| {
            review.verdict.is_none()
                && expected_attempt.is_none_or(|attempt| {
                    review.identity.as_ref().map(|i| i.attempt.as_str()) == Some(attempt)
                })
        })
    });
    let result = verdict_inner(ctx, id, verdict, notes, expected_attempt);
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
) -> Result<ReviewRecord> {
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
        && started
            .as_ref()
            .and_then(|r| r.identity.as_ref())
            .map(|i| i.attempt.as_str())
            != Some(attempt)
    {
        let actual = started
            .as_ref()
            .and_then(|r| r.identity.as_ref())
            .map(|i| i.attempt.as_str());
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
        record_verdict(ctx, &task, &run, record, Verdict::Escalate, escalation)?;
        bail!(
            "{}: worktree {} has uncommitted changes; a reviewer must not edit files. Recorded verdict \
             escalate: reviewer modified the worktree",
            task.id,
            run.worktree.display()
        );
    }
    if let Some(s) = &started {
        let changes = verification::review_changes(ctx, &task, &run, s)?;
        if !changes.is_empty() {
            bail!("stale review: {}", serde_json::to_string(&changes)?);
        }
    }
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
    record_verdict(ctx, &task, &run, record, verdict, notes.to_string())
}
