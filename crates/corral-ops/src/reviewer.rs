//! Reviewer runs and structured verdicts: `review_run` hands the current run to an independent reviewer (a
//! profile in its own pane, or the PM agent), `verdict` records the reviewer's machine-readable judgment.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};
use corral_core::{
    ArtifactKind, GateResult, ProfileChoice, ReviewRecord, Reviewer, Run, Status, Task, Verdict,
};
use jiff::Timestamp;
use serde_json::json;

use crate::Ctx;
use crate::artifacts::add_artifact;
use crate::gate::{gate_paths, open_run_pane, worktree_head};
use crate::pm::{live_pm_pane, open_pm};
use crate::prompt::{corral_bin, fill, render_template};
use crate::review::current_run;

const OWNER: &str = "owner";
const PM: &str = "pm";

const DEFAULT_TEMPLATE: &str = include_str!("../assets/review.md");

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

fn gate_text(ctx: &Ctx, task: &Task, run: &Run) -> String {
    let Some(g) = &run.gate else {
        return "No gate has run for this commit.".to_string();
    };
    let sha = short(&g.sha);
    let mut text = match g.result {
        GateResult::Running => format!("Gate still running at {sha}."),
        GateResult::Pass => format!("Gate passed at {sha} ({})", g.at),
        GateResult::Fail => format!(
            "Gate FAILED at {sha} ({}): `{}`",
            g.at,
            g.failed_cmd.as_deref().unwrap_or("?")
        ),
    };
    if let Some(head) = worktree_head(run)
        && head != g.sha
    {
        text.push_str(&format!(
            " (stale: HEAD is now {}, the gate did not check it)",
            short(&head)
        ));
    }
    let log = gate_paths(ctx, &task.id, &run.id).log;
    if log.is_file() {
        text.push_str(&format!("\n\nGate log: {}", log.display()));
    }
    text
}

fn artifacts_text(ctx: &Ctx, task: &Task, run: &Run) -> String {
    let rows: Vec<String> = task
        .artifacts
        .iter()
        .filter(|a| a.run == run.id)
        .map(|a| {
            format!(
                "- [{}] {}: {}",
                a.kind.as_str(),
                a.title,
                ctx.store.dir().join(&a.path).display()
            )
        })
        .collect();
    if rows.is_empty() {
        "(none)".to_string()
    } else {
        rows.join("\n")
    }
}

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
    let corral = corral_bin()?.display().to_string();
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
        ("verdict_cmd", format!("{corral} verdict {}", task.id)),
    ];
    Ok(fill(&template, &values))
}

fn pane_alive(ctx: &Ctx, pane: &str) -> Result<bool> {
    Ok(ctx.herdr.pane_get(pane)?.is_some())
}

fn launch_profile(
    ctx: &Ctx,
    task: &Task,
    run: &Run,
    profile: &str,
    prompt_file: &Path,
) -> Result<String> {
    let choice = ProfileChoice {
        profile: Some(profile.to_string()),
        ..ProfileChoice::default()
    };
    let resolved = ctx.store.config.resolve_profile(&choice, None, None)?;
    let title = format!("{} review", task.id);
    let command = render_template(
        &resolved,
        &[
            ("prompt_file", prompt_file.display().to_string()),
            ("name", title.clone()),
            ("task", task.id.clone()),
            ("worktree", run.worktree.display().to_string()),
        ],
    )?;
    let pane = open_run_pane(ctx, task, run, "review")?;
    let launched = ctx.herdr.pane_run(&pane, &command).and_then(|()| {
        ctx.herdr
            .pane_report_metadata(&pane, &title, &[("task", task.id.as_str())])
    });
    if let Err(err) = launched {
        if let Err(close_err) = ctx.herdr.pane_close(&pane) {
            eprintln!("corral: closing reviewer pane {pane} after failed launch: {close_err:#}");
        }
        return Err(err);
    }
    Ok(pane)
}

fn prompt_pm(ctx: &Ctx, task: &Task, run: &Run, prompt_file: &Path) -> Result<String> {
    let pane = match live_pm_pane(ctx)? {
        Some(p) => p,
        None => open_pm(ctx)?,
    };
    let corral = corral_bin()?.display().to_string();
    let notes = review_notes_path(ctx, &task.id, &run.id);
    let message = format!(
        "Review run {} of {} ({}): read the review prompt at {} and follow it. Do not edit files; finish with \
         `{corral} verdict {} approve|changes|escalate --notes-file {}`.",
        run.id,
        task.id,
        task.title,
        prompt_file.display(),
        task.id,
        notes.display()
    );
    ctx.herdr.agent_prompt(&pane, &message)?;
    Ok(pane)
}

/// Starts a review of the current run of a Review task. `reviewer`: "pm" or a profile; default: the effective
/// review policy's reviewer. The worktree must be clean so the review belongs to HEAD.
/// - profile: a "review" tab in the run's herdr workspace (else a new workspace) on the run's worktree runs that
///   profile with the rendered review prompt (`prompts/<id>-<run>-review.md`);
/// - pm: the PM agent gets a request to follow that prompt (`agent_prompt`, falling back to send-text).
///
/// Records `review: {reviewer, sha, at, pane}` (no verdict yet) on the run and logs "review_started". A
/// previous reviewer pane of this run that is still open is closed first (never the PM pane).
pub fn review_run(ctx: &Ctx, id: &str, reviewer: Option<&str>) -> Result<ReviewRecord> {
    ctx.store.config.validate_review()?;
    let task = ctx.store.get(id)?;
    if task.status != Status::Review {
        bail!(
            "{} is {}; only Review tasks can be reviewed",
            task.id,
            task.status.label()
        );
    }
    let run = current_run(&task)?;
    let reviewer = match reviewer {
        Some(r) => Reviewer::parse(r),
        None => {
            ctx.store
                .config
                .review_policy(task.project.as_deref(), task.review)
                .reviewer
        }
    };
    let name = match &reviewer {
        Reviewer::None => bail!(
            "no reviewer for {}: pass --reviewer <profile|pm> or set `reviewer` under [review]",
            task.id
        ),
        Reviewer::Pm => PM.to_string(),
        Reviewer::Profile(p) => {
            let profiles = ctx.store.config.profile_names();
            if !profiles.contains(p) {
                bail!(
                    "unknown reviewer profile '{p}' (use pm or one of: {})",
                    profiles.join(", ")
                );
            }
            p.clone()
        }
    };
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

    if let Some(prev) = &run.review
        && prev.reviewer != PM
        && let Some(p) = prev.pane.as_deref()
        && pane_alive(ctx, p)?
        && let Err(e) = ctx.herdr.pane_close(p)
    {
        eprintln!("corral: could not close previous reviewer pane {p} (ignored): {e:#}");
    }

    let prompts = ctx.store.prompts_dir();
    std::fs::create_dir_all(&prompts).with_context(|| format!("creating {}", prompts.display()))?;
    let prompt_file = prompts.join(format!("{}-{}-review.md", task.id, run.id));
    std::fs::write(&prompt_file, review_prompt(ctx, &task, &run)?)
        .with_context(|| format!("writing {}", prompt_file.display()))?;
    let notes = review_notes_path(ctx, &task.id, &run.id);
    if let Some(dir) = notes.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }

    let pane = match &reviewer {
        Reviewer::Profile(p) => launch_profile(ctx, &task, &run, p, &prompt_file)?,
        _ => prompt_pm(ctx, &task, &run, &prompt_file)?,
    };
    let record = ReviewRecord {
        verdict: None,
        reviewer: name.clone(),
        notes: None,
        sha: sha.clone(),
        at: Timestamp::now(),
        pane: Some(pane.clone()),
    };
    save_review(ctx, &task.id, &run.id, &record)?;
    ctx.store.log_event(
        Some(&task.id),
        "review_started",
        json!({ "run": run.id, "reviewer": name, "pane": pane, "sha": sha, "prompt": prompt_file }),
    )?;
    Ok(record)
}

fn save_review(ctx: &Ctx, task: &str, run_id: &str, record: &ReviewRecord) -> Result<()> {
    ctx.store.update(task, |t| {
        let r = t
            .current_run_mut()
            .filter(|r| r.id == run_id)
            .ok_or_else(|| anyhow!("{task}: run {run_id} is no longer current"))?;
        r.review = Some(record.clone());
        Ok(())
    })?;
    Ok(())
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
        eprintln!("corral: could not remove {} (ignored): {e}", tmp.display());
    }
    snapshot?;
    record.verdict = Some(verdict);
    record.notes = Some(notes);
    record.at = Timestamp::now();
    save_review(ctx, &task.id, &run.id, &record)?;
    ctx.store.log_event(
        Some(&task.id),
        "verdict",
        json!({ "run": run.id, "verdict": verdict.as_str(), "reviewer": record.reviewer, "sha": record.sha }),
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
    let record = started.clone().unwrap_or_else(|| ReviewRecord {
        verdict: None,
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
    if let Some(s) = &started
        && s.sha != head
    {
        bail!(
            "stale review: {} HEAD moved from {} to {} since the review started; start a new review with \
             `corral review-run {}`",
            task.id,
            short(&s.sha),
            short(&head),
            task.id
        );
    }
    record_verdict(ctx, &task, &run, record, verdict, notes.to_string())
}
