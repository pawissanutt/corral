use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result, ensure};
pub use corral_types::{UsageBy, UsageDay, UsageGroup, UsageSummary, UsageTotal};
use jiff::{Timestamp, civil::Date, tz::Offset};
use rusqlite::{Row, params, params_from_iter};

use crate::{Engine, Store, Usage, store::encode};

// Jiff stores UTC timestamps with optional fractional seconds. Sort by a
// biased unsigned year (including BCE), then by the remaining ISO text.
// The midnight bounds have no fraction; dropping `Z` places a bound before
// every stored timestamp in its second, including the exact instant.
macro_rules! instant_key {
    ($value:literal) => {
        concat!(
            "printf('%05d%s', 10000 + CAST(substr(",
            $value,
            ", 1, 7) AS INTEGER), ",
            "substr(",
            $value,
            ", instr(",
            $value,
            ", 'T') - 5))"
        )
    };
}

// Do not require a matching run: usage with partial or absent IDs still counts.
pub(crate) const SCHEMA: &str = concat!(
    "CREATE TABLE IF NOT EXISTS usage(seq INTEGER PRIMARY KEY, task TEXT, run TEXT, document TEXT NOT NULL);
     CREATE INDEX IF NOT EXISTS usage_task_run ON usage(task,run);
     CREATE INDEX IF NOT EXISTS usage_at ON usage(", instant_key!("json_extract(document, '$.at')"), ");
     CREATE INDEX IF NOT EXISTS runs_started ON runs(", instant_key!("json_extract(document, '$.started')"), ");"
);

const USAGE_AT: &str = instant_key!("json_extract(usage.document, '$.at')");
const RUN_START: &str = "json_extract(runs.document, '$.started')";
const RUN_START_KEY: &str = instant_key!("json_extract(runs.document, '$.started')");
const USAGE_JOIN: &str =
    "FROM usage LEFT JOIN runs ON runs.task = usage.task AND runs.id = usage.run";

/// A validated range of 1–366 local days at a single fixed offset east of UTC.
pub struct UsageWindow {
    from: Date,
    offset: Offset,
    days: usize,
    bounds: [String; 2],
}

impl UsageWindow {
    pub fn parse(from: &str, to: &str, utc_offset_minutes: i16) -> Result<Self> {
        let from = parse_date(from).context("invalid usage start date")?;
        let to = parse_date(to).context("invalid usage end date")?;
        let days = from.duration_until(to).as_secs() / 86_400;
        ensure!(
            (1..=366).contains(&days),
            "usage range must span 1 to 366 days"
        );
        ensure!(
            (-1439..=1439).contains(&utc_offset_minutes),
            "usage UTC offset must be less than 24 hours"
        );
        let offset = Offset::from_seconds(i32::from(utc_offset_minutes) * 60)?;
        let bound = |day: Date| -> Result<String> {
            let mut text = offset.to_timestamp(day.at(0, 0, 0, 0))?.to_string();
            let _ = text.pop();
            Ok(text)
        };
        Ok(Self {
            from,
            offset,
            days: days as usize,
            bounds: [bound(from)?, bound(to)?],
        })
    }

    fn bucket(&self, day: Date) -> usize {
        (self.from.duration_until(day).as_secs() / 86_400) as usize
    }
}

fn parse_date(value: &str) -> Result<Date> {
    let bytes = value.as_bytes();
    ensure!(
        bytes.len() == 10
            && bytes.iter().enumerate().all(|(index, &byte)| match index {
                4 | 7 => byte == b'-',
                _ => byte.is_ascii_digit(),
            }),
        "expected a date in YYYY-MM-DD format"
    );
    Ok(value.parse()?)
}

#[derive(Default)]
struct Bucket {
    total: UsageTotal,
    groups: BTreeMap<Option<GroupKey>, GroupTally>,
    total_runs: u64,
    // Matched run rowids. The flag tracks coverage in that run's own group.
    covered: HashMap<i64, bool>,
}

#[derive(Default)]
struct GroupTally {
    total: UsageTotal,
    covered_runs: u64,
    total_runs: u64,
}

impl Bucket {
    fn into_summary(self) -> UsageSummary {
        UsageSummary {
            total: self.total,
            covered_runs: self.covered.len() as u64,
            total_runs: self.total_runs,
            groups: self
                .groups
                .into_iter()
                .map(|(key, tally)| UsageGroup {
                    key: key.map(GroupKey::into_label),
                    total: tally.total,
                    covered_runs: Some(tally.covered_runs),
                    total_runs: Some(tally.total_runs),
                })
                .collect(),
        }
    }
}

fn add(total: &mut UsageTotal, usage: &Usage) -> Result<()> {
    total.requests += 1;
    total.input = total
        .input
        .checked_add(usage.input)
        .context("usage input total overflow")?;
    total.output = total
        .output
        .checked_add(usage.output)
        .context("usage output total overflow")?;
    total.cache_read = total
        .cache_read
        .checked_add(usage.cache_read)
        .context("usage cache-read total overflow")?;
    total.cache_write = total
        .cache_write
        .checked_add(usage.cache_write)
        .context("usage cache-write total overflow")?;
    if let Some(price) = usage.usd {
        let usd = total.usd.unwrap_or_default() + price;
        ensure!(usd.is_finite(), "usage USD total overflow");
        total.priced_requests += 1;
        total.usd = Some(usd);
    }
    Ok(())
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum GroupKey {
    Text(String),
    Day(Date),
}

impl GroupKey {
    fn into_label(self) -> String {
        match self {
            Self::Text(text) => text,
            Self::Day(date) => date.to_string(),
        }
    }
}

fn text<'a>(row: &'a Row<'_>, column: usize) -> Result<Option<&'a str>> {
    Ok(row.get_ref(column)?.as_str_or_null()?)
}

fn local_start(row: &Row<'_>, column: usize, offset: Offset) -> Result<Option<Date>> {
    text(row, column)?
        .map(|value| {
            let start: Timestamp = value.parse().context("parse usage run start")?;
            Ok(offset.to_datetime(start).date())
        })
        .transpose()
}

fn select_bucket<'a>(
    buckets: &'a mut [Bucket],
    window: Option<&UsageWindow>,
    day: Option<Date>,
) -> Result<&'a mut Bucket> {
    let index = match window {
        Some(window) => window.bucket(day.context("usage is missing its local day")?),
        None => 0,
    };
    buckets
        .get_mut(index)
        .context("usage is outside its selected window")
}

fn add_bounds(query: &mut String, key: &str) {
    query.push_str(" WHERE ");
    query.push_str(key);
    query.push_str(concat!(" >= ", instant_key!("?1"), " AND "));
    query.push_str(key);
    query.push_str(concat!(" < ", instant_key!("?2")));
}

fn bounds(window: Option<&UsageWindow>) -> impl rusqlite::Params {
    params_from_iter(window.into_iter().flat_map(|window| &window.bounds))
}

fn run_group_column(by: UsageBy) -> &'static str {
    match by {
        UsageBy::Profile => "json_extract(runs.document, '$.profile')",
        UsageBy::Model => "json_extract(runs.document, '$.model')",
        UsageBy::Task => "runs.task",
        UsageBy::Day => "NULL",
    }
}

fn matches_group(key: Option<&GroupKey>, run_group: Option<&str>, run_day: Option<Date>) -> bool {
    match key {
        Some(GroupKey::Text(key)) => run_group == Some(key.as_str()),
        Some(GroupKey::Day(day)) => run_day == Some(*day),
        None => run_group.is_none(),
    }
}

fn take_key(by: UsageBy, usage: &mut Usage, day: Option<Date>) -> Option<GroupKey> {
    let label = match by {
        UsageBy::Profile => usage.profile.take(),
        UsageBy::Model => Some(std::mem::take(&mut usage.model)),
        UsageBy::Task => usage.task.take(),
        UsageBy::Day => return day.map(GroupKey::Day),
    };
    label.map(GroupKey::Text)
}

impl Engine {
    pub fn record_usage(&self, usage: &Usage) -> Result<()> {
        // JSON would otherwise silently encode a non-finite price as null.
        ensure!(
            usage.usd.is_none_or(f64::is_finite),
            "usage USD price must be finite"
        );
        let document = encode(usage)?;
        self.store.transaction(|store| {
            store
                .connection
                .execute(
                    "INSERT INTO usage(task,run,document) VALUES(?1,?2,?3)",
                    params![usage.task, usage.run, document],
                )
                .context("record usage")?;
            Ok(())
        })
    }

    /// Roll up all recorded requests. Grouping does not change overall run coverage.
    pub fn usage_summary(&self, by: Option<UsageBy>) -> Result<UsageSummary> {
        self.usage_snapshot(|| {
            if by.is_none() {
                let mut total = UsageTotal::default();
                self.store.read_usage(
                    "SELECT document FROM usage ORDER BY seq",
                    None,
                    |usage, _| add(&mut total, &usage),
                )?;
                let (total_runs, covered_runs) = self.store.usage_run_counts()?;
                return Ok(UsageSummary {
                    total,
                    total_runs,
                    covered_runs,
                    ..UsageSummary::default()
                });
            }
            let mut bucket = Bucket::default();
            self.roll_up_usage(by, None, std::slice::from_mut(&mut bucket))?;
            Ok(bucket.into_summary())
        })
    }

    /// Roll up requests by local report day against runs started on that day.
    pub fn usage_series(&self, window: UsageWindow, by: Option<UsageBy>) -> Result<Vec<UsageDay>> {
        self.usage_snapshot(|| {
            let mut buckets: Vec<_> = (0..window.days).map(|_| Bucket::default()).collect();
            self.roll_up_usage(by, Some(&window), &mut buckets)?;
            let mut day = window.from;
            let mut days = Vec::with_capacity(window.days);
            for bucket in buckets {
                days.push(UsageDay {
                    day: day.to_string(),
                    summary: bucket.into_summary(),
                });
                day = day.tomorrow()?;
            }
            Ok(days)
        })
    }

    fn usage_snapshot<T>(&self, read: impl FnOnce() -> Result<T>) -> Result<T> {
        if self.store.connection.is_autocommit() {
            self.store.read_snapshot(read)
        } else {
            read()
        }
    }

    fn roll_up_usage(
        &self,
        by: Option<UsageBy>,
        window: Option<&UsageWindow>,
        buckets: &mut [Bucket],
    ) -> Result<()> {
        let local_offset = window
            .map(|window| window.offset)
            .or_else(|| (by == Some(UsageBy::Day)).then_some(Offset::ZERO));
        let group = by.map_or("NULL", run_group_column);
        let started = if local_offset.is_some() {
            RUN_START
        } else {
            "NULL"
        };
        self.store.tally_runs(by, window, buckets, local_offset)?;
        let mut query =
            format!("SELECT usage.document, runs.rowid, {group}, {started} {USAGE_JOIN}");
        if window.is_some() {
            add_bounds(&mut query, USAGE_AT);
            query.push_str(" ORDER BY ");
            query.push_str(USAGE_AT);
        } else {
            query.push_str(" ORDER BY usage.seq");
        }
        self.store.read_usage(&query, window, |mut usage, row| {
            let day = local_offset.map(|offset| offset.to_datetime(usage.at).date());
            let bucket = select_bucket(buckets, window, day)?;
            add(&mut bucket.total, &usage)?;
            let run_day = if let Some(offset) = local_offset {
                local_start(row, 3, offset)?
            } else {
                None
            };
            let covered = row
                .get::<_, Option<i64>>(1)?
                .filter(|_| window.is_none() || run_day == day)
                .map(|run| bucket.covered.entry(run).or_default());
            if let Some(by) = by {
                let key = take_key(by, &mut usage, day);
                let matched = if let Some(covered) = covered.filter(|covered| !**covered) {
                    *covered = matches_group(key.as_ref(), text(row, 2)?, run_day);
                    *covered
                } else {
                    false
                };
                let group = bucket.groups.entry(key).or_default();
                group.covered_runs += u64::from(matched);
                add(&mut group.total, &usage)?;
            }
            Ok(())
        })
    }
}

impl Store {
    /// List recorded requests in insertion order, resolving missing harnesses
    /// only from a run matching both stored IDs.
    pub fn list_usage(&self) -> Result<Vec<Usage>> {
        let query = format!(
            "SELECT usage.document, json_extract(runs.document, '$.harness') {USAGE_JOIN} ORDER BY usage.seq"
        );
        let mut usage = Vec::new();
        self.read_usage(&query, None, |mut request, row| {
            if request.harness.is_none() {
                request.harness = row.get(1).context("read usage run harness")?;
            }
            usage.push(request);
            Ok(())
        })?;
        Ok(usage)
    }

    fn usage_run_counts(&self) -> Result<(u64, u64)> {
        self.connection
            .query_row(
                "SELECT count(*), count(*) FILTER (WHERE EXISTS (
                     SELECT 1 FROM usage WHERE usage.task = runs.task AND usage.run = runs.id
                 )) FROM runs",
                [],
                |row| Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)? as u64)),
            )
            .context("count usage run coverage")
    }

    fn tally_runs(
        &self,
        by: Option<UsageBy>,
        window: Option<&UsageWindow>,
        buckets: &mut [Bucket],
        local_offset: Option<Offset>,
    ) -> Result<()> {
        let group = by.map_or("NULL", run_group_column);
        let started = if local_offset.is_some() {
            RUN_START
        } else {
            "NULL"
        };
        let mut query = format!("SELECT {group}, {started} FROM runs");
        if window.is_some() {
            add_bounds(&mut query, RUN_START_KEY);
        }
        let mut statement = self
            .connection
            .prepare(&query)
            .context("prepare usage run cohorts")?;
        let mut rows = statement
            .query(bounds(window))
            .context("query usage run cohorts")?;
        while let Some(row) = rows.next().context("read usage run cohort")? {
            let day = if let Some(offset) = local_offset {
                local_start(row, 1, offset)?
            } else {
                None
            };
            let bucket = select_bucket(buckets, window, day)?;
            bucket.total_runs += 1;
            if let Some(by) = by {
                let key = match by {
                    UsageBy::Day => Some(GroupKey::Day(day.context("missing run start day")?)),
                    _ => text(row, 0)?.map(|label| GroupKey::Text(label.to_owned())),
                };
                bucket.groups.entry(key).or_default().total_runs += 1;
            }
        }
        Ok(())
    }

    fn read_usage(
        &self,
        query: &str,
        window: Option<&UsageWindow>,
        mut consume: impl FnMut(Usage, &Row<'_>) -> Result<()>,
    ) -> Result<()> {
        let mut statement = self
            .connection
            .prepare(query)
            .context("prepare usage listing")?;
        let mut rows = statement
            .query(bounds(window))
            .context("query usage rows")?;
        while let Some(row) = rows.next().context("read usage row")? {
            let document = row.get_ref(0).context("read usage document")?;
            let document = document.as_str().context("read usage document as text")?;
            consume(
                serde_json::from_str(document).context("decode usage document")?,
                row,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
