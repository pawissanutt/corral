use std::collections::BTreeMap;

use anyhow::Result;
use corral_core::{GateResult, RunOutcome, Store};
use jiff::Timestamp;
use serde::Serialize;

#[derive(Debug, Clone, Copy)]
pub enum StatsBy {
    Profile,
    Model,
    Kind,
    Project,
}

#[derive(Debug, Serialize)]
pub struct StatsRow {
    pub group: String,
    pub runs: u32,
    pub accepted: u32,
    /// First-round approvals / resolved first reviews (approval, changes, discard).
    pub first_round_accept_rate: Option<f64>,
    /// One plus change requests, averaged over approved runs only.
    pub mean_rounds_to_accept: Option<f64>,
    pub gate_failure_rate: Option<f64>,
    pub changes_requested: u32,
    pub median_wall_seconds: Option<f64>,
    pub discards: u32,
}

#[derive(Default)]
struct Totals {
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
}

fn ratio(n: u32, d: u32) -> Option<f64> {
    (d != 0).then(|| f64::from(n) / f64::from(d))
}

/// Run cohort selected by dispatch time (inclusive). Events outside the cohort's
/// time window still describe those runs and are not truncated by `since`.
/// Legacy events without a run id belong to the dispatch interval containing them.
pub fn stats(store: &Store, by: StatsBy, since: Option<Timestamp>) -> Result<Vec<StatsRow>> {
    let events = store.events()?;
    let mut groups: BTreeMap<String, Totals> = BTreeMap::new();
    for task in store.list()? {
        for run in &task.runs {
            if since.is_some_and(|s| run.started < s) {
                continue;
            }
            let key = match by {
                StatsBy::Profile => Some(run.agent.as_str()),
                StatsBy::Model => run.model.as_deref(),
                StatsBy::Kind => task.kind.as_deref(),
                StatsBy::Project => task.project.as_deref(),
            };
            let total = groups
                .entry(key.unwrap_or("unknown").to_owned())
                .or_default();
            total.runs += 1;
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
                        && match e.detail.get("run").and_then(|v| v.as_str()) {
                            Some(id) => id == run.id,
                            None => e.ts >= run.started && next_start.is_none_or(|end| e.ts < end),
                        }
                })
                .collect();
            let changes = u32::try_from(
                related
                    .iter()
                    .filter(|e| e.kind == "changes_requested")
                    .count(),
            )?;
            let accepted = related.iter().any(|e| e.kind == "approved");
            let discarded = run.outcome == Some(RunOutcome::Discarded)
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
            for event in &related {
                if event.kind == "gate"
                    && let Some(result @ ("pass" | "fail")) = event.detail["result"].as_str()
                {
                    gates += 1;
                    total.failures += u32::from(result == "fail");
                }
            }
            // Old stores may retain only the latest gate snapshot.
            if gates == 0
                && let Some(gate) = &run.gate
            {
                match gate.result {
                    GateResult::Pass => gates += 1,
                    GateResult::Fail => {
                        gates += 1;
                        total.failures += 1;
                    }
                    GateResult::Running => {}
                }
            }
            total.gates += gates;
            let done = related
                .iter()
                .filter(|e| e.kind == "done")
                .map(|e| e.ts)
                .max()
                .or_else(|| {
                    (run.outcome == Some(RunOutcome::Done))
                        .then_some(run.ended)
                        .flatten()
                });
            if let Some(end) = done.filter(|end| *end >= run.started) {
                total
                    .walls
                    .push(end.duration_since(run.started).as_secs_f64());
            }
        }
    }
    Ok(groups
        .into_iter()
        .map(|(group, mut t)| {
            t.walls.sort_by(f64::total_cmp);
            let median = if t.walls.is_empty() {
                None
            } else {
                let middle = t.walls.len() / 2;
                Some((t.walls[middle] + t.walls[(t.walls.len() - 1) / 2]) / 2.0)
            };
            StatsRow {
                group,
                runs: t.runs,
                accepted: t.accepted,
                first_round_accept_rate: ratio(t.first, t.reviewed),
                mean_rounds_to_accept: ratio(t.rounds, t.accepted),
                gate_failure_rate: ratio(t.failures, t.gates),
                changes_requested: t.changes,
                median_wall_seconds: median,
                discards: t.discards,
            }
        })
        .collect())
}
