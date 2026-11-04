//! Pure, deterministic report model over tasks and events at a fixed snapshot.
//!
//! Dispatches are selected by start time in `[since, snapshot_at]`; reviewer sessions stay
//! children of their dispatch. Usage is repriced in memory against the config and the dated
//! catalog. Unpriced tokens are a category of their own and never count as zero dollars.
//! Events after the snapshot are ignored; window events are those in `(since, snapshot_at]`.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use corral_core::pricing::CATALOG_DATE;
use corral_core::{Config, Event, Run, Task, UsageEntry};
use jiff::Timestamp;
use serde::Serialize;

use crate::analytics::{PhaseAccumulator, PhaseTotals, number};
use crate::phases::{Phase, task_phases};
use crate::throughput::{ModeThroughput, collect_mode_throughput, percentile};

pub const REPORT_SCHEMA_VERSION: u32 = 1;
const NONE: &str = "(none)";
const UNKNOWN: &str = "(unknown)";
const SLOWEST_COMMANDS: usize = 10;
/// Upper bounds (seconds) of the gate duration buckets; the last bucket is open.
const GATE_BUCKETS: [(f64, &str); 5] = [
    (60.0, "< 1 min"),
    (300.0, "1-5 min"),
    (900.0, "5-15 min"),
    (1800.0, "15-30 min"),
    (3600.0, "30-60 min"),
];

/// Usage of one group. Dollars are est. API-equivalent; `None` when nothing is priced.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Spend {
    pub estimated_usd: Option<f64>,
    pub reported_usd: Option<f64>,
    pub tokens: u64,
    pub unpriced_tokens: u64,
    pub entries: u64,
}

impl Spend {
    fn add(&mut self, entry: &UsageEntry) {
        self.entries += 1;
        self.tokens = self.tokens.saturating_add(entry.total_tokens());
        self.unpriced_tokens = self.unpriced_tokens.saturating_add(entry.unpriced_tokens);
        if let Some(cost) = entry.estimated_usd {
            *self.estimated_usd.get_or_insert(0.0) += cost;
        }
        if let Some(cost) = entry.reported_usd {
            *self.reported_usd.get_or_insert(0.0) += cost;
        }
    }

    fn merge(&mut self, other: &Spend) {
        self.entries += other.entries;
        self.tokens = self.tokens.saturating_add(other.tokens);
        self.unpriced_tokens = self.unpriced_tokens.saturating_add(other.unpriced_tokens);
        if let Some(cost) = other.estimated_usd {
            *self.estimated_usd.get_or_insert(0.0) += cost;
        }
        if let Some(cost) = other.reported_usd {
            *self.reported_usd.get_or_insert(0.0) += cost;
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ReportModel {
    pub schema_version: u32,
    pub since: Timestamp,
    pub snapshot_at: Timestamp,
    pub project: Option<String>,
    pub summary: Summary,
    pub spend: SpendSection,
    pub models: Vec<ModelRow>,
    pub modes: Vec<ModeRow>,
    /// Response rates keyed by each response's own model (stored estimates).
    pub throughput_by_mode: Vec<ModeThroughput>,
    /// Highest est. spend first, then task id.
    pub tasks: Vec<TaskRow>,
    pub quality: Quality,
    pub time: TimeSection,
    pub data_quality: DataQuality,
}

#[derive(Debug, Default, Serialize)]
pub struct Summary {
    pub dispatches: u32,
    pub reviewer_sessions: u32,
    pub tasks: u32,
    pub landed_tasks: u32,
    pub spend: Spend,
    pub cache_hit_ratio: Option<f64>,
    pub sessions_without_usage: u32,
    pub gates_finished: u32,
    pub gate_failure_rate: Option<f64>,
    pub send_backs: u32,
    pub rework_share: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct CategoryRow {
    /// input, output, cache_read, cache_write or unpriced.
    pub category: &'static str,
    pub tokens: u64,
    /// Always `None` for the unpriced category.
    pub estimated_usd: Option<f64>,
}

#[derive(Debug, Default, Serialize)]
pub struct SpendSection {
    /// UTC day of each response, else of its dispatch.
    pub by_day: BTreeMap<String, Spend>,
    pub by_project: BTreeMap<String, Spend>,
    /// implementation or review.
    pub by_role: BTreeMap<String, Spend>,
    pub categories: Vec<CategoryRow>,
    /// Cache reads over input, cache read and cache write tokens.
    pub cache_hit_ratio: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct ModelRow {
    pub model: String,
    pub price_sources: Vec<String>,
    pub spend: Spend,
}

#[derive(Debug, Serialize)]
pub struct ModeRow {
    pub role: String,
    pub harness: String,
    pub model: String,
    pub effort: String,
    pub mode: String,
    pub sessions: u32,
    pub spend: Spend,
}

#[derive(Debug, Serialize)]
pub struct RunRow {
    pub id: String,
    pub started: Timestamp,
    pub ended: Option<Timestamp>,
    pub outcome: Option<String>,
    pub harness: Option<String>,
    pub model: Option<String>,
    pub mode: String,
    pub rounds: u32,
    pub gate_failures: u32,
    pub reviewers: u32,
    pub usage_measured: bool,
    pub implementation: Spend,
    pub review: Spend,
}

#[derive(Debug, Serialize)]
pub struct TaskRow {
    pub id: String,
    pub title: String,
    pub project: String,
    pub status: String,
    pub runs: u32,
    pub rounds: u32,
    pub landed: bool,
    pub implementation: Spend,
    pub review: Spend,
    pub total: Spend,
    pub run_rows: Vec<RunRow>,
}

#[derive(Debug, Default, Serialize)]
pub struct GateCounts {
    pub pass: u32,
    pub fail: u32,
    pub failure_rate: Option<f64>,
}

#[derive(Debug, Default, Serialize)]
pub struct Quality {
    /// Rounds per dispatch (one plus its change rounds) to dispatch count.
    pub rounds_distribution: BTreeMap<u32, u32>,
    pub gates_by_project: BTreeMap<String, GateCounts>,
    /// gate, rebase (gate failed after rebasing), reviewer or owner.
    pub send_backs: BTreeMap<String, u32>,
    /// Est. spend after each dispatch's first send-back; `None` when timing is missing.
    pub rework_estimated_usd: Option<f64>,
    pub rework_share: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct Bucket {
    pub label: &'static str,
    pub count: u32,
}

#[derive(Debug, Serialize)]
pub struct CommandRow {
    pub command: String,
    pub samples: u32,
    pub median_seconds: Option<f64>,
    pub max_seconds: f64,
    pub total_seconds: f64,
}

#[derive(Debug, Default, Serialize)]
pub struct TimeSection {
    pub phases_by_project: BTreeMap<String, BTreeMap<Phase, PhaseTotals>>,
    pub gate_durations: Vec<Bucket>,
    pub gate_median_seconds: Option<f64>,
    pub gate_p90_seconds: Option<f64>,
    pub slowest_gate_commands: Vec<CommandRow>,
}

#[derive(Debug, Serialize)]
pub struct UnpricedModelRow {
    pub model: String,
    pub entries: u64,
    pub tokens: u64,
}

#[derive(Debug, Serialize)]
pub struct MissingUsage {
    pub task: String,
    pub run: String,
    pub role: &'static str,
    pub harness: String,
    pub note: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct DataQuality {
    pub catalog_date: &'static str,
    /// Priced entries per rate source (config, catalog, ...).
    pub price_sources: BTreeMap<String, u64>,
    pub unpriced_models: Vec<UnpricedModelRow>,
    pub runs_without_usage: Vec<MissingUsage>,
    /// Sessions without usage whose note names an unsupported harness, per harness.
    pub unsupported_harnesses: BTreeMap<String, u32>,
}

pub(crate) fn label<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn day(at: Timestamp) -> String {
    at.to_string().get(..10).unwrap_or_default().to_owned()
}

fn ratio(part: u64, whole: u64) -> Option<f64> {
    (whole > 0).then(|| number(part) / number(whole))
}

/// The cause of a send-back event, or `None` when the event is not one.
pub(crate) fn send_back_cause(event: &Event) -> Option<&'static str> {
    match event.kind.as_str() {
        "autopilot" if event.detail["to"] == "changes" => {
            let reason = event.detail["reason"].as_str().unwrap_or_default();
            Some(match reason.starts_with("the gate") {
                true if reason.contains("after rebasing") => "rebase",
                true => "gate",
                false => "reviewer",
            })
        }
        "changes_requested"
            if event.detail["by"] != "autopilot" && event.detail["submitted"] != false =>
        {
            Some("owner")
        }
        _ => None,
    }
}

fn reviewers<'a>(run: &'a Run, out: &mut Vec<&'a Run>) {
    for child in &run.review_runs {
        out.push(child);
        reviewers(child, out);
    }
}

#[derive(Default)]
struct Collector {
    spend: SpendSection,
    categories: [(u64, f64); 4],
    unpriced: u64,
    cache: [u64; 3],
    models: BTreeMap<String, (BTreeSet<String>, Spend)>,
    modes: BTreeMap<[String; 5], (u32, Spend)>,
    data: DataQuality,
    unpriced_models: BTreeMap<String, (u64, u64)>,
    sessions_without_usage: u32,
}

impl Collector {
    /// Reprices and records one session; returns its spend and repriced entries.
    fn session(
        &mut self,
        config: &Config,
        task: &Task,
        project: &str,
        run: &Run,
        role: &'static str,
    ) -> (Spend, Option<Vec<UsageEntry>>) {
        let harness = run.harness.clone().unwrap_or_else(|| UNKNOWN.into());
        let key = [
            role.to_owned(),
            harness.clone(),
            run.model.clone().unwrap_or_else(|| UNKNOWN.into()),
            run.effort.clone().unwrap_or_else(|| NONE.into()),
            label(&run.mode),
        ];
        let mut spend = Spend::default();
        let Some(mut usage) = run.usage.clone() else {
            self.sessions_without_usage += 1;
            let note = run.usage_note.clone();
            if note.as_deref().is_some_and(|n| n.contains("unsupported")) {
                *self
                    .data
                    .unsupported_harnesses
                    .entry(harness.clone())
                    .or_default() += 1;
            }
            self.data.runs_without_usage.push(MissingUsage {
                task: task.id.clone(),
                run: run.id.clone(),
                role,
                harness,
                note,
            });
            self.modes.entry(key).or_default().0 += 1;
            return (spend, None);
        };
        usage.reprice(config, run.model.as_deref());
        for entry in &usage.entries {
            spend.add(entry);
            let at = entry.at.unwrap_or(run.started);
            self.spend.by_day.entry(day(at)).or_default().add(entry);
            self.cache[0] = self.cache[0].saturating_add(entry.input);
            self.cache[1] = self.cache[1].saturating_add(entry.cache_read);
            self.cache[2] = self.cache[2].saturating_add(entry.cache_write);
            let model = entry.model.clone().unwrap_or_else(|| UNKNOWN.into());
            let row = self.models.entry(model.clone()).or_default();
            row.1.add(entry);
            let price = entry.model.as_deref().and_then(|m| config.price(m));
            match (entry.estimated_usd, price) {
                (Some(_), Some(price)) => {
                    if let Some(source) = &entry.price_source {
                        row.0.insert(source.clone());
                        *self.data.price_sources.entry(source.clone()).or_default() += 1;
                    }
                    let rates = [
                        price.input,
                        price.output,
                        price.cache_read(),
                        price.cache_write(),
                    ];
                    let counts = [
                        entry.input,
                        entry.output,
                        entry.cache_read,
                        entry.cache_write,
                    ];
                    for ((tokens, cost), (count, rate)) in self
                        .categories
                        .iter_mut()
                        .zip(counts.into_iter().zip(rates))
                    {
                        *tokens = tokens.saturating_add(count);
                        *cost += number(count) * rate / 1_000_000.0;
                    }
                }
                _ => {
                    self.unpriced = self.unpriced.saturating_add(entry.total_tokens());
                    let row = self.unpriced_models.entry(model).or_default();
                    row.0 += 1;
                    row.1 = row.1.saturating_add(entry.total_tokens());
                }
            }
        }
        self.spend
            .by_project
            .entry(project.to_owned())
            .or_default()
            .merge(&spend);
        self.spend
            .by_role
            .entry(role.to_owned())
            .or_default()
            .merge(&spend);
        let mode = self.modes.entry(key).or_default();
        mode.0 += 1;
        mode.1.merge(&spend);
        (spend, Some(usage.entries))
    }
}

#[derive(Default)]
struct Rework {
    usd: f64,
    unavailable: bool,
}

impl Rework {
    fn add(&mut self, change: Option<Timestamp>, entries: Option<&[UsageEntry]>) {
        let Some(change) = change else { return };
        let Some(entries) = entries else {
            self.unavailable = true;
            return;
        };
        for entry in entries {
            if let Some(cost) = entry.estimated_usd.filter(|c| *c > 0.0) {
                match entry.at {
                    Some(at) if at > change => self.usd += cost,
                    Some(_) => {}
                    None => self.unavailable = true,
                }
            }
        }
    }
}

/// Collect the report model. Same tasks, events, config and snapshot give the same model,
/// whatever the input order of tasks.
pub fn collect_report(
    config: &Config,
    tasks: &[Task],
    events: &[Event],
    since: Timestamp,
    snapshot_at: Timestamp,
    project: Option<&str>,
) -> Result<ReportModel> {
    let mut tasks: Vec<&Task> = tasks
        .iter()
        .filter(|t| project.is_none_or(|p| t.project.as_deref() == Some(p)))
        .collect();
    tasks.sort_by(|a, b| a.id.cmp(&b.id));
    let ids: BTreeSet<&str> = tasks.iter().map(|t| t.id.as_str()).collect();
    let mut by_task: BTreeMap<&str, Vec<&Event>> = BTreeMap::new();
    for event in events.iter().filter(|e| e.ts <= snapshot_at) {
        if let Some(id) = event.task.as_deref().filter(|id| ids.contains(id)) {
            by_task.entry(id).or_default().push(event);
        }
    }
    let selected = |run: &Run| run.started >= since && run.started <= snapshot_at;

    let mut collector = Collector::default();
    let mut summary = Summary::default();
    let mut quality = Quality::default();
    let mut rework = Rework::default();
    let mut phases: BTreeMap<String, PhaseAccumulator> = BTreeMap::new();
    let mut gate_seconds = Vec::new();
    let mut commands: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let mut rows = Vec::new();

    for task in &tasks {
        let project = task.project.clone().unwrap_or_else(|| NONE.into());
        let history = by_task.get(task.id.as_str()).map_or(&[][..], Vec::as_slice);
        let window: Vec<&Event> = history.iter().copied().filter(|e| e.ts > since).collect();
        let mut first_change: BTreeMap<&str, Timestamp> = BTreeMap::new();
        for event in history {
            if send_back_cause(event).is_some()
                && let Some(run) = event.detail["run"].as_str()
            {
                let at = first_change.entry(run).or_insert(event.ts);
                *at = (*at).min(event.ts);
            }
        }
        let gates = quality.gates_by_project.entry(project.clone()).or_default();
        let mut landed = false;
        for event in &window {
            if let Some(cause) = send_back_cause(event) {
                *quality.send_backs.entry(cause.to_owned()).or_default() += 1;
                summary.send_backs += 1;
            }
            match (event.kind.as_str(), event.detail["result"].as_str()) {
                ("gate", Some("pass")) => gates.pass += 1,
                ("gate", Some("fail")) => gates.fail += 1,
                ("approved", _) if event.detail["strategy"] != "keep_branch" => landed = true,
                _ => {}
            }
        }
        let accumulator = phases.entry(project.clone()).or_default();
        for span in task_phases(task, history)
            .iter()
            .filter(|s| s.start > since && s.start <= snapshot_at)
        {
            accumulator.add(span);
            if span.phase == Phase::Gate
                && let Some(seconds) = span.seconds
            {
                gate_seconds.push(seconds);
                for command in &span.commands {
                    if let Some(seconds) = command.seconds {
                        commands
                            .entry(command.command.clone())
                            .or_default()
                            .push(seconds);
                    }
                }
            }
        }

        let mut row = TaskRow {
            id: task.id.clone(),
            title: task.title.clone(),
            project: project.clone(),
            status: label(&task.status),
            runs: 0,
            rounds: 0,
            landed,
            implementation: Spend::default(),
            review: Spend::default(),
            total: Spend::default(),
            run_rows: Vec::new(),
        };
        for run in task.runs.iter().filter(|r| selected(r)) {
            let (implementation, mut entries) =
                collector.session(config, task, &project, run, "implementation");
            let mut children = Vec::new();
            reviewers(run, &mut children);
            let mut review = Spend::default();
            for child in &children {
                let (spend, child_entries) =
                    collector.session(config, task, &project, child, "review");
                review.merge(&spend);
                match (&mut entries, child_entries) {
                    (Some(all), Some(more)) => all.extend(more),
                    (all @ Some(_), None) => *all = None,
                    _ => {}
                }
            }
            rework.add(
                first_change.get(run.id.as_str()).copied(),
                entries.as_deref(),
            );
            let rounds = 1 + run.changes_round.as_ref().map_or(0, |c| c.n);
            *quality.rounds_distribution.entry(rounds).or_default() += 1;
            summary.dispatches += 1;
            summary.reviewer_sessions += u32::try_from(children.len()).unwrap_or(u32::MAX);
            row.runs += 1;
            row.rounds += rounds;
            row.implementation.merge(&implementation);
            row.review.merge(&review);
            row.run_rows.push(RunRow {
                id: run.id.clone(),
                started: run.started,
                ended: run.ended,
                outcome: run.outcome.as_ref().map(label),
                harness: run.harness.clone(),
                model: run.model.clone(),
                mode: label(&run.mode),
                rounds,
                gate_failures: run.failures,
                reviewers: u32::try_from(children.len()).unwrap_or(u32::MAX),
                usage_measured: run.usage.is_some(),
                implementation,
                review,
            });
        }
        if row.runs > 0 || landed {
            summary.tasks += 1;
            summary.landed_tasks += u32::from(landed);
            row.total.merge(&row.implementation);
            row.total.merge(&row.review);
            rows.push(row);
        }
    }

    rows.sort_by(|a, b| {
        let cost = |r: &TaskRow| r.total.estimated_usd.unwrap_or(f64::NEG_INFINITY);
        cost(b).total_cmp(&cost(a)).then_with(|| a.id.cmp(&b.id))
    });
    quality.gates_by_project.retain(|_, g| g.pass + g.fail > 0);
    for gates in quality.gates_by_project.values_mut() {
        gates.failure_rate = ratio(gates.fail.into(), (gates.pass + gates.fail).into());
        summary.gates_finished += gates.pass + gates.fail;
    }
    let failed: u32 = quality.gates_by_project.values().map(|g| g.fail).sum();
    summary.gate_failure_rate = ratio(failed.into(), summary.gates_finished.into());

    let Collector {
        mut spend,
        categories,
        unpriced,
        cache,
        models,
        modes,
        mut data,
        unpriced_models,
        sessions_without_usage,
    } = collector;
    for spend_row in spend.by_role.values() {
        summary.spend.merge(spend_row);
    }
    let names = ["input", "output", "cache_read", "cache_write"];
    spend.categories = names
        .into_iter()
        .zip(categories)
        .map(|(category, (tokens, cost))| CategoryRow {
            category,
            tokens,
            estimated_usd: (tokens > 0).then_some(cost),
        })
        .collect();
    spend.categories.push(CategoryRow {
        category: "unpriced",
        tokens: unpriced,
        estimated_usd: None,
    });
    spend.cache_hit_ratio = ratio(
        cache[1],
        cache[0].saturating_add(cache[1]).saturating_add(cache[2]),
    );
    summary.cache_hit_ratio = spend.cache_hit_ratio;
    summary.sessions_without_usage = sessions_without_usage;
    quality.rework_estimated_usd = summary
        .spend
        .estimated_usd
        .filter(|_| !rework.unavailable)
        .map(|_| rework.usd);
    quality.rework_share = quality
        .rework_estimated_usd
        .zip(summary.spend.estimated_usd)
        .filter(|(_, total)| *total > 0.0)
        .map(|(part, total)| part / total);
    summary.rework_share = quality.rework_share;

    data.catalog_date = CATALOG_DATE;
    data.unpriced_models = unpriced_models
        .into_iter()
        .map(|(model, (entries, tokens))| UnpricedModelRow {
            model,
            entries,
            tokens,
        })
        .collect();

    let owned: Vec<Task> = tasks.iter().map(|t| (*t).clone()).collect();
    Ok(ReportModel {
        schema_version: REPORT_SCHEMA_VERSION,
        since,
        snapshot_at,
        project: project.map(str::to_owned),
        summary,
        spend,
        models: models
            .into_iter()
            .map(|(model, (sources, spend))| ModelRow {
                model,
                price_sources: sources.into_iter().collect(),
                spend,
            })
            .collect(),
        modes: modes
            .into_iter()
            .map(
                |([role, harness, model, effort, mode], (sessions, spend))| ModeRow {
                    role,
                    harness,
                    model,
                    effort,
                    mode,
                    sessions,
                    spend,
                },
            )
            .collect(),
        throughput_by_mode: collect_mode_throughput(&owned, Some(since), snapshot_at)?,
        tasks: rows,
        quality,
        time: time_section(phases, gate_seconds, commands),
        data_quality: data,
    })
}

fn time_section(
    phases: BTreeMap<String, PhaseAccumulator>,
    mut gate_seconds: Vec<f64>,
    commands: BTreeMap<String, Vec<f64>>,
) -> TimeSection {
    let mut buckets: Vec<Bucket> = GATE_BUCKETS
        .iter()
        .map(|(_, label)| Bucket { label, count: 0 })
        .chain([Bucket {
            label: "≥ 60 min",
            count: 0,
        }])
        .collect();
    for seconds in &gate_seconds {
        let index = GATE_BUCKETS
            .iter()
            .position(|(bound, _)| seconds < bound)
            .unwrap_or(GATE_BUCKETS.len());
        if let Some(bucket) = buckets.get_mut(index) {
            bucket.count += 1;
        }
    }
    gate_seconds.sort_by(f64::total_cmp);
    let p90 = percentile(&gate_seconds, 90);
    let median = crate::stats::median(&mut gate_seconds);
    let mut slowest: Vec<CommandRow> = commands
        .into_iter()
        .map(|(command, mut samples)| CommandRow {
            command,
            samples: u32::try_from(samples.len()).unwrap_or(u32::MAX),
            max_seconds: samples.iter().copied().fold(0.0, f64::max),
            total_seconds: samples.iter().sum(),
            median_seconds: crate::stats::median(&mut samples),
        })
        .collect();
    slowest.sort_by(|a, b| {
        b.max_seconds
            .total_cmp(&a.max_seconds)
            .then_with(|| a.command.cmp(&b.command))
    });
    slowest.truncate(SLOWEST_COMMANDS);
    TimeSection {
        phases_by_project: phases
            .into_iter()
            .map(|(project, acc)| (project, acc.finish()))
            .filter(|(_, phases)| !phases.is_empty())
            .collect(),
        gate_durations: buckets,
        gate_median_seconds: median,
        gate_p90_seconds: p90,
        slowest_gate_commands: slowest,
    }
}
