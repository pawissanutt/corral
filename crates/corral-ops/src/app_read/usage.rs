//! Read-only accounting projections. Costs use recorded reported values before recorded
//! API-equivalent estimates; quota is unknown until a canonical quota source exists.
//!
//! Periods are UTC calendar windows (`today`, `7d`/`week`, `30d`/`month`). Undated
//! legacy accounting remains visible as a row, but is never assigned a fabricated day.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use corral_core::backend::{PageReq, RecordQuery, RowQuery, TaskOrder, TaskPart, TaskQuery};
use corral_core::{RunHead, Usage, UsageEntry};
use corral_protocol::board::CostBasis;
use corral_protocol::detail::{
    DetailBody, ProfileUsage, ProviderSeries, UsageDay, UsagePoint, UsageSeries,
};
use corral_protocol::rows::{LimitBasis, UsageRow};
use jiff::Timestamp;
use serde::Deserialize;

use crate::Ctx;

const PAGE: u32 = 256;
const MAX_ROWS: usize = 100_000;
const MAX_TASKS: usize = 20_000;
const DAY: i64 = 86_400;

struct Fact {
    connection: String,
    profile: Option<String>,
    entry: UsageEntry,
}
type Facts = Arc<Vec<Fact>>;
type Snapshot = Arc<Vec<Facts>>;

#[derive(Default)]
struct Cached {
    tasks: BTreeMap<String, (u64, Facts)>,
    snapshot: Option<(u64, i64, Instant, Snapshot)>,
}

// Repeated usage subscriptions do not reassemble task history. On a new store sequence,
// only changed task revisions are decoded; the index walk is bounded and header-only.
static CACHE: LazyLock<Mutex<VecDeque<(PathBuf, Cached)>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));

fn charge(budget: &mut usize, count: usize) -> Result<()> {
    *budget += count;
    ensure!(
        *budget <= MAX_ROWS,
        "unavailable: usage query exceeds {MAX_ROWS} canonical accounting rows"
    );
    Ok(())
}

fn canonical_entry(usage: &Usage) -> UsageEntry {
    UsageEntry {
        input: usage.input,
        output: usage.output,
        cache_read: usage.cache_read,
        cache_write: usage.cache_write,
        reported_usd: usage.reported_usd.or(usage.cost_usd),
        estimated_usd: usage.estimated_usd,
        unpriced_tokens: usage.unpriced_tokens,
        ..UsageEntry::default()
    }
}

fn add_usage(
    facts: &mut Vec<Fact>,
    usage: Usage,
    profile: Option<String>,
    cutoff: Timestamp,
    budget: &mut usize,
) -> Result<()> {
    let entries = if usage.entries.is_empty() {
        vec![canonical_entry(&usage)]
    } else {
        usage.entries
    };
    charge(budget, entries.len())?;
    for entry in entries {
        if entry.at.is_some_and(|at| at < cutoff) {
            continue;
        }
        facts.push(Fact {
            connection: entry
                .connection
                .map(|connection| connection.id().to_owned())
                .unwrap_or_default(),
            profile: profile.clone(),
            entry,
        });
    }
    Ok(())
}

fn task_facts(
    ctx: &Ctx,
    num: corral_core::backend::TaskNum,
    cutoff: Timestamp,
    budget: &mut usize,
) -> Result<Vec<Fact>> {
    let view = ctx.store.view()?;
    let mut owners = BTreeMap::new();
    for part in [TaskPart::Run, TaskPart::ReviewRun] {
        let mut page = PageReq::first(PAGE);
        loop {
            let found = view
                .rows(num, &RowQuery::part(part), page.clone())
                .context("reading usage run index")?;
            charge(budget, found.items.len())?;
            for row in found.items {
                let run: RunHead = serde_json::from_value(row.doc).context("decoding usage run")?;
                let owner = if part == TaskPart::Run {
                    format!("{}/000000", row.key)
                } else {
                    row.key
                };
                let connection = run
                    .execution
                    .as_ref()
                    .and_then(|execution| execution.resolved.connection)
                    .map(|connection| connection.id().to_owned())
                    .unwrap_or_default();
                let profile = run
                    .profile
                    .or_else(|| (!run.agent.is_empty()).then_some(run.agent));
                owners.insert(owner, (profile, connection, run.usage, false));
            }
            match found.next {
                Some(next) => page.after = Some(next),
                None => break,
            }
        }
    }
    let mut facts = Vec::new();
    let mut page = PageReq::first(PAGE);
    loop {
        let found = view
            .rows(num, &RowQuery::part(TaskPart::RunUsage), page.clone())
            .context("reading canonical usage entries")?;
        charge(budget, found.items.len())?;
        for row in found.items {
            let owner = row
                .key
                .rsplit_once('/')
                .context("invalid canonical usage owner key")?
                .0;
            let (profile, connection, _, has_entries) = owners
                .get_mut(owner)
                .context("usage entry has no run owner")?;
            *has_entries = true;
            let entry: UsageEntry =
                serde_json::from_value(row.doc).context("decoding canonical usage entry")?;
            if entry.at.is_some_and(|at| at < cutoff) {
                continue;
            }
            facts.push(Fact {
                connection: entry
                    .connection
                    .map(|connection| connection.id().to_owned())
                    .unwrap_or_else(|| connection.clone()),
                profile: profile.clone(),
                entry,
            });
        }
        match found.next {
            Some(next) => page.after = Some(next),
            None => break,
        }
    }
    for (_, (profile, connection, usage, has_entries)) in owners {
        if has_entries {
            continue;
        }
        if let Some(usage) = usage {
            // Legacy aggregate-only evidence has no response timestamp. Do not use the
            // run's start/end as a surrogate billing time or double-count nested reviews.
            facts.push(Fact {
                connection,
                profile,
                entry: UsageEntry {
                    input: usage.input,
                    output: usage.output,
                    cache_read: usage.cache_read,
                    cache_write: usage.cache_write,
                    reported_usd: usage.reported_usd.or(usage.cost_usd),
                    estimated_usd: usage.estimated_usd,
                    unpriced_tokens: usage.unpriced_tokens,
                    ..UsageEntry::default()
                },
            });
        }
    }
    Ok(facts)
}

fn facts(ctx: &Ctx, now: Timestamp) -> Result<Snapshot> {
    let day = now.as_second().div_euclid(DAY);
    let cutoff = Timestamp::from_second((day - 29) * DAY).context("computing usage window")?;
    let view = ctx.store.view()?;
    let seq = view.seq().0;
    let mut cache = CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let key = ctx.store.dir().to_path_buf();
    let index = cache.iter().position(|(path, _)| *path == key);
    let mut cached = index
        .and_then(|index| cache.remove(index))
        .map(|(_, cached)| cached)
        .unwrap_or_default();
    if let Some((saved_seq, saved_day, at, snapshot)) = &cached.snapshot
        && *saved_seq == seq
        && *saved_day == day
        && at.elapsed() < Duration::from_secs(2)
    {
        let snapshot = Arc::clone(snapshot);
        cache.push_back((key, cached));
        return Ok(snapshot);
    }
    // A replaced store or rolled-back snapshot must not reuse revision-identical history.
    if cached
        .snapshot
        .as_ref()
        .is_some_and(|(saved, saved_day, _, _)| *saved > seq || *saved_day != day)
    {
        cached.tasks.clear();
    }
    let query = TaskQuery {
        statuses: Vec::new(),
        project: None,
        order: TaskOrder::UpdatedDesc,
    };
    let mut page = PageReq::first(PAGE);
    let mut seen = BTreeSet::new();
    let mut budget = 0;
    let mut count = 0;
    'tasks: loop {
        let found = view
            .tasks(&query, page.clone())
            .context("reading recent usage task index")?;
        count += found.items.len();
        ensure!(
            count <= MAX_TASKS,
            "unavailable: usage query exceeds {MAX_TASKS} recent tasks"
        );
        for header in found.items {
            // UpdatedDesc is index-ordered. Accounting writes update the owning task;
            // no older header can contribute newly recorded usage in this UTC window.
            if header.fields.updated < cutoff {
                break 'tasks;
            }
            seen.insert(header.id.clone());
            if cached
                .tasks
                .get(&header.id)
                .is_none_or(|(rev, _)| *rev != header.rev)
            {
                let facts = task_facts(ctx, header.num, cutoff, &mut budget)?;
                cached
                    .tasks
                    .insert(header.id, (header.rev, Arc::new(facts)));
            }
        }
        match found.next {
            Some(next) => page.after = Some(next),
            None => break,
        }
    }
    cached.tasks.retain(|id, _| seen.contains(id));
    let mut all = Vec::with_capacity(cached.tasks.len() + 1);
    let mut total_entries = 0;
    for (_, entries) in cached.tasks.values() {
        total_entries += entries.len();
        ensure!(
            total_entries <= MAX_ROWS,
            "unavailable: usage snapshot exceeds {MAX_ROWS} entries"
        );
        all.push(Arc::clone(entries));
    }
    let mut session_facts = Vec::new();
    // Session records, rather than artifact paths or fixture projections, own assistant
    // accounting. The shared collector reads only each container's bound generations.
    #[derive(Deserialize)]
    struct SessionRecord {
        session: corral_core::assistant::AssistantSession,
    }
    let mut page = PageReq::first(PAGE);
    let query = RecordQuery {
        area: Some("assistant-sessions".into()),
        key: None,
    };
    loop {
        let found = view
            .records(&query, page.clone())
            .context("reading assistant accounting records")?;
        charge(&mut budget, found.items.len())?;
        for row in found.items {
            let record: SessionRecord =
                serde_json::from_value(row.doc).context("decoding assistant accounting record")?;
            corral_core::assistant::validate_id(&record.session.id)?;
            ensure!(
                record.session.id == row.key,
                "mismatched assistant accounting record"
            );
            if record.session.ended.is_some_and(|at| at < cutoff) {
                continue;
            }
            if let Some(usage) = crate::usage::assistant_usage(&ctx.store, &record.session)? {
                add_usage(
                    &mut session_facts,
                    usage,
                    Some(record.session.profile),
                    cutoff,
                    &mut budget,
                )?;
            }
        }
        match found.next {
            Some(next) => page.after = Some(next),
            None => break,
        }
    }
    if let Some(pm) = corral_core::agent::pm_state::load_selected(
        &ctx.store,
        &corral_core::agent::pm_state::Selection::default()
            .family("generations")
            .family("usage_sessions"),
    )? {
        // PM has no frozen profile key in its canonical binding. Keep it None rather
        // than attributing old generations to today's mutable PM configuration.
        if let Ok(usage) = crate::agent::usage::collect_state(&pm.state, None, &ctx.store) {
            add_usage(&mut session_facts, usage, None, cutoff, &mut budget)?;
        }
    }
    ensure!(
        total_entries + session_facts.len() <= MAX_ROWS,
        "unavailable: usage snapshot exceeds {MAX_ROWS} entries"
    );
    all.push(Arc::new(session_facts));
    let all = Arc::new(all);
    cached.snapshot = Some((seq, day, Instant::now(), Arc::clone(&all)));
    cache.push_back((key, cached));
    while cache.len() > 4 {
        cache.pop_front();
    }
    Ok(all)
}

#[derive(Default)]
struct Total {
    usd: f64,
    priced: usize,
    unpriced: usize,
    tokens: u64,
    reported: bool,
    estimated: bool,
}

impl Total {
    fn add(&mut self, entry: &UsageEntry) {
        self.tokens = self.tokens.saturating_add(entry.total_tokens());
        if let Some(cost) = entry
            .reported_usd
            .filter(|cost| cost.is_finite() && *cost >= 0.0)
        {
            self.usd += cost;
            self.priced += 1;
            self.reported = true;
        } else if let Some(cost) = entry
            .estimated_usd
            .filter(|cost| cost.is_finite() && *cost >= 0.0)
        {
            self.usd += cost;
            self.priced += 1;
            self.estimated = true;
        } else {
            self.unpriced += 1;
        }
    }

    fn cost(&self) -> Option<f64> {
        (self.priced > 0 && self.unpriced == 0 && self.usd.is_finite()).then_some(self.usd)
    }

    fn basis(&self) -> Option<CostBasis> {
        self.cost()?;
        match (self.reported, self.estimated) {
            (true, true) => Some(CostBasis::Mixed),
            (true, false) => Some(CostBasis::Reported),
            (false, true) => Some(CostBasis::Estimated),
            (false, false) => None,
        }
    }
}

fn day_string(day: i64) -> Result<String> {
    Ok(Timestamp::from_second(day * DAY)
        .context("formatting usage day")?
        .to_zoned(jiff::tz::TimeZone::UTC)
        .date()
        .to_string())
}

/// One row per observed connection/profile pair; no duplicate provider-total rows.
pub fn usage_rows(ctx: &Ctx) -> Result<Vec<UsageRow>> {
    let now = Timestamp::now();
    let today = now.as_second().div_euclid(DAY);
    let facts = facts(ctx, now)?;
    let mut groups: BTreeMap<(&str, Option<&str>), (Total, Total)> = BTreeMap::new();
    for fact in facts.iter().flat_map(|facts| facts.iter()) {
        let totals = groups
            .entry((&fact.connection, fact.profile.as_deref()))
            .or_default();
        let Some(at) = fact.entry.at else {
            continue;
        };
        if at > now {
            continue;
        }
        let day = at.as_second().div_euclid(DAY);
        if day == today {
            totals.0.add(&fact.entry);
        }
        if day >= today - 6 {
            totals.1.add(&fact.entry);
        }
    }
    let mut rows = Vec::new();
    for ((connection, profile), (today, week)) in groups {
        let mut row = UsageRow {
            id: serde_json::to_string(&(connection, profile))?,
            connection: connection.to_owned(),
            profile: profile.map(str::to_owned),
            window: None,
            used: None,
            limit: None,
            limit_basis: LimitBasis::None,
            resets_at: None,
            today_usd: today.cost(),
            week_usd: week.cost(),
            cost_basis: match (today.basis(), week.basis()) {
                (Some(a), Some(b)) if a != b => Some(CostBasis::Mixed),
                (Some(basis), _) | (_, Some(basis)) => Some(basis),
                _ => None,
            },
            rev: 0,
        };
        row.rev = super::revision(&row)?;
        rows.push(row);
    }
    Ok(rows)
}

/// Provider points are actual daily accounting, not made-up subscription headroom.
pub(super) fn usage_series(ctx: &Ctx, period: &str) -> Result<DetailBody> {
    let days = match period {
        "today" => 1,
        "7d" | "week" => 7,
        "30d" | "month" => 30,
        _ => anyhow::bail!("invalid usage period: use today, 7d or 30d"),
    };
    let now = Timestamp::now();
    let first_day = now.as_second().div_euclid(DAY) - days + 1;
    let facts = facts(ctx, now)?;
    let mut providers: BTreeMap<&str, BTreeMap<i64, Total>> = BTreeMap::new();
    let mut profiles: BTreeMap<i64, BTreeMap<&str, Total>> = BTreeMap::new();
    for fact in facts.iter().flat_map(|facts| facts.iter()) {
        let Some(at) = fact.entry.at else {
            continue;
        };
        let day = at.as_second().div_euclid(DAY);
        if day < first_day || at > now {
            continue;
        }
        providers
            .entry(&fact.connection)
            .or_default()
            .entry(day)
            .or_default()
            .add(&fact.entry);
        profiles
            .entry(day)
            .or_default()
            .entry(fact.profile.as_deref().unwrap_or_default())
            .or_default()
            .add(&fact.entry);
    }
    let mut provider_series = Vec::new();
    for (id, days) in providers {
        let mut points = Vec::new();
        for (day, total) in days {
            points.push(UsagePoint {
                at: Timestamp::from_second(day * DAY)
                    .context("formatting provider usage day")?
                    .to_string(),
                used: None,
                limit: None,
                cost_usd: total.cost(),
                cost_basis: total.basis(),
            });
        }
        provider_series.push(ProviderSeries {
            id: id.to_owned(),
            points,
        });
    }
    let mut usage_days = Vec::new();
    for (day, profiles) in profiles {
        let mut by_profile = Vec::new();
        for (profile, total) in profiles {
            by_profile.push(ProfileUsage {
                profile: profile.to_owned(),
                cost_usd: total.cost(),
                cost_basis: total.basis(),
                tokens: Some(total.tokens),
            });
        }
        usage_days.push(UsageDay {
            day: day_string(day)?,
            by_profile,
        });
    }
    Ok(DetailBody::UsageSeries(UsageSeries {
        providers: provider_series,
        days: usage_days,
    }))
}
