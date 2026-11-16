//! Publish and report in-session builds without creating task-board entries.

use std::fmt::Write;

use anyhow::{Context, Result};
use corral_core::build_record::{BuildRecord, BuildRecordSummary, RecordUsage};

use crate::Ctx;

#[derive(serde::Serialize)]
pub struct Published {
    pub id: String,
    pub revision: u64,
}

pub fn publish(ctx: &Ctx, record_json: &str) -> Result<Published> {
    let authority = crate::assistant::owner_action::build_shell_authority(ctx)?;
    publish_authorized(ctx, record_json, &authority)
}

pub(crate) fn publish_authorized(
    ctx: &Ctx,
    record_json: &str,
    _authority: &crate::assistant::owner_action::BuildAuthority,
) -> Result<Published> {
    let (id, revision) = ctx
        .store
        .publish_build_record(record_json)
        .context("publishing in-session build record")?;
    Ok(Published { id, revision })
}

pub fn list(ctx: &Ctx) -> Result<Vec<BuildRecordSummary>> {
    ctx.store
        .list_build_records()
        .context("listing in-session build records")
}

pub fn show(ctx: &Ctx, id: &str) -> Result<BuildRecord> {
    ctx.store
        .get_build_record(id)
        .with_context(|| format!("showing build record {id}"))
}

pub fn render_list(rows: &[BuildRecordSummary]) -> String {
    let mut text = String::new();
    for row in rows {
        let _ = writeln!(
            text,
            "{}  {}{}  {}  {}  {} slices  {} review rounds  {}",
            row.id,
            row.state.as_str(),
            if row.override_reason.is_some() {
                " override"
            } else {
                ""
            },
            row.updated,
            row.title,
            row.slices,
            row.review_rounds,
            cost(row.usage_total.cost)
        );
    }
    text
}

pub fn render_show(record: &BuildRecord) -> String {
    let mut text = String::new();
    let _ = writeln!(text, "{} ({})", record.title, record.id);
    let _ = write!(text, "Outcome: {}", record.outcome.state.as_str());
    if let Some(at) = &record.outcome.at {
        let _ = write!(text, " at {at}");
    }
    if let Some(head) = &record.outcome.head {
        let _ = write!(text, " | head {head}");
    }
    if let Some(landed) = &record.outcome.landed {
        let _ = write!(text, " | landed {landed}");
    }
    text.push('\n');

    if !record.intent.is_empty() {
        text.push_str("\nIntent\n");
        for prompt in &record.intent {
            let _ = writeln!(text, "[{}] {}", prompt.at, prompt.text);
        }
    }
    if !record.slices.is_empty() {
        text.push_str("\nSlices\nID | Kind | Model | State | Duration | Tokens (in/out) | Cost | Outside ownership\n");
        for slice in &record.slices {
            let _ = writeln!(
                text,
                "{} | {} | {} | {} | {} | {}/{} | {} | {}",
                slice.id,
                slice.kind.as_str(),
                slice.model,
                slice.state.as_str(),
                duration(slice.started.as_deref(), slice.ended.as_deref()),
                slice.usage.usage.input,
                slice.usage.usage.output,
                cost(slice.usage.usage.cost),
                slice.outside.len()
            );
        }
    }
    if !record.verification.is_empty() {
        text.push_str("\nVerification\nAttempt | Tier | Result | Failed command | Duration\n");
        for attempt in &record.verification {
            let _ = writeln!(
                text,
                "{} | {} | {} | {} | {}",
                attempt.attempt,
                attempt.tier,
                attempt.result.as_str(),
                attempt.failed_command.as_deref().unwrap_or("-"),
                duration(Some(&attempt.started), Some(&attempt.ended))
            );
        }
    }
    if !record.reviews.is_empty() {
        text.push_str("\nReview rounds\n");
        for review in &record.reviews {
            let _ = writeln!(
                text,
                "Round {} | {} ({}) | {}",
                review.round,
                review.model,
                review.reviewer,
                review.verdict.as_str()
            );
            if let Some(summary) = &review.summary {
                let _ = writeln!(text, "{summary}");
            }
            for finding in &review.findings {
                let _ = write!(
                    text,
                    "  {} [{}] {}",
                    finding.id,
                    finding.severity.as_str(),
                    finding.path.as_deref().unwrap_or("-")
                );
                if let Some(line) = finding.line {
                    let _ = write!(text, ":{line}");
                }
                let _ = write!(text, ": {} | resolution: ", finding.text);
                if let Some(resolution) = &finding.resolution {
                    text.push_str(resolution.state.as_str());
                    if let Some(commit) = &resolution.commit {
                        let _ = write!(text, " ({commit})");
                    }
                    if let Some(reason) = &resolution.reason {
                        let _ = write!(text, " - {reason}");
                    }
                } else {
                    text.push_str("open");
                }
                text.push('\n');
            }
        }
    }
    if !record.messages.is_empty() {
        let _ = writeln!(text, "\nCoordination ({} messages)", record.messages.len());
        for message in record
            .messages
            .iter()
            .skip(record.messages.len().saturating_sub(20))
        {
            let _ = writeln!(text, "{} -> {}: {}", message.from, message.to, message.text);
        }
    }
    if !record.decisions.is_empty() {
        text.push_str("\nOwner decisions\n");
        for decision in &record.decisions {
            let _ = writeln!(
                text,
                "[{}] {}: {}",
                decision.at,
                decision.kind.as_str(),
                decision.text
            );
            if let Some(answer) = &decision.answer {
                let _ = writeln!(text, "  Answer: {answer}");
            }
        }
    }
    text.push_str("\nUsage totals\nRole | Input | Output | Cost\n");
    for (role, usage) in [
        ("Parent", &record.usage.parent),
        ("Children", &record.usage.children),
        ("Reviewers", &record.usage.reviewers),
        ("Total", &record.usage.total),
    ] {
        render_usage(&mut text, role, usage);
    }
    text
}

fn cost(value: Option<f64>) -> String {
    value.map_or_else(|| "-".into(), |value| format!("${value:.4}"))
}

fn duration(started: Option<&str>, ended: Option<&str>) -> String {
    let interval = started.zip(ended).and_then(|(started, ended)| {
        let started = started.parse::<jiff::Timestamp>().ok()?;
        let ended = ended.parse::<jiff::Timestamp>().ok()?;
        let milliseconds = ended.as_millisecond() - started.as_millisecond();
        (milliseconds >= 0).then_some(milliseconds)
    });
    interval.map_or_else(
        || "-".into(),
        |ms| format!("{}.{:03}s", ms / 1000, ms % 1000),
    )
}

fn render_usage(text: &mut String, role: &str, usage: &RecordUsage) {
    let _ = writeln!(
        text,
        "{role} | {} | {} | {}",
        usage.input,
        usage.output,
        cost(usage.cost)
    );
}

#[cfg(test)]
#[path = "record_tests.rs"]
mod tests;
