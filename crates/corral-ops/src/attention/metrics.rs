//! On-demand attention statistics from deduplicated retained facts. Never on the sync path.
//!
//! Inputs are the event log, the current task ledgers and the existing standalone producer
//! records. Facts are deduplicated by `fact_id` before any metric is computed, so a fact that
//! was appended twice after a crash, or that is both appended and still pending, counts once.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use corral_core::attention::NoticeClaim;
use corral_core::{Event, Task};
use jiff::{SignedDuration, Timestamp};
use serde::Serialize;
use serde_json::{Value, json};

use crate::Ctx;

/// Prefix of the autopilot escalation reason that the classifier treats as infrastructure.
pub const INFRA_PREFIX: &str = "integration preflight error";
const HOUR: i64 = 3600;

/// The closed interval `[start, end]` a report covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Window {
    pub start: Timestamp,
    pub end: Timestamp,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Stats {
    pub version: u32,
    pub window: Option<Window>,
    pub population: Population,
    /// PM-available (`pm_live_at_open` true), PM-responsible conditions.
    pub handling: Latency,
    /// PM-responsible conditions whose `pm_live_at_open` was false or unknown.
    pub handling_pm_absent: Latency,
    pub resolution: Latency,
    pub backlog: Backlog,
    pub wakes: Wakes,
    pub notifications: Notifications,
    pub actions: Actions,
    pub lost_review_rounds: u64,
    /// Per-condition rows, keyed by condition key, for exact scenario assertions.
    pub conditions: BTreeMap<String, ConditionRow>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Population {
    pub total: u64,
    pub carry_in: u64,
    pub opened: u64,
    pub resolved: u64,
    pub open_at_end: u64,
    pub pm_responsible: u64,
    pub owner_responsible: u64,
    pub pm_available: u64,
    pub pm_absent: u64,
    pub by_kind: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Latency {
    pub observed: u64,
    pub censored: u64,
    pub p50_secs: Option<i64>,
    pub p95_secs: Option<i64>,
    pub max_secs: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Backlog {
    pub samples: u64,
    pub peak_open: u64,
    pub peak_p50_secs: Option<i64>,
    pub peak_p95_secs: Option<i64>,
    pub peak_max_secs: Option<i64>,
    pub end_open: u64,
    pub end_p50_secs: Option<i64>,
    pub end_p95_secs: Option<i64>,
    pub end_max_secs: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Wakes {
    pub attention_due: u64,
    /// Distinct (project, pass time) pairs that carried an `attention_due` digest.
    pub due_pairs: u64,
    pub attention_levels: u64,
    pub max_levels_per_condition: u64,
    pub raw_push: RawPush,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RawPush {
    pub submitted: u64,
    /// Submitted receipts dropped because the same (pane, cursor, kind) was already counted.
    pub replayed: u64,
    pub by_kind: BTreeMap<String, u64>,
    pub attributed: BTreeMap<String, u64>,
    pub ambiguous: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Notifications {
    /// Distinct covered claim IDs across all physical batches.
    pub claims: u64,
    pub batches: u64,
    /// Batches claimed before the window that cover a condition in it; never charged to
    /// the window's calls or budget.
    pub carry_in: u64,
    pub carry_in_calls: [u64; 2],
    pub success: u64,
    pub failure: u64,
    pub outcome_unknown: u64,
    pub certainly_not_invoked: u64,
    /// Physical transport invocations: [success + failure, success + failure + unknown].
    pub calls: [u64; 2],
    pub outside_ladder: u64,
    pub exempt_owner_entry: u64,
    pub mixed: u64,
    /// Batches counted against the owner-notification budget (non-exempt), and their calls.
    pub budget_batches: u64,
    pub budget_calls: [u64; 2],
    pub coverage_edges: u64,
    pub condition_links: u64,
    pub legacy_links: u64,
    pub by_producer: BTreeMap<String, ProducerCounts>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ProducerCounts {
    pub batches: u64,
    pub claims: u64,
    pub success: u64,
    pub failure: u64,
    pub outcome_unknown: u64,
    pub certainly_not_invoked: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Actions {
    pub attributed: u64,
    pub ambiguous: u64,
    pub unattributed: u64,
    pub duplicates: u64,
    pub after_terminal: u64,
    pub owner_rescues: u64,
    pub by_condition: BTreeMap<String, u64>,
}

type Timed = BTreeMap<String, Option<Timestamp>>;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ConditionRow {
    pub task: String,
    pub kind: String,
    pub responsible: Option<String>,
    pub since: Option<Timestamp>,
    pub opened_at: Option<Timestamp>,
    pub pm_live_at_open: Option<bool>,
    pub first_handled_at: Option<Timestamp>,
    pub resolved_at: Option<Timestamp>,
    pub carry_in: bool,
    pub handled: u64,
    /// Wake facts timed inside the window; `lifetime_wakes` counts the condition's whole history.
    pub wakes: u64,
    pub lifetime_wakes: u64,
    pub owner_levels: u64,
    pub owner_claim: Option<String>,
    /// The condition's run when its key or open record identifies one; otherwise unknown.
    pub run: Option<String>,
    #[serde(skip)]
    wake_ids: Timed,
    #[serde(skip)]
    owner_ids: Timed,
    #[serde(skip)]
    handled_ids: Timed,
}

/// Read the retained facts and records for `window`. An explicit, read-only command path.
///
/// # Errors
/// Returns store, event-log or producer-record read errors.
pub fn stats(ctx: &Ctx, window: Window) -> Result<Stats> {
    let tasks = ctx.store.list_shared()?;
    // The PM push reader's cursors identify each receipt's source event exactly.
    let (mut events, mut cursors) = (Vec::new(), Vec::new());
    let path = ctx.store.dir().join("events.jsonl");
    if path.exists() {
        let mut reader = corral_core::EventReader::from_offset(&path, 0)?;
        loop {
            let before = reader.read_position();
            match reader.next_event()? {
                Some((event, cursor)) => {
                    events.push(event);
                    cursors.push(cursor.offset);
                }
                None if reader.read_position() == before => break,
                None => {}
            }
        }
    }
    let mut claims: Vec<NoticeClaim> = tasks
        .iter()
        .flat_map(|task| task.attention.notice_outbox.iter().cloned())
        .collect();
    claims.extend(crate::resource_escalation::notice(ctx)?);
    claims.extend(
        crate::hooks::post_land_operations(ctx, None)?
            .into_iter()
            .filter_map(|op| op.notice),
    );
    let dir = ctx.store.state_dir().join("availability");
    let mut ids = Vec::new();
    match std::fs::read_dir(&dir) {
        Ok(entries) => {
            for entry in entries {
                let path = entry.context("reading availability entry")?.path();
                if path.extension().is_some_and(|ext| ext == "json") {
                    ids.extend(path.file_stem().and_then(|s| s.to_str()).map(String::from));
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).with_context(|| format!("listing {}", dir.display())),
    }
    ids.sort();
    for id in ids {
        claims.extend(crate::availability::notice(ctx, &id)?);
    }
    Ok(compute_logged(&tasks, &events, &cursors, &claims, window))
}

/// One deduplicated fact: its first position in the log, or after the log when only pending.
struct Fact {
    kind: String,
    detail: Value,
}

fn facts(tasks: &[Task], events: &[Event], claims: &[NoticeClaim]) -> Vec<Fact> {
    let appended = (events.iter())
        .filter_map(|event| Some((event.detail["fact_id"].as_str()?.to_owned(), event)))
        .map(|(id, event)| (id, event.kind.clone(), event.detail.clone()));
    // Retained, not yet appended facts are durable too.
    let ledgers = tasks.iter().flat_map(|task| &task.attention.outbox);
    let ledgers = ledgers.map(|f| (f.fact_id.clone(), f.kind.clone(), f.detail.clone()));
    let notices = claims.iter().flat_map(|claim| &claim.pending);
    let notices = notices.map(|f| (f.fact_id.clone(), f.kind.clone(), f.detail.clone()));
    let mut seen = BTreeSet::new();
    (appended.chain(ledgers).chain(notices))
        .filter(|(id, ..)| seen.insert(id.clone()))
        .map(|(id, kind, mut detail)| {
            detail["fact_id"] = json!(id);
            Fact { kind, detail }
        })
        .collect()
}

fn time(value: &Value) -> Option<Timestamp> {
    value.as_str().and_then(|text| text.parse().ok())
}

fn within(at: Option<Timestamp>, window: Window) -> bool {
    at.is_some_and(|at| window.start <= at && at <= window.end)
}

/// An event's canonical detail time when it records one, otherwise its append time.
fn when(event: &Event) -> Timestamp {
    time(&event.detail["at"]).unwrap_or(event.ts)
}

/// Whether a condition was open at a canonical instant: from its canonical `since` (or opening)
/// through its resolution, both inclusive. Publication order never stands in for time.
fn open_at(row: &ConditionRow, at: Timestamp) -> bool {
    row.since.or(row.opened_at).is_some_and(|since| since <= at)
        && row.resolved_at.is_none_or(|end| at <= end)
}

fn counted(ids: &Timed, window: Window) -> u64 {
    ids.values().filter(|at| within(**at, window)).count() as u64
}

fn secs(later: Timestamp, earlier: Timestamp) -> i64 {
    later.duration_since(earlier).as_secs().max(0)
}

/// Nearest-rank percentile over an ascending slice.
pub fn percentile(sorted: &[i64], p: u32) -> Option<i64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (sorted.len() * p as usize).div_ceil(100).max(1);
    sorted.get(rank - 1).copied()
}

fn latency(mut values: Vec<i64>, censored: u64) -> Latency {
    values.sort_unstable();
    Latency {
        observed: values.len() as u64,
        censored,
        p50_secs: percentile(&values, 50),
        p95_secs: percentile(&values, 95),
        max_secs: values.last().copied(),
    }
}

fn condition<'a>(
    rows: &'a mut BTreeMap<String, ConditionRow>,
    detail: &Value,
) -> Option<&'a mut ConditionRow> {
    let key = detail["key"].as_str()?;
    let row = rows.entry(key.to_owned()).or_default();
    if row.task.is_empty() {
        row.task = key.split('/').next().unwrap_or_default().to_owned();
    }
    Some(row)
}

fn fill(row: &mut ConditionRow, detail: &Value) {
    if row.kind.is_empty()
        && let Some(kind) = detail["kind"].as_str()
    {
        row.kind = kind.into();
    }
    if row.responsible.is_none() {
        row.responsible = detail["responsible"].as_str().map(Into::into);
    }
    row.since = row.since.or_else(|| time(&detail["since"]));
    row.opened_at = row.opened_at.or_else(|| time(&detail["opened_at"]));
    if row.owner_claim.is_none() {
        row.owner_claim = detail["owner_claim"].as_str().map(Into::into);
    }
}

/// Merge a self-contained record: a resolution fact or a current ledger record.
fn merge(row: &mut ConditionRow, record: &Value) {
    fill(row, record);
    row.resolved_at = row.resolved_at.or_else(|| time(&record["resolved_at"]));
    if row.pm_live_at_open.is_none() {
        row.pm_live_at_open = record["pm_live_at_open"].as_bool();
    }
    if let Some(at) = time(&record["first_handled_at"]) {
        earliest(&mut row.first_handled_at, at);
    }
    if row.owner_claim.is_none() {
        row.owner_claim = record["levels"]["owner_claim"].as_str().map(Into::into);
    }
}

fn earliest(slot: &mut Option<Timestamp>, at: Timestamp) {
    *slot = Some(slot.map_or(at, |old| old.min(at)));
}

fn conditions(tasks: &[Task], facts: &[Fact]) -> BTreeMap<String, ConditionRow> {
    let mut rows = BTreeMap::new();
    for fact in facts {
        let id = fact.detail["fact_id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let lifecycle = matches!(
            fact.kind.as_str(),
            "attention_opened" | "attention_overdue" | "attention_handled" | "attention_resolved"
        );
        let Some(row) = lifecycle
            .then(|| condition(&mut rows, &fact.detail))
            .flatten()
        else {
            continue;
        };
        match fact.kind.as_str() {
            "attention_opened" => {
                fill(row, &fact.detail);
                if let Some(live) = fact.detail.get("pm_live") {
                    row.pm_live_at_open = row.pm_live_at_open.or(live.as_bool());
                }
                if fact.detail["wake"] == true {
                    row.wake_ids.insert(id, time(&fact.detail["opened_at"]));
                }
            }
            "attention_overdue" => {
                fill(row, &fact.detail);
                let at = time(&fact.detail["at"]);
                match fact.detail["level"].as_str() {
                    Some("owner") => row.owner_ids.insert(id, at),
                    Some(_) if fact.detail["wake"] == true => row.wake_ids.insert(id, at),
                    _ => None,
                };
            }
            "attention_handled" => {
                let at = time(&fact.detail["handling"]["at"]);
                row.handled_ids.insert(id, at);
                if let Some(at) = at {
                    earliest(&mut row.first_handled_at, at);
                }
            }
            "attention_resolved" => {
                merge(row, &fact.detail);
            }
            _ => {}
        }
    }
    for task in tasks {
        for (key, record) in &task.attention.records {
            let row = rows.entry(key.0.clone()).or_default();
            row.task.clone_from(&task.id);
            if let Ok(record) = serde_json::to_value(record) {
                merge(row, &record);
            }
        }
    }
    for (key, row) in &mut rows {
        // Run-scoped kinds embed their run as the first anchor field. Other kinds take the
        // retained run that had started by the condition's canonical time, never the current
        // run, so resolution, pruning and newer runs leave the historical identity unchanged.
        let anchor = key.splitn(3, '/').nth(2).unwrap_or_default();
        row.run = if matches!(
            row.kind.as_str(),
            "agent_lost" | "silent_turn" | "stuck_process"
        ) {
            anchor.split(':').next().map(Into::into)
        } else {
            let since = row.since.or(row.opened_at);
            (tasks.iter().find(|task| task.id == row.task))
                .and_then(|task| {
                    (task.runs.iter())
                        .filter(|run| since.is_some_and(|since| run.started <= since))
                        .max_by_key(|run| run.started)
                })
                .map(|run| run.id.clone())
        };
        row.lifetime_wakes = row.wake_ids.len() as u64;
    }
    rows
}

/// Compute every metric from already-loaded inputs. Pure: no clock, store or transport.
/// Without log cursors, raw pushes cannot be resolved to their source events.
pub fn compute(tasks: &[Task], events: &[Event], claims: &[NoticeClaim], window: Window) -> Stats {
    compute_logged(tasks, events, &[], claims, window)
}

/// As [`compute`], with each event's reader cursor offset, parallel to `events`.
pub fn compute_logged(
    tasks: &[Task],
    events: &[Event],
    cursors: &[u64],
    claims: &[NoticeClaim],
    window: Window,
) -> Stats {
    let facts = facts(tasks, events, claims);
    let mut rows = conditions(tasks, &facts);
    rows.retain(|_, row| {
        let opened = row.opened_at.or(row.since);
        opened.is_some_and(|at| at <= window.end)
            && row.resolved_at.is_none_or(|at| at >= window.start)
    });
    let mut stats = Stats {
        version: 1,
        window: Some(window),
        ..Stats::default()
    };
    let mut handling = (Vec::new(), 0);
    let mut absent = (Vec::new(), 0);
    let mut resolution = (Vec::new(), 0);
    for row in rows.values_mut() {
        let population = &mut stats.population;
        population.total += 1;
        *population.by_kind.entry(row.kind.clone()).or_default() += 1;
        row.carry_in = row
            .opened_at
            .or(row.since)
            .is_some_and(|at| at < window.start);
        population.carry_in += u64::from(row.carry_in);
        population.opened += u64::from(within(row.opened_at, window));
        let resolved = within(row.resolved_at, window);
        population.resolved += u64::from(resolved);
        population.open_at_end += u64::from(!resolved);
        let pm = row.responsible.as_deref() == Some("pm");
        population.pm_responsible += u64::from(pm);
        population.owner_responsible += u64::from(row.responsible.as_deref() == Some("owner"));
        let live = row.pm_live_at_open == Some(true);
        population.pm_available += u64::from(live);
        population.pm_absent += u64::from(!live);
        let since = row.since.or(row.opened_at);
        if let Some(since) = since {
            if resolved && let Some(at) = row.resolved_at {
                resolution.0.push(secs(at, since));
            } else {
                resolution.1 += 1;
            }
            if pm {
                let cohort = if live { &mut handling } else { &mut absent };
                let handled = [row.first_handled_at, row.resolved_at]
                    .into_iter()
                    .flatten()
                    .filter(|at| *at <= window.end)
                    .min();
                match handled {
                    Some(at) => cohort.0.push(secs(at, since)),
                    None => cohort.1 += 1,
                }
            }
        }
        row.wakes = counted(&row.wake_ids, window);
        row.owner_levels = counted(&row.owner_ids, window);
        row.handled = counted(&row.handled_ids, window);
        stats.wakes.attention_levels += row.wakes;
        stats.wakes.max_levels_per_condition =
            stats.wakes.max_levels_per_condition.max(row.lifetime_wakes);
    }
    stats.handling = latency(handling.0, handling.1);
    stats.handling_pm_absent = latency(absent.0, absent.1);
    stats.resolution = latency(resolution.0, resolution.1);
    stats.backlog = backlog(&rows, window);
    wakes(&mut stats.wakes, events, cursors, &facts, &rows, window);
    stats.notifications = notifications(&facts, claims, &rows, window);
    stats.actions = actions(events, &facts, &rows, window);
    let infra = |event: &&Event| {
        event.kind == "autopilot"
            && event.detail["to"] == "escalated"
            && within(Some(when(event)), window)
            && (event.detail["reason"].as_str()).is_some_and(|r| r.starts_with(INFRA_PREFIX))
    };
    stats.lost_review_rounds = (events.iter().filter(infra))
        .map(|event| {
            event.detail["released_review_runs"]
                .as_array()
                .map_or(0, Vec::len) as u64
        })
        .sum();
    stats.conditions = rows;
    stats
}
fn backlog(rows: &BTreeMap<String, ConditionRow>, window: Window) -> Backlog {
    let intervals: Vec<_> = (rows.values())
        .filter(|row| row.responsible.as_deref() == Some("pm"))
        .filter_map(|row| Some((row.since.or(row.opened_at)?, row.resolved_at)))
        .collect();
    let mut points = Vec::new();
    // Hourly points carry no information before the earliest PM-responsible interval opens.
    if let Some(first) = intervals.iter().map(|(since, _)| *since).min() {
        let skip = secs(first, window.start) / HOUR;
        let mut at = (window.start).checked_add(SignedDuration::from_secs((skip + 1) * HOUR));
        while let Ok(point) = at
            && point < window.end
        {
            points.push(point);
            at = point.checked_add(SignedDuration::from_secs(HOUR));
        }
    }
    points.push(window.end);
    let mut out = Backlog::default();
    for point in points {
        let mut ages: Vec<_> = (intervals.iter())
            .filter(|(since, resolved)| *since <= point && resolved.is_none_or(|end| point < end))
            .map(|(since, _)| secs(point, *since))
            .collect();
        ages.sort_unstable();
        let (p50, p95, max) = (
            percentile(&ages, 50),
            percentile(&ages, 95),
            ages.last().copied(),
        );
        out.samples += 1;
        out.peak_open = out.peak_open.max(ages.len() as u64);
        out.peak_p50_secs = out.peak_p50_secs.max(p50);
        out.peak_p95_secs = out.peak_p95_secs.max(p95);
        out.peak_max_secs = out.peak_max_secs.max(max);
        (out.end_open, out.end_p50_secs, out.end_p95_secs) = (ages.len() as u64, p50, p95);
        out.end_max_secs = max;
    }
    out
}

fn wakes(
    out: &mut Wakes,
    events: &[Event],
    cursors: &[u64],
    facts: &[Fact],
    rows: &BTreeMap<String, ConditionRow>,
    window: Window,
) {
    let mut pairs = BTreeSet::new();
    for fact in facts.iter().filter(|fact| fact.kind == "attention_due") {
        if within(time(&fact.detail["at"]), window) {
            out.attention_due += 1;
            pairs.insert((
                fact.detail["project"].to_string(),
                fact.detail["at"].to_string(),
            ));
        }
    }
    out.due_pairs = pairs.len() as u64;
    let mut receipts = BTreeSet::new();
    for event in events {
        if event.kind != "pm_event_delivery"
            || event.detail["submitted"] != true
            || !within(Some(when(event)), window)
        {
            continue;
        }
        let kind = event.detail["event_kind"].as_str().unwrap_or_default();
        let identity = (
            event.detail["pane"].to_string(),
            event.detail["cursor"].to_string(),
            kind.to_owned(),
        );
        if !receipts.insert(identity) {
            out.raw_push.replayed += 1;
            continue;
        }
        out.raw_push.submitted += 1;
        *out.raw_push.by_kind.entry(kind.into()).or_default() += 1;
        // A receipt names its source event by reader cursor; only that event's identity joins.
        let source = event.detail["cursor"]["offset"]
            .as_u64()
            .and_then(|offset| cursors.iter().position(|end| *end == offset))
            .and_then(|index| events.get(index))
            .filter(|source| source.kind == "question_opened");
        let Some((question, asked)) = source.and_then(|source| {
            let key = source.detail["key"].as_str()?;
            let revision = source.detail["revision"].as_u64()?;
            Some((format!("/{key}@{revision}"), when(source)))
        }) else {
            continue;
        };
        // The source event names one question revision at one instant; later escalation or
        // unattended conditions for that revision were not what this push announced.
        let matches: Vec<_> = (rows.iter())
            .filter(|(key, row)| row.kind.starts_with("question_") && key.ends_with(&question))
            .filter(|(_, row)| open_at(row, asked))
            .map(|(key, _)| key)
            .collect();
        match matches.as_slice() {
            [key] => *out.raw_push.attributed.entry((*key).clone()).or_default() += 1,
            [] => {}
            _ => out.raw_push.ambiguous += 1,
        }
    }
}

#[derive(Default)]
struct Batch {
    producer: String,
    claimed_at: Option<Timestamp>,
    claim_ids: BTreeSet<String>,
    coverage: Vec<Value>,
    exempt: bool,
    outcome: Option<String>,
}
fn absorb(batches: &mut BTreeMap<String, Batch>, detail: &Value) -> Option<String> {
    let id = detail["batch_id"].as_str()?.to_owned();
    let batch = batches.entry(id.clone()).or_default();
    if batch.producer.is_empty() {
        batch.producer = detail["producer"].as_str().unwrap_or_default().into();
    }
    batch.claimed_at = batch.claimed_at.or_else(|| time(&detail["claimed_at"]));
    for claim in detail["claim_ids"].as_array().into_iter().flatten() {
        batch.claim_ids.extend(claim.as_str().map(String::from));
    }
    if batch.coverage.is_empty() {
        batch.coverage = detail["coverage"].as_array().cloned().unwrap_or_default();
    }
    batch.exempt |= detail["exemption_basis"] == "owner_responsible_entry";
    if let Some(kind) = detail["outcome"]["kind"].as_str() {
        batch.outcome.get_or_insert(kind.into());
    }
    Some(id)
}

fn notifications(
    facts: &[Fact],
    claims: &[NoticeClaim],
    rows: &BTreeMap<String, ConditionRow>,
    window: Window,
) -> Notifications {
    let mut batches = BTreeMap::<String, Batch>::new();
    let claimed = claims
        .iter()
        .filter_map(|claim| serde_json::to_value(claim).ok());
    for detail in facts
        .iter()
        .filter(|fact| {
            fact.kind.starts_with("notification_") || fact.kind.starts_with("owner_noti")
        })
        .map(|fact| fact.detail.clone())
        .chain(claimed)
    {
        absorb(&mut batches, &detail);
    }
    let mut out = Notifications::default();
    let mut claim_ids = BTreeSet::new();
    for (id, batch) in &batches {
        let (success, failure, unknown, skipped) = match batch.outcome.as_deref() {
            Some("notified") => (1, 0, 0, 0),
            Some("failed") => (0, 1, 0, 0),
            Some(_) => (0, 0, 0, 1),
            None => (0, 0, 1, 0),
        };
        // Membership is a property of the lifecycle facts, never of record retention.
        let linked = rows
            .values()
            .any(|row| row.owner_claim.as_ref() == Some(id))
            || batch.coverage.iter().any(|edge| {
                edge["condition_key"]
                    .as_str()
                    .is_some_and(|key| rows.contains_key(key))
            });
        if batch.claimed_at.is_some_and(|at| at < window.start) && linked {
            out.carry_in += 1;
            out.carry_in_calls[0] += success + failure;
            out.carry_in_calls[1] += success + failure + unknown;
            continue;
        }
        if !within(batch.claimed_at, window) {
            continue;
        }
        out.batches += 1;
        claim_ids.extend(batch.claim_ids.iter().cloned());
        out.coverage_edges += batch.coverage.len() as u64;
        out.outside_ladder += u64::from(batch.coverage.is_empty());
        out.exempt_owner_entry += u64::from(batch.exempt);
        let episode = batch.coverage.iter().any(|edge| edge["kind"] == "episode");
        let exact = batch.coverage.iter().any(|edge| edge["kind"] == "exact");
        out.mixed += u64::from(episode && exact);
        let producer = out.by_producer.entry(batch.producer.clone()).or_default();
        producer.batches += 1;
        producer.claims += batch.claim_ids.len() as u64;
        producer.success += success;
        producer.failure += failure;
        producer.outcome_unknown += unknown;
        producer.certainly_not_invoked += skipped;
        if !batch.exempt {
            out.budget_batches += 1;
            out.budget_calls[0] += success + failure;
            out.budget_calls[1] += success + failure + unknown;
        }
    }
    for producer in out.by_producer.values() {
        out.success += producer.success;
        out.failure += producer.failure;
        out.outcome_unknown += producer.outcome_unknown;
        out.certainly_not_invoked += producer.certainly_not_invoked;
    }
    out.claims = claim_ids.len() as u64;
    out.calls = [
        out.success + out.failure,
        out.success + out.failure + out.outcome_unknown,
    ];
    for row in rows.values() {
        match row.owner_claim.as_deref() {
            Some(claim) if claim.starts_with("legacy_unknown/") => out.legacy_links += 1,
            Some(claim) if batches.contains_key(claim) => out.condition_links += 1,
            _ => {}
        }
    }
    out
}

mod joins;
use joins::actions;

#[cfg(test)]
#[path = "metrics_tests.rs"]
mod tests;
