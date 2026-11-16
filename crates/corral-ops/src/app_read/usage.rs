//! Read-only, bounded accounting rollups; row limits produce explicit partial evidence.
//! Periods are UTC calendar windows. Undated usage never receives a fabricated day.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

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

#[path = "usage_aggregate.rs"]
mod aggregate;
use super::read_budget as budget;
#[path = "usage_quota.rs"]
mod quota;
#[path = "usage_rollup.rs"]
mod rollup;
use aggregate::{Aggregate, Total};

const PAGE: u32 = 256;
const MAX_ROWS: usize = 100_000;
const MAX_TASKS: usize = 20_000;
const DAY: i64 = 86_400;

type Snapshot = Arc<Aggregate>;


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
    facts: &mut Aggregate,
    usage: Usage,
    profile: Option<String>,
    cutoff: Timestamp,
    now: Timestamp,
) {
    if usage.entries.is_empty() {
        if facts.budget.admit(1) {
            facts.add(String::new(), profile, canonical_entry(&usage), cutoff, now);
        }
        return;
    }
    for entry in usage.entries {
        if !facts.budget.admit(1) {
            break;
        }
        let connection = entry
            .connection
            .map(|connection| connection.id().to_owned())
            .unwrap_or_default();
        facts.add(connection, profile.clone(), entry, cutoff, now);
    }
}

fn add_state(
    ctx: &Ctx,
    facts: &mut Aggregate,
    source: rollup::Source<'_>,
    fallback: Option<Usage>,
    cutoff: Timestamp,
    now: Timestamp,
) {
    let profile = source.profile.clone();
    let state = source.state;
    match rollup::state(ctx, source, cutoff, now, facts.budget.remaining()) {
        Ok(source) => facts.merge(&source),
        Err(_) => {
            if let Some(usage) = fallback {
                add_usage(facts, usage, profile, cutoff, now);
            }
            if !state.generations.is_empty() || !state.usage_sessions.is_empty() {
                facts.budget.partial("usage_source_unavailable");
            }
        }
    }
}

fn task_facts(
    ctx: &Ctx,
    num: corral_core::backend::TaskNum,
    cutoff: Timestamp,
    now: Timestamp,
    max: usize,
) -> Result<Aggregate> {
    let view = ctx.store.view()?;
    let mut facts = Aggregate::new(max);
    let mut owners = BTreeMap::new();
    for part in [TaskPart::Run, TaskPart::ReviewRun] {
        let mut page = PageReq::first(PAGE);
        loop {
            let found = view
                .rows(num, &RowQuery::part(part), page.clone())
                .context("reading usage run index")?;
            for row in found.items {
                if !facts.budget.admit(1) {
                    return Ok(facts);
                }
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
                owners.insert(owner, (profile, connection, run.model, run.usage, false));
            }
            match found.next {
                Some(next) => page.after = Some(next),
                None => break,
            }
        }
    }
    let mut page = PageReq::first(PAGE);
    loop {
        let found = view
            .rows(num, &RowQuery::part(TaskPart::RunUsage), page.clone())
            .context("reading canonical usage entries")?;
        for row in found.items {
            if !facts.budget.admit(1) {
                // Unscanned entries may belong to any owner: legacy totals must not
                // be substituted here, or a partial prefix would be counted twice.
                return Ok(facts);
            }
            let owner = row
                .key
                .rsplit_once('/')
                .context("invalid canonical usage owner key")?
                .0;
            let (profile, connection, model, _, has_entries) = owners
                .get_mut(owner)
                .context("usage entry has no run owner")?;
            *has_entries = true;
            let mut entry: UsageEntry =
                serde_json::from_value(row.doc).context("decoding canonical usage entry")?;
            entry.model = entry.model.or_else(|| model.clone());
            if entry.model.is_some() {
                entry.reprice(&ctx.store.config);
            }
            if entry.model.is_none() && entry.reported_usd.is_none() {
                entry.estimated_usd = None;
                entry.unpriced_tokens = entry.total_tokens();
            }
            let connection = entry
                .connection
                .map(|connection| connection.id().to_owned())
                .unwrap_or_else(|| connection.clone());
            facts.add(connection, profile.clone(), entry, cutoff, now);
        }
        match found.next {
            Some(next) => page.after = Some(next),
            None => break,
        }
    }
    for (_, (profile, connection, model, usage, has_entries)) in owners {
        if !has_entries && let Some(usage) = usage {
            // An aggregate-only run has no billing instant; preserve that uncertainty.
            let mut entry = UsageEntry {
                model,
                input: usage.input,
                output: usage.output,
                cache_read: usage.cache_read,
                cache_write: usage.cache_write,
                reported_usd: usage.reported_usd.or(usage.cost_usd),
                ..UsageEntry::default()
            };
            if entry.model.is_some() || entry.reported_usd.is_none() {
                entry.reprice(&ctx.store.config);
            }
            facts.add(connection, profile, entry, cutoff, now);
        }
    }
    Ok(facts)
}

fn facts(ctx: &Ctx, now: Timestamp) -> Result<Snapshot> {
    facts_with_limit(ctx, now, MAX_ROWS)
}

fn facts_with_limit(ctx: &Ctx, now: Timestamp, max_rows: usize) -> Result<Snapshot> {
    let day = now.as_second().div_euclid(DAY);
    let cutoff = Timestamp::from_second((day - 29) * DAY).context("computing usage window")?;
    let view = ctx.store.view()?;
    let mut all = Aggregate::new(usize::MAX);
    // Source budgets are independent: transcript history can never hide task accounting.
    let mut records = budget::Budget::new(max_rows);
    #[derive(Deserialize)]
    struct SessionRecord {
        session: corral_core::assistant::AssistantSession,
    }
    let mut page = PageReq::first(PAGE);
    let query = RecordQuery {
        area: Some("assistant-sessions".into()),
        key: None,
    };
    'sessions: loop {
        let found = view
            .records(&query, page.clone())
            .context("reading assistant accounting records")?;
        for row in found.items {
            if !records.admit(1) {
                break 'sessions;
            }
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
            let container = ctx.store.get_assistant_container(&record.session.id)?;
            let mut session = Aggregate::new(max_rows);
            session.budget.admit(1);
            add_state(
                ctx,
                &mut session,
                rollup::Source {
                    key: &format!("assistant:{}", record.session.id),
                    state: &container.state,
                    ended: record.session.ended.is_some(),
                    profile: Some(record.session.profile),
                },
                record.session.usage,
                cutoff,
                now,
            );
            all.merge(&session);
        }
        match found.next {
            Some(next) => page.after = Some(next),
            None => break,
        }
    }
    if records.coverage.partial {
        all.budget.partial("assistant_session_limit");
    }
    if let Some(pm) = corral_core::agent::pm_state::load_selected(
            &ctx.store,
            &corral_core::agent::pm_state::Selection::default()
                .family("generations")
                .family("usage_sessions"),
        )?
    {
        // Historical PM generations have no frozen profile; today's config is not evidence.
        let mut facts = Aggregate::new(max_rows);
        add_state(ctx, &mut facts, rollup::Source { key: "pm", state: &pm.state, ended: false, profile: None }, None, cutoff, now);
        all.merge(&facts);
    }
    let query = TaskQuery {
        statuses: Vec::new(),
        project: None,
        order: TaskOrder::UpdatedDesc,
    };
    let mut page = PageReq::first(PAGE);
    let pricing = rollup::pricing(ctx)?;
    let mut count = 0;
    'tasks: loop {
        let found = view
            .tasks(&query, page.clone())
            .context("reading recent usage task index")?;
        for header in found.items {
            if header.fields.updated < cutoff {
                break 'tasks;
            }
            if count == MAX_TASKS {
                all.budget.partial("task_limit");
                break 'tasks;
            }
            count += 1;
            let key = format!("task:{}", header.id);
            let identity = rollup::fingerprint(&(header.rev, header.fields.updated, &pricing))?;
            let task = match rollup::load::<aggregate::Saved>(ctx, &key, &identity).filter(|saved| saved.valid(now)) {
                Some(saved) => saved.restore(cutoff),
                None => {
                    // Compact canonical accounting, not transcript replay. A revision is
                    // a complete immutable snapshot, rebuilt when any run usage changes.
                    let task = task_facts(ctx, header.num, Timestamp::MIN, now, usize::MAX)?;
                    if !task.budget.coverage.partial && !task.future {
                        rollup::save(ctx, &key, &identity, &aggregate::Saved::new(&task));
                    }
                    task.window(cutoff)
                }
            };
            all.merge(&task);
        }
        match found.next {
            Some(next) => page.after = Some(next),
            None => break,
        }
    }
    Ok(Arc::new(all))
}

fn day_string(day: i64) -> Result<String> {
    Ok(Timestamp::from_second(day * DAY)
        .context("formatting usage day")?
        .to_zoned(jiff::tz::TimeZone::UTC)
        .date()
        .to_string())
}

/// One row per connection/profile, including quota-only observations.
pub fn usage_rows(ctx: &Ctx) -> Result<Vec<UsageRow>> {
    let now = Timestamp::now();
    let today = now.as_second().div_euclid(DAY);
    let facts = facts(ctx, now)?;
    let (mut quotas, quota_coverage) = quota::read(ctx, now)?;
    let keys: BTreeSet<_> = facts.groups.keys().chain(quotas.keys()).cloned().collect();
    let mut rows = Vec::new();
    for (connection, profile) in keys {
        let mut day_total = Total::default();
        let mut week_total = Total::default();
        let mut undated = 0;
        if let Some(days) = facts.groups.get(&(connection.clone(), profile.clone())) {
            for (day, total) in days {
                match day {
                    Some(day) if *day == today => day_total.merge(total),
                    None => undated += total.entries(),
                    _ => {}
                }
                if day.is_some_and(|day| day >= today - 6) {
                    week_total.merge(total);
                }
            }
        }
        let mut observations = quotas
            .remove(&(connection.clone(), profile.clone()))
            .unwrap_or_else(|| vec![quota::unknown()]);
        if quota_coverage.partial {
            let mut unknown = quota::unknown();
            unknown.completeness = quota_coverage.clone();
            observations.push(unknown);
        }
        let observed = observations
            .iter()
            .filter(|quota| quota.source != "unknown")
            .max_by(|a, b| a.observed_at.cmp(&b.observed_at));
        let mut row = UsageRow {
            id: serde_json::to_string(&(&connection, &profile))?,
            connection,
            profile,
            window: None,
            used: None,
            limit: None,
            limit_basis: if observed.is_some() {
                LimitBasis::Observed
            } else {
                LimitBasis::None
            },
            resets_at: observed.and_then(|quota| quota.resets_at.clone()),
            today_usd: day_total.cost(),
            week_usd: week_total.cost(),
            cost_basis: match (day_total.basis(), week_total.basis()) {
                (Some(a), Some(b)) if a != b => Some(CostBasis::Mixed),
                (Some(basis), _) | (_, Some(basis)) => Some(basis),
                _ => None,
            },
            today_completeness: day_total.completeness(&facts.budget.coverage, undated),
            week_completeness: week_total.completeness(&facts.budget.coverage, undated),
            quotas: observations,
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
    let mut profiles: BTreeMap<i64, Vec<ProfileUsage>> = BTreeMap::new();
    let mut provider_undated: BTreeMap<&str, u64> = BTreeMap::new();
    let mut series_total = Total::default();
    for ((connection, profile), days) in &facts.groups {
        let undated = days.get(&None).map_or(0, Total::entries);
        *provider_undated.entry(connection).or_default() += undated;
        for (day, total) in days {
            let Some(day) = day else {
                continue;
            };
            if *day < first_day {
                continue;
            }
            series_total.merge(total);
            providers
                .entry(connection)
                .or_default()
                .entry(*day)
                .or_default()
                .merge(total);
            profiles.entry(*day).or_default().push(ProfileUsage {
                profile: profile.as_deref().unwrap_or_default().to_owned(),
                connection: connection.to_owned(),
                cost_usd: total.cost(),
                cost_basis: total.basis(),
                tokens: Some(total.tokens),
                completeness: total.completeness(&facts.budget.coverage, undated),
            });
        }
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
                completeness: total.completeness(
                    &facts.budget.coverage,
                    provider_undated.get(id).copied().unwrap_or(0),
                ),
            });
        }
        provider_series.push(ProviderSeries {
            id: id.to_owned(),
            points,
        });
    }
    let mut usage_days = Vec::new();
    for (day, by_profile) in profiles {
        usage_days.push(UsageDay {
            day: day_string(day)?,
            by_profile,
        });
    }
    Ok(DetailBody::UsageSeries(UsageSeries {
        providers: provider_series,
        days: usage_days,
        completeness: series_total.completeness(
            &facts.budget.coverage,
            facts.budget.coverage.undated_entries,
        ),
    }))
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "usage_rollup_tests.rs"]
mod rollup_tests;

#[cfg(test)]
#[path = "usage_persistence_tests.rs"]
mod persistence_tests;
