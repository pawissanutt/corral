use std::fmt::Write;

use anyhow::Result;
use jiff::Timestamp;

use super::{SprintChange, SprintSections, SprintTask};

pub(super) fn render(
    s: &SprintSections,
    since: Option<Timestamp>,
    at: Timestamp,
) -> Result<String> {
    let mut out = format!(
        "# Sprint review\n\nSince: {} (exclusive)\nSnapshot: {at} (inclusive)\n\n",
        since.map_or_else(|| "all history".into(), |s| s.to_string())
    );
    writeln!(out, "## 1. Landed\n")?;
    for row in &s.landed {
        change(&mut out, row)?;
    }
    writeln!(out, "\n## 2. Sent back / failed\n")?;
    for row in &s.sent_back_failed {
        change(&mut out, row)?;
    }
    writeln!(out, "\n## 3. Dropped / rejected / escalated\n")?;
    for row in &s.dropped_rejected_escalated {
        change(&mut out, row)?;
    }
    writeln!(out, "\n## 4. In flight\n")?;
    for task in &s.in_flight {
        snapshot(&mut out, task)?;
    }
    writeln!(out, "\n## 5. Your ideas\n")?;
    for task in &s.owner_ideas {
        snapshot(&mut out, task)?;
        if let Some(decision) = &task.disposition {
            writeln!(
                out,
                "  - Disposition: {}; reason: {}; into: {}",
                decision.action,
                decision.reason,
                decision.into.as_deref().unwrap_or("-")
            )?;
        }
        for decision in &task.decisions {
            writeln!(out, "  - {}: {}", decision.kind, decision.detail)?;
        }
    }
    writeln!(out, "\n## 6. Decisions\n")?;
    for decision in &s.decisions {
        writeln!(
            out,
            "- {}: {}\n  - Requested action: {}\n  - PM recommendation: {}",
            decision.task,
            decision.question,
            decision.requested_action.as_deref().unwrap_or("unknown"),
            decision
                .pm_recommendation
                .as_deref()
                .unwrap_or("not recorded")
        )?;
    }
    writeln!(
        out,
        "\n## 7. Numbers\n\nRecorded cumulative usage for runs ending in the window, or starting there if unfinished; reviewer sessions are counted separately. Missing costs remain unknown. These are not usage deltas.\n"
    )?;
    for (label, groups) in [
        ("Task", &s.numbers.by_task),
        ("Profile", &s.numbers.by_profile),
    ] {
        writeln!(out, "### By {label}\n")?;
        for (key, n) in groups {
            writeln!(
                out,
                "- {key}: {} runs; reported USD {}; estimated USD {}; unknown cost runs {}; wall seconds {}",
                n.runs,
                n.reported_usd
                    .map_or_else(|| "unknown".into(), |v| format!("{v:.4}")),
                n.estimated_usd
                    .map_or_else(|| "unknown".into(), |v| format!("{v:.4}")),
                n.unknown_cost_runs,
                n.wall_seconds
                    .map_or_else(|| "unknown".into(), |v| v.to_string())
            )?;
        }
    }
    writeln!(
        out,
        "\nReview rounds: {}\nGate failures: {}\n\n### Lessons\n",
        s.numbers.review_rounds, s.numbers.gate_failures
    )?;
    for event in &s.numbers.lessons {
        writeln!(
            out,
            "- {} {}: {}",
            event.task.as_deref().unwrap_or("-"),
            event.kind,
            event.detail
        )?;
    }
    Ok(out)
}

fn change(out: &mut String, row: &SprintChange) -> Result<()> {
    writeln!(
        out,
        "- {} {} [{}] {}",
        row.task.as_deref().unwrap_or("-"),
        row.title.as_deref().unwrap_or("unknown"),
        row.kind,
        if row.owner_origin && row.action.is_some() {
            if row.owner_ok == Some(true) {
                "needs your OK: owner approval recorded"
            } else {
                "needs your OK"
            }
        } else if row.needs_owner_ok {
            "needs your OK"
        } else {
            ""
        }
    )?;
    writeln!(
        out,
        "  - At: {}; run: {}; SHA: {}; verdict: {}",
        row.at,
        row.run.as_deref().unwrap_or("unknown"),
        row.sha.as_deref().unwrap_or("unknown"),
        row.verdict.as_deref().unwrap_or("unknown")
    )?;
    writeln!(
        out,
        "  - Summary: {}\n  - Reason: {}\n  - Resolution: {}",
        row.summary.as_deref().unwrap_or("not recorded"),
        row.reason.as_deref().unwrap_or("not recorded"),
        row.resolution.as_deref().unwrap_or("not recorded")
    )?;
    if row.action.is_some() || row.owner_origin {
        writeln!(
            out,
            "  - Action: {}; actor: {}; owner approval: {}; merged into: {}",
            row.action.as_deref().unwrap_or("unknown"),
            row.actor.as_deref().unwrap_or("unknown"),
            row.owner_ok
                .map_or("unknown", |ok| if ok { "approved" } else { "not approved" }),
            row.into.as_deref().unwrap_or("-")
        )?;
    }
    for demo in &row.demos {
        writeln!(out, "  - Demo: `{demo}`")?;
    }
    Ok(())
}

fn snapshot(out: &mut String, task: &SprintTask) -> Result<()> {
    writeln!(
        out,
        "- [{}]({}) {} [{}]; stage: {}; progress: {}; wait: {}",
        task.task,
        task.path,
        task.title,
        task.status.as_str(),
        task.stage.as_deref().unwrap_or("unknown"),
        task.progress
            .as_ref()
            .map_or_else(|| "unknown".into(), |p| p.summary()),
        task.wait_reason.as_deref().unwrap_or("-")
    )?;
    for link in &task.links {
        writeln!(out, "  - `{link}`")?;
    }
    Ok(())
}
