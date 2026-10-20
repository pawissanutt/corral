use std::collections::BTreeMap;

use anyhow::{Context, Result};
use corral_core::{CostBasis, Event, GateResult, Run, RunOutcome, Store, Task};
use jiff::Timestamp;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatsBy {
    Profile,
    Model,
    Kind,
    Project,
    Task,
    Size,
    Mode,
    Day,
}

/// Observational cohorts, not evidence of a causal lesson effect.
#[derive(Debug, Default, Serialize)]
pub struct LessonEffectiveness {
    pub exposed: LessonCohort,
    pub unexposed: LessonCohort,
    pub unknown: LessonCohort,
}

/// Outcome totals and the denominators behind each cohort's rates.
#[derive(Debug, Default, Serialize)]
pub struct LessonCohort {
    pub runs: u32,
    pub accepted: u32,
    pub mean_rounds_to_accept: Option<f64>,
    pub gate_failure_rate: Option<f64>,
    pub gate_attempts: u32,
    pub gate_failures: u32,
    pub changes_requested: u32,
    /// Sum of one plus change requests for accepted runs only.
    pub rounds_to_accept: u32,
}

#[derive(Debug, Serialize)]
pub struct StatsRow {
    #[serde(flatten)]
    pub analytics: crate::analytics::AnalyticsFacets,
    pub group: String,
    pub runs: u32,
    pub accepted: u32,
    /// First-round approvals / resolved first reviews (approval, changes, discard).
    pub first_round_accept_rate: Option<f64>,
    /// One plus change requests, averaged over approved runs only.
    pub mean_rounds_to_accept: Option<f64>,
    pub gate_failure_rate: Option<f64>,
    pub changes_requested: u32,
    pub lesson_effectiveness: LessonEffectiveness,
    pub median_wall_seconds: Option<f64>,
    pub discards: u32,
    /// Known usage only; zero is a sample, missing usage is not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub median_tokens_per_run: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_runs: Option<u32>,
    /// Known usage entries without response timestamps, including legacy aggregates.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_timing_entries: Option<u64>,
    /// Legacy reported-only costs; estimates never enter these fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub median_cost_usd_per_run: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_runs: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub median_estimated_usd_per_run: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_estimated_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_runs: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unpriced_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_basis: Option<CostBasis>,
}

#[derive(Default)]
struct Totals {
    analytics: crate::analytics::FacetTotals,
    lesson_effectiveness: LessonEffectiveness,
    runs: u32,
    accepted: u32,
    first: u32,
    reviewed: u32,
    rounds: u32,
    gates: u32,
    failures: u32,
    changes: u32,
    discards: u32,
    walls: Vec<f64>,
    tokens: Vec<f64>,
    total_tokens: u64,
    no_timing_entries: u64,
    costs: Vec<f64>,
    estimates: Vec<f64>,
    unpriced_tokens: u64,
    reported: bool,
    estimated: bool,
    unreported: bool,
}

pub(crate) fn median(samples: &mut [f64]) -> Option<f64> {
    samples.sort_by(f64::total_cmp);
    if samples.is_empty() {
        None
    } else {
        Some((samples[samples.len() / 2] + samples[(samples.len() - 1) / 2]) / 2.0)
    }
}

fn ratio(n: u32, d: u32) -> Option<f64> {
    (d != 0).then(|| f64::from(n) / f64::from(d))
}

/// Run cohort selected by dispatch time (inclusive). Events outside the cohort's
/// time window still describe those runs and are not truncated by `since`.
/// Legacy events without a run id belong to the dispatch interval containing them.
pub fn stats(store: &Store, by: StatsBy, since: Option<Timestamp>) -> Result<Vec<StatsRow>> {
    crate::usage::collect_missing(store, since)?;
    let now = Timestamp::now();
    let tasks = store.list()?;
    let events = store.events_shared()?;
    collect_stats(&tasks, &events, by, since, now)
}

/// Group of a dispatch, or of a never-dispatched task (`run = None`), whose run
/// dimensions are `not_dispatched`.
fn group_key(by: StatsBy, task: &Task, run: Option<&Run>) -> Result<String> {
    let unknown = |v: Option<&str>| v.unwrap_or("unknown").to_owned();
    Ok(match (by, run) {
        (StatsBy::Kind, _) => unknown(task.kind.as_deref()),
        (StatsBy::Project, _) => unknown(task.project.as_deref()),
        (StatsBy::Task, _) => task.id.clone(),
        (StatsBy::Size, _) => task
            .size
            .map_or_else(|| "unknown".into(), |s| s.to_string()),
        (StatsBy::Profile | StatsBy::Model | StatsBy::Mode | StatsBy::Day, None) => {
            "not_dispatched".into()
        }
        (StatsBy::Profile, Some(run)) => run.agent.clone(),
        (StatsBy::Model, Some(run)) => unknown(run.model.as_deref()),
        (StatsBy::Mode, Some(run)) => serde_json::to_string(&(
            run.model.as_deref(),
            run.effort.as_deref(),
            run.harness.as_deref(),
        ))?,
        (StatsBy::Day, Some(run)) => run
            .started
            .to_zoned(jiff::tz::TimeZone::UTC)
            .date()
            .to_string(),
    })
}

/// Shared, pure outcome and usage accounting for every analytics surface.
pub(crate) fn collect_stats(
    tasks: &[Task],
    events: &[Event],
    by: StatsBy,
    since: Option<Timestamp>,
    now: Timestamp,
) -> Result<Vec<StatsRow>> {
    let mut groups: BTreeMap<String, Totals> = BTreeMap::new();
    for task in tasks {
        let first_change = events
            .iter()
            .filter(|e| {
                e.task.as_deref() == Some(&task.id) && e.kind == "changes_requested" && e.ts <= now
            })
            .map(|e| e.ts)
            .min();
        let task_events: Vec<&Event> = events
            .iter()
            .filter(|e| e.task.as_deref() == Some(&task.id) && e.ts <= now)
            .collect();
        let spans = crate::phases::task_phases(task, &task_events);
        let eligible = |run: &Run| run.started <= now && since.is_none_or(|s| run.started >= s);
        // Task-level spans (lead, cycle, undispatched queue waits) go with the latest
        // run in this cohort as of `now`.
        let last_run = task
            .runs
            .iter()
            .filter(|r| eligible(r))
            .max_by_key(|r| r.started)
            .map(|r| &r.id);
        if last_run.is_none()
            && spans.iter().any(|s| {
                s.phase == crate::phases::Phase::Queue && since.is_none_or(|since| s.start >= since)
            })
        {
            // Never dispatched as of `now`: report its pending queue (and lead) time
            // without inventing a dispatch.
            let total = groups.entry(group_key(by, task, None)?).or_default();
            for span in spans.iter().filter(|s| s.run.is_none()) {
                total.analytics.add_span(span);
            }
        }
        for run in &task.runs {
            if !eligible(run) {
                continue;
            }
            let total = groups.entry(group_key(by, task, Some(run))?).or_default();
            total.runs += 1;
            if let Some(usage) = run.total_usage() {
                let tokens = usage.total_tokens();
                total.total_tokens = total
                    .total_tokens
                    .checked_add(tokens)
                    .context("group token total overflow")?;
                total.tokens.push(
                    serde_json::Number::from(tokens)
                        .as_f64()
                        .context("converting token count for median")?,
                );
                total.no_timing_entries = total
                    .no_timing_entries
                    .checked_add(u64::try_from(
                        usage
                            .entries
                            .iter()
                            .filter(|entry| entry.at.is_none())
                            .count(),
                    )?)
                    .context("group no-timing entry count overflow")?;
                if let Some(cost) = usage.reported_usd.or(usage.cost_usd) {
                    total.costs.push(cost);
                }
                if let Some(cost) = usage.estimated_usd {
                    total.estimates.push(cost);
                }
                total.unreported |= usage.reported_usd.or(usage.cost_usd).is_none();
                total.unpriced_tokens = total
                    .unpriced_tokens
                    .checked_add(usage.unpriced_tokens)
                    .context("group unpriced token total overflow")?;
                match usage.cost_basis {
                    Some(CostBasis::Reported) => total.reported = true,
                    Some(CostBasis::Estimated) => total.estimated = true,
                    Some(CostBasis::Mixed) => {
                        total.reported = true;
                        total.estimated = true;
                    }
                    None => {
                        total.reported |= usage.reported_usd.or(usage.cost_usd).is_some();
                        total.estimated |= usage.estimated_usd.is_some();
                    }
                }
            }
            let next_start = task
                .runs
                .iter()
                .filter(|r| r.started > run.started)
                .map(|r| r.started)
                .min();
            let related: Vec<_> = events
                .iter()
                .filter(|e| {
                    e.task.as_deref() == Some(&task.id)
                        && e.ts <= now
                        && match e.detail.get("run").and_then(|v| v.as_str()) {
                            Some(id) => id == run.id,
                            None => e.ts >= run.started && next_start.is_none_or(|end| e.ts < end),
                        }
                })
                .collect();
            total.analytics.add(task, run, &related, first_change);
            for span in spans.iter().filter(|span| match &span.run {
                Some(id) => *id == run.id,
                None => last_run == Some(&run.id),
            }) {
                total.analytics.add_span(span);
            }
            let changes = u32::try_from(
                related
                    .iter()
                    .filter(|e| e.kind == "changes_requested")
                    .count(),
            )?;
            let accepted = related.iter().any(|e| e.kind == "approved");
            let discarded = (run.outcome == Some(RunOutcome::Discarded)
                && run.ended.is_none_or(|end| end <= now))
                || related.iter().any(|e| e.kind == "discarded");
            total.changes += changes;
            total.discards += u32::from(discarded);
            if accepted || changes > 0 || discarded {
                total.reviewed += 1;
            }
            if accepted {
                total.accepted += 1;
                total.rounds += 1 + changes;
                total.first += u32::from(changes == 0);
            }
            let mut gates = 0;
            let mut failures = 0;
            for event in &related {
                if event.kind == "gate"
                    && let Some(result @ ("pass" | "fail")) = event.detail["result"].as_str()
                {
                    gates += 1;
                    failures += u32::from(result == "fail");
                }
            }
            // Old stores may retain only the latest gate snapshot.
            if gates == 0
                && let Some(gate) = run.gate.as_ref().filter(|gate| gate.at <= now)
            {
                match gate.result {
                    GateResult::Pass => gates += 1,
                    GateResult::Fail => {
                        gates += 1;
                        failures += 1;
                    }
                    GateResult::Running => {}
                }
            }
            total.gates += gates;
            total.failures += failures;
            // A snapshot can precede dispatch; use the same explicit-run attribution
            // as outcomes. Missing snapshots must not imply a known lack of lessons.
            let lessons = related.iter().rev().find_map(|event| {
                (event.kind == "lessons_applied")
                    .then(|| event.detail.get("lessons").and_then(|v| v.as_array()))
                    .flatten()
            });
            let cohort = match lessons {
                Some(lessons) if lessons.is_empty() => &mut total.lesson_effectiveness.unexposed,
                Some(_) => &mut total.lesson_effectiveness.exposed,
                None => &mut total.lesson_effectiveness.unknown,
            };
            cohort.runs += 1;
            cohort.accepted += u32::from(accepted);
            cohort.changes_requested += changes;
            if accepted {
                cohort.rounds_to_accept += 1 + changes;
            }
            cohort.gate_attempts += gates;
            cohort.gate_failures += failures;
            let done = related
                .iter()
                .filter(|e| e.kind == "done")
                .map(|e| e.ts)
                .max()
                .or_else(|| {
                    (run.outcome == Some(RunOutcome::Done))
                        .then_some(run.ended.filter(|end| *end <= now))
                        .flatten()
                });
            if let Some(end) = done.filter(|end| *end >= run.started) {
                total
                    .walls
                    .push(end.duration_since(run.started).as_secs_f64());
            }
        }
    }
    groups
        .into_iter()
        .map(|(group, mut t)| {
            for cohort in [
                &mut t.lesson_effectiveness.exposed,
                &mut t.lesson_effectiveness.unexposed,
                &mut t.lesson_effectiveness.unknown,
            ] {
                cohort.mean_rounds_to_accept = ratio(cohort.rounds_to_accept, cohort.accepted);
                cohort.gate_failure_rate = ratio(cohort.gate_failures, cohort.gate_attempts);
            }
            let usage_runs = (!t.tokens.is_empty())
                .then(|| u32::try_from(t.tokens.len()))
                .transpose()?;
            let cost_runs = (!t.costs.is_empty())
                .then(|| u32::try_from(t.costs.len()))
                .transpose()?;
            let total_tokens = usage_runs.map(|_| t.total_tokens);
            let total_cost_usd = cost_runs.map(|_| t.costs.iter().sum());
            let estimated_runs = (!t.estimates.is_empty())
                .then(|| u32::try_from(t.estimates.len()))
                .transpose()?;
            let total_estimated_usd = estimated_runs.map(|_| t.estimates.iter().sum());
            Ok(StatsRow {
                analytics: t.analytics.finish(),
                group,
                runs: t.runs,
                accepted: t.accepted,
                first_round_accept_rate: ratio(t.first, t.reviewed),
                mean_rounds_to_accept: ratio(t.rounds, t.accepted),
                gate_failure_rate: ratio(t.failures, t.gates),
                changes_requested: t.changes,
                lesson_effectiveness: t.lesson_effectiveness,
                median_wall_seconds: median(&mut t.walls),
                discards: t.discards,
                median_tokens_per_run: median(&mut t.tokens),
                total_tokens,
                usage_runs,
                no_timing_entries: usage_runs.map(|_| t.no_timing_entries),
                median_cost_usd_per_run: median(&mut t.costs),
                total_cost_usd,
                cost_runs,
                median_estimated_usd_per_run: median(&mut t.estimates),
                total_estimated_usd,
                estimated_runs,
                unpriced_tokens: usage_runs.map(|_| t.unpriced_tokens),
                cost_basis: match (t.reported, t.estimated, t.unreported) {
                    (true, _, true) | (true, true, false) => Some(CostBasis::Mixed),
                    (true, false, false) => Some(CostBasis::Reported),
                    (false, true, _) => Some(CostBasis::Estimated),
                    (false, false, _) => None,
                },
            })
        })
        .collect()
}
