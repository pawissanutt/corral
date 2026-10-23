//! Gate-level analytics behind `corral stats --gates`.
//!
//! Tier is a property of each gate, and one task has gates of several tiers, so this is a
//! report over gates rather than a [`crate::StatsBy`] grouping. [`collect_gate_metrics`] is
//! pure over tasks, events and [`GateSources`]; [`gate_stats`] supplies the store, gate logs
//! and Git.
//!
//! Pairing rules (kept equal to the T-251 baseline script `measure_gates.py`):
//! - A `gate_started` opens a gate. A `gate` event closes the start with the same
//!   `identity.attempt`, otherwise the latest open start of the same task and run; a `gate`
//!   event with no open start counts as unmatched.
//! - A finished gate is `pass`, `stale_result`, `invalid_receipts` or `command_failed`, by its
//!   result and `failed_cmd` prefix. Duration is start to finish.
//! - A gate is followed by a rebase when its run's next `candidate_rebased`, `review_started`,
//!   `done`, `approved` or `gate_started` inside the window is `candidate_rebased`.
//! - Behind at start: the gate's `identity.integration_base` is not an ancestor of
//!   `identity.head` in the task's project repository; Git errors (pruned commits, unknown
//!   repository) count as unknown, and a start without identity as `no_identity`.
//!
//! Differences from the baseline scripts, by design:
//! - A landing is an `approved` event that moved a ref; `keep_branch` approvals do not land.
//! - Review rounds count `verdict` events, not started reviews paired to verdicts.
//! - Behind-at-start hours use this module's gate pairing; `measure_behind.py` pairs each
//!   start with the next `gate` event of its run and checks every gate in one repository.
//!
//! A destination is `<project>:<branch>`: the Plan target branch, else the run's base.
//!
//! Events without a `tier` (all gates before tiers existed) count as tier `full`. Full-gate
//! step seconds come from the `<== <step> <seconds>s` markers in each attempt's gate log, and
//! from the gate's timed command receipts when the log has none.
use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::Result;
use corral_core::{Actor, Config, Event, GateRules, Run, Store, Task};
use jiff::Timestamp;
use serde::Serialize;
use serde_json::Value;

pub const GATE_METRICS_SCHEMA_VERSION: u32 = 1;

/// The tier of every gate recorded without one.
pub const FULL_TIER: &str = "full";

/// Autopilot's `autopilot` event note when it adopts the run's existing gate receipt.
pub const AUTOPILOT_REUSE_NOTE: &str = "reusing the gate run on this commit";

type RunKey = (String, Option<String>);

const NEXT: [&str; 5] = [
    "candidate_rebased",
    "review_started",
    "done",
    "approved",
    "gate_started",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Cohort {
    /// A tiered run that lands on an owned Plan integration target.
    PlanMember,
    /// A tiered run whose base is the project's main branch.
    MainBoundTiered,
    /// A tiered run to any other base; it keeps full-gate rules.
    TieredOtherBase,
    /// Legacy runs (dispatched before tiers or in untiered projects) and gates whose run is
    /// no longer recorded.
    LegacyOrUntiered,
}

/// Whether a gate's candidate already contained its destination when the gate started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Behind {
    Behind,
    Contained,
    Unknown,
    NoIdentity,
}

/// One `gate_started` with what the window says about it.
#[derive(Debug, Clone)]
struct GateRow {
    task: String,
    run: Option<String>,
    attempt: Option<String>,
    start: Timestamp,
    end: Option<Timestamp>,
    class: Option<&'static str>,
    tier: String,
    cohort: Cohort,
    destination: String,
    worker: bool,
    reused: bool,
    followed_by_rebase: bool,
    behind: Behind,
    steps: Vec<(String, f64)>,
}

impl GateRow {
    fn seconds(&self) -> Option<f64> {
        self.end
            .filter(|end| *end >= self.start)
            .map(|end| end.duration_since(self.start).as_secs_f64())
    }
}

#[derive(Debug, Default, Serialize)]
pub struct Count {
    pub gates: u32,
    pub hours: f64,
}

impl Count {
    fn add(&mut self, hours: f64) {
        self.gates += 1;
        self.hours += hours;
    }
}

#[derive(Debug, Default, Serialize)]
pub struct BehindCounts {
    pub behind_at_start: Count,
    pub behind_at_start_followed_by_rebase: Count,
    pub contained_base: Count,
    pub contained_base_followed_by_rebase: Count,
    pub unknown: Count,
    pub no_identity: Count,
}

#[derive(Debug, Serialize)]
pub struct StepSeconds {
    pub samples: u32,
    pub total_seconds: f64,
    pub median_seconds: Option<f64>,
}

/// The same figures for the whole window and for each cohort, tier and destination.
#[derive(Debug, Default, Serialize)]
pub struct GateFigures {
    pub gates_started: u32,
    pub gates_finished: u32,
    pub gates_unfinished: u32,
    /// Finished gates by class: pass, command_failed, stale_result, invalid_receipts.
    pub classes: BTreeMap<&'static str, u32>,
    pub gate_hours: f64,
    pub stale_result_hours: f64,
    /// stale_result gates over finished gates.
    pub stale_result_share: Option<f64>,
    pub landed_tasks: u32,
    /// Finished gates of landed tasks over landed tasks.
    pub gates_per_landed_task: Option<f64>,
    pub gate_hours_per_landed_task: Option<f64>,
    pub review_rounds: u32,
    pub gates_per_review_round: Option<f64>,
    pub followed_by_rebase: u32,
    pub behind: BehindCounts,
    /// Gates a worker started (`corral gate` by an agent).
    pub worker_started: u32,
    /// Worker-started receipts autopilot adopted instead of starting its own gate.
    pub worker_receipts_reused_by_autopilot: u32,
    /// Per step of finished full-tier gates: log step markers, else timed command receipts.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub full_step_seconds: BTreeMap<String, StepSeconds>,
}

#[derive(Debug, Serialize)]
pub struct GateGroup {
    pub cohort: Cohort,
    pub tier: String,
    pub destination: String,
    #[serde(flatten)]
    pub figures: GateFigures,
}

#[derive(Debug, Default, Serialize)]
pub struct CarryCounts {
    pub gate: BTreeMap<String, u32>,
    pub verdict: BTreeMap<String, u32>,
    pub evidence: BTreeMap<String, u32>,
}

/// Carries of one cohort and destination from one tier to another. An event without
/// `from_tier`/`to_tier` uses its `tier`, and an event without any tier (every carry
/// before tiers existed) counts as `full` to `full`.
#[derive(Debug, Serialize)]
pub struct CarryGroup {
    pub cohort: Cohort,
    pub from_tier: String,
    pub to_tier: String,
    pub destination: String,
    #[serde(flatten)]
    pub carries: CarryCounts,
}

#[derive(Debug, Serialize)]
pub struct GateMetrics {
    pub schema_version: u32,
    pub since: Option<Timestamp>,
    pub until: Option<Timestamp>,
    /// `gate` events with no open start to close.
    pub gate_events_unmatched: u32,
    pub totals: GateFigures,
    pub groups: Vec<GateGroup>,
    pub carries: Vec<CarryGroup>,
}

/// What the pure collector needs from outside the event log.
pub trait GateSources {
    /// Whether `base` is an ancestor of `head` in the task's repository; `None` if unknown.
    fn contains(&mut self, task: &Task, base: &str, head: &str) -> Option<bool>;
    /// The `<== <step> <seconds>s` markers of one gate attempt's log, in order; `None` when
    /// the log is gone or has no markers, so the gate's command receipts stand in.
    fn steps(&mut self, task: &str, run: &str, attempt: &str) -> Option<Vec<(String, f64)>>;
}

/// The step markers `scripts/gate.sh` writes after each finished step, failed ones included.
pub fn parse_step_markers(log: &[u8]) -> Vec<(String, f64)> {
    log.split(|b| *b == b'\n')
        .filter_map(|line| {
            let line = std::str::from_utf8(line).ok()?.trim_end_matches('\r');
            let (step, seconds) = line.strip_prefix("<== ")?.rsplit_once(' ')?;
            let seconds: f64 = seconds.strip_suffix('s')?.parse().ok()?;
            (!step.is_empty() && seconds.is_finite() && seconds >= 0.0)
                .then(|| (step.to_owned(), seconds))
        })
        .collect()
}

struct StoreSources<'a> {
    store: &'a Store,
    ancestry: HashMap<(String, String, String), Option<bool>>,
}

impl GateSources for StoreSources<'_> {
    fn contains(&mut self, task: &Task, base: &str, head: &str) -> Option<bool> {
        let project = task.project.clone().unwrap_or_default();
        let store = self.store;
        *self
            .ancestry
            .entry((project, base.to_owned(), head.to_owned()))
            .or_insert_with(|| {
                let repo = store.project_path(task.project.as_deref()?).ok()?;
                corral_git::is_ancestor(&repo, base, head).ok()
            })
    }

    fn steps(&mut self, task: &str, run: &str, attempt: &str) -> Option<Vec<(String, f64)>> {
        let path = self
            .store
            .state_dir()
            .join("gates")
            .join(format!("{task}-{run}-{attempt}.log"));
        let steps = parse_step_markers(&std::fs::read(path).ok()?);
        (!steps.is_empty()).then_some(steps)
    }
}

/// Reads the store's tasks, events and gate logs, and asks each task's project repository
/// whether a gate's destination was an ancestor of its head. Read-only.
pub fn gate_stats(
    store: &Store,
    since: Option<Timestamp>,
    until: Option<Timestamp>,
) -> Result<GateMetrics> {
    let tasks = store.list_shared()?;
    let events = store.events_shared()?;
    let mut sources = StoreSources {
        store,
        ancestry: HashMap::new(),
    };
    Ok(collect_gate_metrics(
        &store.config,
        &tasks,
        &events,
        since,
        until,
        &mut sources,
    ))
}

type GroupKey = (Cohort, String, String);

/// Pure gate report over tasks and events, with Git ancestry and gate logs from `sources`.
///
/// A landing or verdict inside the window belongs to its run's cohort and destination and to
/// the tier of the run's latest gate before it, wherever that gate started (`full` when the
/// run has none), so groups keep landings whose gates precede the window.
pub fn collect_gate_metrics(
    config: &Config,
    tasks: &[Task],
    events: &[Event],
    since: Option<Timestamp>,
    until: Option<Timestamp>,
    sources: &mut dyn GateSources,
) -> GateMetrics {
    let by_id: HashMap<&str, &Task> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();
    let in_window = |e: &Event| since.is_none_or(|s| e.ts >= s) && until.is_none_or(|u| e.ts <= u);
    let mut rows: Vec<GateRow> = Vec::new();
    let mut open_by_attempt: HashMap<String, usize> = HashMap::new();
    let mut open_by_run: HashMap<RunKey, Vec<usize>> = HashMap::new();
    let mut timeline: HashMap<RunKey, Vec<(&str, Option<usize>)>> = HashMap::new();
    let mut last_tier: HashMap<RunKey, String> = HashMap::new();
    let mut unmatched = 0;
    let mut landed: BTreeSet<String> = BTreeSet::new();
    let mut verdicts = 0u32;
    let mut group_landed: BTreeMap<GroupKey, BTreeSet<String>> = BTreeMap::new();
    let mut group_verdicts: BTreeMap<GroupKey, u32> = BTreeMap::new();
    let mut carries: BTreeMap<(Cohort, String, String, String), CarryCounts> = BTreeMap::new();

    for event in events {
        let Some(task_id) = event.task.as_deref() else {
            continue;
        };
        let detail = &event.detail;
        let run_id = detail["run"].as_str().map(str::to_owned);
        let key = (task_id.to_owned(), run_id.clone());
        let tier = detail["tier"].as_str().unwrap_or(FULL_TIER).to_owned();
        if !in_window(event) {
            if event.kind == "gate_started" {
                last_tier.insert(key, tier);
            }
            continue;
        }
        let task = by_id.get(task_id).copied();
        let run = task
            .zip(run_id.as_deref())
            .and_then(|(t, r)| find_run(t, r));
        let (cohort, destination) = classify_run(config, task, run);
        let group = |last_tier: &HashMap<RunKey, String>| {
            let tier = last_tier.get(&key).map_or(FULL_TIER, String::as_str);
            (cohort, tier.to_owned(), destination.clone())
        };
        match event.kind.as_str() {
            "gate_started" => {
                let identity = &detail["identity"];
                let behind = match (
                    task,
                    identity["integration_base"].as_str(),
                    identity["head"].as_str(),
                ) {
                    (_, None, _) | (_, _, None) => Behind::NoIdentity,
                    (None, _, _) => Behind::Unknown,
                    (Some(task), Some(base), Some(head)) => {
                        match sources.contains(task, base, head) {
                            Some(true) => Behind::Contained,
                            Some(false) => Behind::Behind,
                            None => Behind::Unknown,
                        }
                    }
                };
                let attempt = identity["attempt"].as_str().map(str::to_owned);
                let index = rows.len();
                last_tier.insert(key.clone(), tier.clone());
                rows.push(GateRow {
                    task: task_id.to_owned(),
                    run: run_id,
                    attempt: attempt.clone(),
                    start: event.ts,
                    end: None,
                    class: None,
                    tier,
                    cohort,
                    destination,
                    worker: event.actor == Some(Actor::Agent),
                    reused: false,
                    followed_by_rebase: false,
                    behind,
                    steps: Vec::new(),
                });
                if let Some(attempt) = attempt {
                    open_by_attempt.insert(attempt, index);
                }
                open_by_run.entry(key.clone()).or_default().push(index);
                timeline
                    .entry(key)
                    .or_default()
                    .push(("gate_started", Some(index)));
            }
            "gate" => {
                let by_attempt = detail["identity"]["attempt"]
                    .as_str()
                    .and_then(|a| open_by_attempt.remove(a));
                let index = by_attempt.or_else(|| {
                    open_by_run
                        .get(&key)
                        .and_then(|stack| stack.iter().rev().find(|i| rows[**i].end.is_none()))
                        .copied()
                });
                let Some(index) = index else {
                    unmatched += 1;
                    continue;
                };
                let row = &mut rows[index];
                if let Some(attempt) = row.attempt.as_deref()
                    && by_attempt.is_none()
                {
                    open_by_attempt.remove(attempt);
                }
                row.end = Some(event.ts);
                row.class = Some(gate_class(detail));
                if row.tier == FULL_TIER {
                    row.steps = row
                        .run
                        .as_deref()
                        .zip(row.attempt.as_deref())
                        .and_then(|(run, attempt)| sources.steps(&row.task, run, attempt))
                        .unwrap_or_else(|| receipt_steps(event));
                }
                timeline.entry(key).or_default().push(("gate", None));
            }
            kind @ ("candidate_rebased" | "review_started" | "done") => {
                timeline.entry(key).or_default().push((kind, None));
            }
            "approved" => {
                if detail["strategy"] != "keep_branch" {
                    landed.insert(task_id.to_owned());
                    group_landed
                        .entry(group(&last_tier))
                        .or_default()
                        .insert(task_id.to_owned());
                }
                timeline.entry(key).or_default().push(("approved", None));
            }
            "verdict" => {
                verdicts += 1;
                *group_verdicts.entry(group(&last_tier)).or_default() += 1;
            }
            "autopilot" if detail["note"] == AUTOPILOT_REUSE_NOTE => {
                if let Some(index) = open_by_run.get(&key).and_then(|stack| stack.last())
                    && rows[*index].worker
                {
                    rows[*index].reused = true;
                }
            }
            kind @ ("gate_carried" | "verdict_carried") => {
                let from = detail["from_tier"].as_str().unwrap_or(&tier).to_owned();
                let to = detail["to_tier"].as_str().unwrap_or(&tier).to_owned();
                let counts = carries.entry((cohort, from, to, destination)).or_default();
                let method = detail["method"]
                    .as_str()
                    .unwrap_or("clean_replay")
                    .to_owned();
                let side = if kind == "gate_carried" {
                    &mut counts.gate
                } else {
                    &mut counts.verdict
                };
                *side.entry(method).or_default() += 1;
                if let Some(evidence) = detail["evidence_carry"]["method"].as_str() {
                    *counts.evidence.entry(evidence.to_owned()).or_default() += 1;
                }
            }
            _ => {}
        }
    }

    for items in timeline.values() {
        for (i, (kind, index)) in items.iter().enumerate() {
            if *kind != "gate_started" {
                continue;
            }
            let next = items[i + 1..]
                .iter()
                .map(|(k, _)| *k)
                .find(|k| NEXT.contains(k));
            if let (Some(index), Some("candidate_rebased")) = (index, next) {
                rows[*index].followed_by_rebase = true;
            }
        }
    }

    let totals = figures(rows.iter(), &landed, verdicts);
    let mut grouped: BTreeMap<GroupKey, Vec<&GateRow>> = BTreeMap::new();
    for key in group_landed.keys().chain(group_verdicts.keys()) {
        grouped.entry(key.clone()).or_default();
    }
    for row in &rows {
        grouped
            .entry((row.cohort, row.tier.clone(), row.destination.clone()))
            .or_default()
            .push(row);
    }
    let none = BTreeSet::new();
    let groups = grouped
        .into_iter()
        .map(|(key, rows)| {
            let landed = group_landed.get(&key).unwrap_or(&none);
            let verdicts = group_verdicts.get(&key).copied().unwrap_or(0);
            let (cohort, tier, destination) = key;
            GateGroup {
                cohort,
                tier,
                destination,
                figures: figures(rows.into_iter(), landed, verdicts),
            }
        })
        .collect();

    GateMetrics {
        schema_version: GATE_METRICS_SCHEMA_VERSION,
        since,
        until,
        gate_events_unmatched: unmatched,
        totals,
        groups,
        carries: carries
            .into_iter()
            .map(
                |((cohort, from_tier, to_tier, destination), carries)| CarryGroup {
                    cohort,
                    from_tier,
                    to_tier,
                    destination,
                    carries,
                },
            )
            .collect(),
    }
}

/// Timed command receipts, for gates whose log has no step markers.
fn receipt_steps(event: &Event) -> Vec<(String, f64)> {
    crate::phases::commands(event)
        .into_iter()
        .filter_map(|span| Some((span.command, span.seconds?)))
        .collect()
}

fn find_run<'a>(task: &'a Task, id: &str) -> Option<&'a Run> {
    task.runs.iter().find(|r| r.id == id)
}

fn main_branch<'a>(config: &'a Config, task: &Task) -> &'a str {
    task.project
        .as_deref()
        .and_then(|project| config.projects.get(project))
        .and_then(|project| project.base.as_deref())
        .unwrap_or("main")
}

fn classify_run(config: &Config, task: Option<&Task>, run: Option<&Run>) -> (Cohort, String) {
    let Some((task, run)) = task.zip(run) else {
        return (Cohort::LegacyOrUntiered, "unknown".into());
    };
    let branch = run
        .plan_target
        .as_ref()
        .map_or(run.base.as_str(), |target| target.branch.as_str());
    let destination = format!("{}:{branch}", task.project.as_deref().unwrap_or("unknown"));
    let cohort = match (run.gate_rules, &run.plan_target) {
        (GateRules::Legacy, _) => Cohort::LegacyOrUntiered,
        (GateRules::Tiered, Some(_)) => Cohort::PlanMember,
        (GateRules::Tiered, None) if run.base == main_branch(config, task) => {
            Cohort::MainBoundTiered
        }
        (GateRules::Tiered, None) => Cohort::TieredOtherBase,
    };
    (cohort, destination)
}

fn gate_class(detail: &Value) -> &'static str {
    if detail["result"] == "pass" {
        return "pass";
    }
    match detail["failed_cmd"].as_str() {
        Some(f) if f.starts_with("stale_result") => "stale_result",
        Some(f) if f.starts_with("invalid_command_receipts") => "invalid_receipts",
        _ => "command_failed",
    }
}

fn ratio(numerator: f64, denominator: u32) -> Option<f64> {
    (denominator > 0).then(|| numerator / f64::from(denominator))
}

fn figures<'a>(
    rows: impl Iterator<Item = &'a GateRow>,
    landed: &BTreeSet<String>,
    review_rounds: u32,
) -> GateFigures {
    let mut f = GateFigures::default();
    let mut landed_gates = 0u32;
    let mut landed_hours = 0.0;
    let mut steps: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for row in rows {
        f.gates_started += 1;
        f.worker_started += u32::from(row.worker);
        f.worker_receipts_reused_by_autopilot += u32::from(row.reused);
        f.followed_by_rebase += u32::from(row.followed_by_rebase);
        let hours = row.seconds().map_or(0.0, |s| s / 3600.0);
        let (count, followed) = match row.behind {
            Behind::Behind => (
                &mut f.behind.behind_at_start,
                Some(&mut f.behind.behind_at_start_followed_by_rebase),
            ),
            Behind::Contained => (
                &mut f.behind.contained_base,
                Some(&mut f.behind.contained_base_followed_by_rebase),
            ),
            Behind::Unknown => (&mut f.behind.unknown, None),
            Behind::NoIdentity => (&mut f.behind.no_identity, None),
        };
        count.add(hours);
        if row.followed_by_rebase
            && let Some(followed) = followed
        {
            followed.add(hours);
        }
        let Some(class) = row.class else { continue };
        f.gates_finished += 1;
        *f.classes.entry(class).or_default() += 1;
        f.gate_hours += hours;
        if class == "stale_result" {
            f.stale_result_hours += hours;
        }
        if landed.contains(&row.task) {
            landed_gates += 1;
            landed_hours += hours;
        }
        if row.tier == FULL_TIER {
            for (step, seconds) in &row.steps {
                steps.entry(step.clone()).or_default().push(*seconds);
            }
        }
    }
    f.gates_unfinished = f.gates_started - f.gates_finished;
    f.stale_result_share = ratio(
        f64::from(f.classes.get("stale_result").copied().unwrap_or(0)),
        f.gates_finished,
    );
    f.landed_tasks = u32::try_from(landed.len()).unwrap_or(u32::MAX);
    f.gates_per_landed_task = ratio(f64::from(landed_gates), f.landed_tasks);
    f.gate_hours_per_landed_task = ratio(landed_hours, f.landed_tasks);
    f.review_rounds = review_rounds;
    f.gates_per_review_round = ratio(f64::from(f.gates_started), f.review_rounds);
    f.full_step_seconds = steps
        .into_iter()
        .map(|(command, mut samples)| {
            let step = StepSeconds {
                samples: u32::try_from(samples.len()).unwrap_or(u32::MAX),
                total_seconds: samples.iter().sum(),
                median_seconds: crate::stats::median(&mut samples),
            };
            (command, step)
        })
        .collect();
    f
}
