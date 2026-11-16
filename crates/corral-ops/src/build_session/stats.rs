//! Build-record statistics, deliberately separate from task/run cohorts.

use std::fmt::Write;

use anyhow::{Context, Result};
use corral_core::build_record::{BuildRecord, GateResult, OutcomeState, ReviewVerdict};
use jiff::Timestamp;

use crate::Ctx;

#[derive(Default, serde::Serialize)]
pub struct BuildStats {
    pub builds: u64,
    pub landed: u64,
    pub kept: u64,
    pub abandoned: u64,
    pub open: u64,
    pub slices: u64,
    /// Mean over builds with at least one review round.
    pub mean_review_rounds: Option<f64>,
    /// Fraction of reviewed builds whose numbered first round approved.
    pub first_round_approve_rate: Option<f64>,
    pub gate_attempts: u64,
    /// Failed attempts / completed (pass + fail) attempts; skipped attempts are excluded.
    pub gate_failure_rate: Option<f64>,
    /// Created-to-outcome duration, for landed builds with a recorded outcome time.
    pub median_wall_seconds: Option<f64>,
    pub messages: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Unknown for an empty cohort or if any counted build lacks a total cost.
    pub cost_usd: Option<f64>,
}

/// Collect records created at or after `since`, without reading task or event state.
/// Stored records already have validated timestamps; absent outcome times remain unknown.
pub fn collect(records: &[BuildRecord], since: Option<Timestamp>) -> BuildStats {
    let mut result = BuildStats::default();
    let mut reviewed = 0;
    let mut review_rounds = 0;
    let mut first_approved = 0;
    let mut completed_gates = 0;
    let mut failed_gates = 0;
    let mut wall_seconds = Vec::new();
    let mut cost = Some(0.0);
    for record in records {
        let created = (since.is_some() || record.outcome.state == OutcomeState::Landed)
            .then(|| record.created.parse::<Timestamp>().ok())
            .flatten();
        if since.is_some_and(|since| created.is_none_or(|created| created < since)) {
            continue;
        }
        result.builds += 1;
        match record.outcome.state {
            OutcomeState::Landed => {
                result.landed += 1;
                if let Some((created, ended)) = created.zip(
                    record
                        .outcome
                        .at
                        .as_deref()
                        .and_then(|at| at.parse::<Timestamp>().ok()),
                ) && ended >= created
                {
                    wall_seconds.push(
                        (ended.as_nanosecond() - created.as_nanosecond()) as f64 / 1_000_000_000.0,
                    );
                }
            }
            OutcomeState::Kept => result.kept += 1,
            OutcomeState::Abandoned => result.abandoned += 1,
            OutcomeState::Open => result.open += 1,
        }
        result.slices += record.slices.len() as u64;
        if !record.reviews.is_empty() {
            reviewed += 1;
            review_rounds += record.reviews.len() as u64;
            if record
                .reviews
                .iter()
                .any(|round| round.round == 1 && round.verdict == ReviewVerdict::Approve)
            {
                first_approved += 1;
            }
        }
        result.gate_attempts += record.verification.len() as u64;
        for attempt in &record.verification {
            match attempt.result {
                GateResult::Pass => completed_gates += 1,
                GateResult::Fail => {
                    completed_gates += 1;
                    failed_gates += 1;
                }
                GateResult::Skipped => {}
            }
        }
        result.messages += record.messages.len() as u64;
        result.input_tokens += record.usage.total.input;
        result.output_tokens += record.usage.total.output;
        cost = cost
            .zip(record.usage.total.cost)
            .map(|(total, next)| total + next);
    }
    result.mean_review_rounds = ratio(review_rounds, reviewed);
    result.first_round_approve_rate = ratio(first_approved, reviewed);
    result.gate_failure_rate = ratio(failed_gates, completed_gates);
    wall_seconds.sort_by(f64::total_cmp);
    if !wall_seconds.is_empty() {
        let middle = wall_seconds.len() / 2;
        result.median_wall_seconds = Some(if wall_seconds.len() % 2 == 0 {
            (wall_seconds[middle - 1] + wall_seconds[middle]) / 2.0
        } else {
            wall_seconds[middle]
        });
    }
    result.cost_usd = (result.builds > 0).then_some(cost).flatten();
    result
}

fn ratio(numerator: u64, denominator: u64) -> Option<f64> {
    (denominator > 0).then(|| numerator as f64 / denominator as f64)
}

/// Load tracked build records. `since` uses the same N h/d/w age grammar as task stats.
pub fn stats(ctx: &Ctx, since: Option<&str>) -> Result<BuildStats> {
    let since = since
        .map(|value| {
            let age = corral_core::parse_age(value).context("--since")?;
            Timestamp::now()
                .checked_sub(age)
                .context("--since is out of range")
        })
        .transpose()?;
    let records = super::record::list(ctx)?
        .into_iter()
        .map(|row| super::record::show(ctx, &row.id))
        .collect::<Result<Vec<_>>>()?;
    Ok(collect(&records, since))
}

/// Render every metric; unavailable values are `-`, not inferred zeroes.
pub fn render(stats: &BuildStats) -> String {
    let mut text = String::from("Build statistics\n");
    for (label, value) in [
        ("Builds", stats.builds),
        ("Landed", stats.landed),
        ("Kept", stats.kept),
        ("Abandoned", stats.abandoned),
        ("Open", stats.open),
        ("Slices", stats.slices),
        ("Gate attempts", stats.gate_attempts),
        ("Messages", stats.messages),
        ("Input tokens", stats.input_tokens),
        ("Output tokens", stats.output_tokens),
    ] {
        let _ = writeln!(text, "{label}: {value}");
    }
    for (label, value) in [
        ("Mean review rounds", stats.mean_review_rounds),
        ("First-round approve rate", stats.first_round_approve_rate),
        ("Gate failure rate", stats.gate_failure_rate),
        ("Median wall seconds", stats.median_wall_seconds),
        ("Cost USD", stats.cost_usd),
    ] {
        match value {
            Some(value) => {
                let _ = writeln!(text, "{label}: {value:.4}");
            }
            None => {
                let _ = writeln!(text, "{label}: -");
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(id: &str, state: &str, cost: Option<f64>) -> BuildRecord {
        serde_json::from_value(json!({
            "version": 1, "id": id, "kind": "build", "title": id,
            "created": "2026-01-01T00:00:00Z", "updated": "2026-01-01T00:10:00Z",
            "repo_root": "/unused", "worktree": "/unused", "branch": null, "base": null,
            "intent": [], "outcome": {"state": state}, "commits": [], "slices": [],
            "verification": [], "reviews": [], "messages": [], "decisions": [],
            "usage": {
                "parent": {"input": 0, "output": 0},
                "children": {"input": 0, "output": 0},
                "reviewers": {"input": 0, "output": 0},
                "total": {"input": 10, "output": 5, "cost": cost}
            },
            "timeline": [], "transcripts": {"children": {}}
        }))
        .unwrap()
    }

    #[test]
    fn build_cohort_counts_reviews_completed_gates_and_unknown_cost_without_tasks() {
        let mut landed = record("B-landed", "landed", Some(1.0));
        landed.outcome.at = Some("2026-01-01T00:10:00Z".into());
        landed.outcome.landed = Some("abc".into());
        landed.slices = serde_json::from_value(json!([{
            "id": "S1", "title": "slice", "kind": "slice", "owns": [], "model": "worker",
            "state": "done", "usage": {"input": 0, "output": 0, "turns": 0}, "outside": []
        }]))
        .unwrap();
        landed.reviews = serde_json::from_value(json!([
            {"round": 2, "reviewer": "V2", "model": "reviewer", "head": "abc",
                "verdict": "approve", "at": "2026-01-01T00:09:00Z", "findings": []},
            {"round": 1, "reviewer": "V1", "model": "reviewer", "head": "abc",
                "verdict": "changes", "at": "2026-01-01T00:08:00Z", "findings": []}
        ]))
        .unwrap();
        landed.verification = serde_json::from_value(json!([
            {"attempt": "G1", "tier": "quick", "head": "abc", "result": "fail",
                "started": "2026-01-01T00:01:00Z", "ended": "2026-01-01T00:02:00Z"},
            {"attempt": "G2", "tier": "quick", "head": "abc", "result": "pass",
                "started": "2026-01-01T00:03:00Z", "ended": "2026-01-01T00:04:00Z"}
        ]))
        .unwrap();
        landed.messages = serde_json::from_value(json!([{
            "at": "2026-01-01T00:01:00Z", "from": "S1", "to": "parent",
            "text": "ready", "delivery": "delivered"
        }]))
        .unwrap();
        let kept = record("B-kept", "kept", None);
        let open = record("B-open", "open", Some(0.0));
        let records = [landed, kept, open];
        let stats = collect(&records, None);
        assert_eq!(
            serde_json::to_value(&stats).unwrap(),
            json!({
                "builds": 3, "landed": 1, "kept": 1, "abandoned": 0, "open": 1,
                "slices": 1, "mean_review_rounds": 2.0, "first_round_approve_rate": 0.0,
                "gate_attempts": 2, "gate_failure_rate": 0.5, "median_wall_seconds": 600.0,
                "messages": 1, "input_tokens": 30, "output_tokens": 15, "cost_usd": null
            })
        );
        let unreviewed = collect(&records[1..], None);
        assert_eq!(unreviewed.mean_review_rounds, None);
        assert_eq!(unreviewed.first_round_approve_rate, None);
        assert_eq!(unreviewed.gate_failure_rate, None);
        assert_eq!(unreviewed.median_wall_seconds, None);
        assert_eq!(unreviewed.cost_usd, None);
        let since = records[0].created.parse::<Timestamp>().unwrap();
        assert_eq!(collect(&records, Some(since)).builds, 3);
        let empty = collect(
            &records,
            Some("2026-01-01T00:00:00.000000001Z".parse().unwrap()),
        );
        assert_eq!(empty.builds, 0);
        assert_eq!(empty.cost_usd, None);
    }

    #[test]
    fn skipped_gates_do_not_dilute_failure_rate_and_landed_durations_use_median() {
        let mut first = record("B-first", "landed", Some(1.0));
        first.outcome.at = Some("2026-01-01T00:00:10.5Z".into());
        first.verification = serde_json::from_value(json!([
            {"attempt": "G1", "tier": "quick", "head": "abc", "result": "fail",
                "started": "2026-01-01T00:00:00Z", "ended": "2026-01-01T00:00:01Z"},
            {"attempt": "G2", "tier": "quick", "head": "abc", "result": "skipped",
                "started": "2026-01-01T00:00:00Z", "ended": "2026-01-01T00:00:01Z"}
        ]))
        .unwrap();
        first.reviews = serde_json::from_value(json!([{
            "round": 1, "reviewer": "V1", "model": "reviewer", "head": "abc",
            "verdict": "approve", "at": "2026-01-01T00:00:00Z", "findings": []
        }]))
        .unwrap();
        let mut second = record("B-second", "landed", Some(2.0));
        second.outcome.at = Some("2026-01-01T00:00:30.5Z".into());
        let stats = collect(&[first, second], None);
        assert_eq!(stats.gate_attempts, 2);
        assert_eq!(stats.gate_failure_rate, Some(1.0));
        assert_eq!(stats.first_round_approve_rate, Some(1.0));
        assert_eq!(stats.median_wall_seconds, Some(20.5));
        assert_eq!(stats.cost_usd, Some(3.0));
    }
}
