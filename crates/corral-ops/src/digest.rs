//! Event-window reports with recorded run accounting. No model or transcript access.
use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use corral_core::{Artifact, ArtifactKind, Event, PinRef, Status, Task, parse_age};
use jiff::Timestamp;
use serde::Serialize;
use serde_json::json;

use crate::{Ctx, SprintNumbers, SprintTotals};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DigestEntry {
    pub task: Option<String>,
    pub title: Option<String>,
    pub run: Option<String>,
    pub at: Timestamp,
    pub kind: String,
    pub sha: Option<String>,
    pub verdict: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Digest {
    /// Events and accounting use (since, snapshot_at].
    pub since: Timestamp,
    pub snapshot_at: Timestamp,
    pub landed: Vec<DigestEntry>,
    pub escalated: Vec<DigestEntry>,
    pub sent_back: Vec<DigestEntry>,
    pub dropped: Vec<DigestEntry>,
    /// Recorded cumulative costs of runs ending in the window (unfinished: starting there).
    /// Reviewer child runs are counted separately, never twice.
    pub totals: SprintTotals,
    pub by_task: BTreeMap<String, SprintTotals>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DigestReport {
    #[serde(flatten)]
    pub digest: Digest,
    pub artifact: Artifact,
    pub artifact_ref: String,
    pub markdown: String,
}

/// Pure collection at a fixed clock. Missing usage remains unknown, not a zero-cost run.
pub fn collect_digest(
    tasks: &[Task],
    events: &[Event],
    since: Timestamp,
    snapshot_at: Timestamp,
) -> Digest {
    let mut digest = Digest {
        since,
        snapshot_at,
        landed: Vec::new(),
        escalated: Vec::new(),
        sent_back: Vec::new(),
        dropped: Vec::new(),
        totals: SprintTotals::default(),
        by_task: BTreeMap::new(),
    };
    let mut history: Vec<_> = events.iter().filter(|e| e.ts <= snapshot_at).collect();
    history.sort_by_key(|e| e.ts);
    for event in history.iter().copied().filter(|e| e.ts > since) {
        let section = match event.kind.as_str() {
            "approved" if event.detail["strategy"] != "keep_branch" => &mut digest.landed,
            "needs_me" | "escalated" => &mut digest.escalated,
            "autopilot" if event.detail["to"] == "escalated" => &mut digest.escalated,
            "verdict" if event.detail["verdict"] == "escalate" => &mut digest.escalated,
            "changes_requested"
                if event.detail["by"] != "autopilot"
                    && (event.detail["submitted"] != false
                        || observed(&history, &event.detail["message"])) =>
            {
                &mut digest.sent_back
            }
            // The transition covers both delivery to the same pane and a queued
            // retry when that pane is gone. Do not also count its delivery event.
            "autopilot" if event.detail["to"] == "changes" => &mut digest.sent_back,
            "discarded" | "dropped" => &mut digest.dropped,
            "idea_decided"
                if matches!(
                    event.detail["action"].as_str(),
                    Some("drop" | "decline" | "merge")
                ) =>
            {
                &mut digest.dropped
            }
            "moved"
                if event.detail["to"] == "dropped"
                    && !history.iter().any(|audit| {
                        audit.ts > since
                            && audit.task == event.task
                            && audit.kind == "idea_decided"
                            && !event.detail["task_updated"].is_null()
                            && audit.detail["task_updated"] == event.detail["task_updated"]
                            && matches!(
                                audit.detail["action"].as_str(),
                                Some("drop" | "decline" | "merge")
                            )
                    }) =>
            {
                &mut digest.dropped
            }
            _ => continue,
        };
        let task = tasks.iter().find(|t| event.task.as_deref() == Some(&t.id));
        let run = task.and_then(|task| match event.detail["run"].as_str() {
            Some(id) => task.runs.iter().find(|r| r.id == id),
            None => task
                .runs
                .iter()
                .filter(|r| r.started <= event.ts)
                .max_by_key(|r| r.started),
        });
        let run_id = event.detail["run"]
            .as_str()
            .or_else(|| run.map(|r| r.id.as_str()));
        let verdict = history
            .iter()
            .rev()
            .find(|e| {
                e.ts <= event.ts
                    && e.task == event.task
                    && e.kind == "verdict"
                    && match e.detail["run"].as_str() {
                        Some(id) => Some(id) == run_id,
                        None => run.is_some_and(|r| e.ts >= r.started),
                    }
            })
            .and_then(|e| e.detail["verdict"].as_str())
            .or_else(|| {
                run.and_then(|r| r.review.as_ref())
                    .filter(|r| r.at <= event.ts)
                    .and_then(|r| r.verdict.map(|v| v.as_str()))
            });
        section.push(DigestEntry {
            task: event.task.clone(),
            title: task.map(|t| t.title.clone()),
            run: run_id.map(str::to_owned),
            at: event.ts,
            kind: event.kind.clone(),
            sha: event.detail["sha"].as_str().map(str::to_owned),
            verdict: verdict.map(str::to_owned),
            reason: ["reason", "text", "question", "note"]
                .iter()
                .find_map(|key| event.detail[*key].as_str().map(str::to_owned)),
        });
    }
    let mut numbers = SprintNumbers::default();
    for task in tasks {
        for run in &task.runs {
            crate::sprint::account_run(&mut numbers, &task.id, run, Some(since), snapshot_at);
        }
    }
    digest.by_task = numbers.by_task;
    for total in digest.by_task.values() {
        digest.totals.runs += total.runs;
        digest.totals.unknown_cost_runs += total.unknown_cost_runs;
        if let Some(cost) = total.reported_usd {
            *digest.totals.reported_usd.get_or_insert(0.0) += cost;
        }
        if let Some(cost) = total.estimated_usd {
            *digest.totals.estimated_usd.get_or_insert(0.0) += cost;
        }
        if let Some(seconds) = total.wall_seconds {
            *digest.totals.wall_seconds.get_or_insert(0) += seconds;
        }
    }
    digest
}

/// Generate and pin a digest for the last N h/d/w (default CLI window: 1d).
pub fn digest(ctx: &Ctx, since: &str) -> Result<DigestReport> {
    digest_at(ctx, since, Timestamp::now())
}

/// Explicit clock for deterministic callers.
pub fn digest_at(ctx: &Ctx, since: &str, now: Timestamp) -> Result<DigestReport> {
    let since = now.checked_sub(parse_age(since)?)?;
    let _lock = ctx
        .store
        .try_lock("digest")?
        .context("another digest is in progress")?;
    write_report(ctx, since, now, false)
}

/// Called by sync. First enabled sync reports the last interval; later reports
/// start at the previous scheduled snapshot so downtime never drops events.
pub fn scheduled_digest_at(ctx: &Ctx, now: Timestamp) -> Result<Option<DigestReport>> {
    let Some(every) = ctx.store.config.digest.every.as_deref() else {
        return Ok(None);
    };
    let every = parse_age(every)?;
    let Some(_lock) = ctx.store.try_lock("digest")? else {
        return Ok(None);
    };
    let previous = ctx
        .store
        .events_shared()?
        .iter()
        .rev()
        .find(|e| e.kind == "digest" && e.detail["scheduled"] == true)
        .map(|event| {
            event.detail["snapshot_at"]
                .as_str()
                .context("scheduled digest has no snapshot_at")?
                .parse::<Timestamp>()
                .context("invalid scheduled digest snapshot")
        })
        .transpose()?;
    if previous.is_some_and(|at| now.duration_since(at) < every) {
        return Ok(None);
    }
    let since = match previous {
        Some(at) => at,
        None => now.checked_sub(every)?,
    };
    write_report(ctx, since, now, true).map(Some)
}

fn write_report(
    ctx: &Ctx,
    since: Timestamp,
    now: Timestamp,
    scheduled: bool,
) -> Result<DigestReport> {
    ensure!(since <= now, "digest window starts after its snapshot");
    let tasks = ctx.store.list()?;
    let digest = collect_digest(&tasks, ctx.store.events_shared()?.as_slice(), since, now);
    let markdown = render(&digest);
    // Artifacts are task-owned. Match sprint reports' stable first-task anchor,
    // with a single Done anchor only for an initially empty workspace.
    let anchor = match tasks.iter().min_by_key(|t| &t.id) {
        Some(task) => task.id.clone(),
        None => {
            ctx.store
                .create(
                    "Digest reports",
                    None,
                    Status::Done,
                    "Artifact anchor for deterministic workspace digests.",
                )?
                .id
        }
    };
    let scratch = tempfile::tempdir().context("creating digest scratch directory")?;
    let path = scratch.path().join("digest-report.md");
    std::fs::write(&path, &markdown).context("writing digest report")?;
    let artifact = crate::add_artifact(
        ctx,
        &anchor,
        &path,
        Some(ArtifactKind::Report),
        Some("Daily digest"),
    )?;
    let artifact_ref = PinRef::of_artifact(&anchor, &artifact).to_string();
    ctx.store.pin(&artifact_ref, Some("Daily digest"), false)?;
    ctx.store.log_event(
        Some(&anchor),
        "digest",
        json!({"since": since, "snapshot_at": now, "scheduled": scheduled, "artifact": artifact_ref}),
    )?;
    Ok(DigestReport {
        digest,
        artifact,
        artifact_ref,
        markdown,
    })
}

fn render(digest: &Digest) -> String {
    let mut out = format!(
        "# Daily digest\n\nWindow: ({}, {}]\n",
        digest.since, digest.snapshot_at
    );
    for (heading, entries) in [
        ("Landed", &digest.landed),
        ("Escalated / needs-owner", &digest.escalated),
        ("Sent back", &digest.sent_back),
        ("Dropped", &digest.dropped),
    ] {
        out.push_str(&format!("\n## {heading}\n\n"));
        if entries.is_empty() {
            out.push_str("None.\n");
        }
        for row in entries {
            out.push_str(&format!(
                "- {} {} at {}",
                row.task.as_deref().unwrap_or("workspace"),
                row.title.as_deref().unwrap_or(""),
                row.at
            ));
            if let Some(sha) = &row.sha {
                out.push_str(&format!("; sha `{sha}`"));
            }
            if heading == "Landed" {
                out.push_str(&format!(
                    "; reviewer verdict: {}",
                    row.verdict.as_deref().unwrap_or("not recorded")
                ));
            }
            if let Some(reason) = &row.reason {
                out.push_str(&format!("; {}", reason.replace('\n', " ")));
            }
            out.push('\n');
        }
    }
    let total = &digest.totals;
    let money = |value: Option<f64>| {
        value
            .map(|v| format!("${v:.6}"))
            .unwrap_or_else(|| "unknown".into())
    };
    out.push_str(&format!(
        "\n## Cost / time totals\n\n\
         Recorded cumulative usage for runs ending in the window (unfinished: starting there), \
         including reviewer children once. Costs are not prorated window deltas.\n\n\
         - Runs: {}\n- Reported USD: {}\n- Estimated USD (separate, not added): {}\n\
         - Unknown cost runs: {}\n- Recorded elapsed seconds: {}\n",
        total.runs,
        money(total.reported_usd),
        money(total.estimated_usd),
        total.unknown_cost_runs,
        total
            .wall_seconds
            .map(|s| s.to_string())
            .unwrap_or_else(|| "unknown".into())
    ));
    out
}

/// An inbox request logged `submitted: false` was posted and awaits its receipt; it counts once
/// `message_observed` names the same message id.
fn observed(history: &[&Event], message: &serde_json::Value) -> bool {
    message.as_str().is_some_and(|id| {
        history
            .iter()
            .any(|e| e.kind == "message_observed" && e.detail["id"] == id)
    })
}
