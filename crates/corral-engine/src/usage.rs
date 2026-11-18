use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result, ensure};
pub use corral_types::{UsageBy, UsageGroup, UsageSummary, UsageTotal};
use jiff::{Timestamp, civil::Date, tz::TimeZone};
use rusqlite::{Row, Transaction, TransactionBehavior, params};

use crate::{Engine, Store, Usage, store::encode};

// Keep identity columns for coverage queries, but do not require a matching run:
// taskless sessions and reports carrying only one identifier still contribute.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS usage(seq INTEGER PRIMARY KEY, task TEXT, run TEXT, document TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS usage_task_run ON usage(task,run);
";

const USAGE_ROWS: &str = "SELECT document FROM usage ORDER BY seq";
const USAGE_RUNS: &str = "FROM usage
    LEFT JOIN runs ON runs.task = usage.task AND runs.id = usage.run
    ORDER BY usage.seq";

/// A stored request with the harness of its exact associated run, if known.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageRow {
    pub usage: Usage,
    pub harness: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageGroupCoverage {
    pub key: Option<String>,
    /// Members of this run cohort with at least one request in the same group.
    pub covered_runs: u64,
    /// Persisted runs grouped by profile, configured model, task or UTC start day.
    pub total_runs: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageReport {
    pub summary: UsageSummary,
    /// The union of request and run keys, in the same order as summary groups.
    /// Empty when ungrouped.
    pub group_coverage: Vec<UsageGroupCoverage>,
}

#[derive(Default)]
struct GroupTally {
    usage: Option<UsageTotal>,
    covered_runs: u64,
    total_runs: u64,
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

fn utc_day(at: Timestamp) -> Date {
    at.to_zoned(TimeZone::UTC).date()
}

fn run_group_column(by: UsageBy) -> &'static str {
    match by {
        UsageBy::Profile => "json_extract(runs.document, '$.profile')",
        UsageBy::Model => "json_extract(runs.document, '$.model')",
        UsageBy::Task => "runs.task",
        UsageBy::Day => "json_extract(runs.document, '$.started')",
    }
}

fn started_day(started: Option<&str>) -> Result<Date> {
    Ok(utc_day(
        started
            .context("run is missing its start timestamp")?
            .parse()
            .context("parse run start timestamp")?,
    ))
}

fn covers_group(
    row: &Row<'_>,
    key: Option<&GroupKey>,
    covered: &mut HashMap<i64, bool>,
) -> Result<bool> {
    let Some(run) = row
        .get::<_, Option<i64>>(1)
        .context("read matched run ID")?
    else {
        return Ok(false);
    };
    let covered_in_group = covered.entry(run).or_default();
    if *covered_in_group {
        return Ok(false);
    }
    let run_key = row.get_ref(2).context("read usage run group")?;
    let run_key = run_key.as_str_or_null().context("read run group as text")?;
    // A request can report a different model or arrive on a later day. It
    // contributes to that request group but does not cover another run cohort.
    let matched = match key {
        Some(GroupKey::Text(key)) => run_key == Some(key.as_str()),
        Some(GroupKey::Day(day)) => started_day(run_key)? == *day,
        None => run_key.is_none(),
    };
    *covered_in_group = matched;
    Ok(matched)
}

fn take_key(by: UsageBy, usage: &mut Usage) -> Option<GroupKey> {
    let label = match by {
        UsageBy::Profile => usage.profile.take(),
        UsageBy::Model => Some(std::mem::take(&mut usage.model)),
        UsageBy::Task => usage.task.take(),
        UsageBy::Day => return Some(GroupKey::Day(utc_day(usage.at))),
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
        Ok(self.roll_up_usage(by, false)?.summary)
    }

    /// Add per-group coverage without changing the totals or overall coverage.
    pub fn usage_report(&self, by: Option<UsageBy>) -> Result<UsageReport> {
        self.roll_up_usage(by, true)
    }

    fn roll_up_usage(&self, by: Option<UsageBy>, include_coverage: bool) -> Result<UsageReport> {
        // Count coverage and read requests from the same snapshot, without taking
        // the write lock used by Store::transaction.
        let transaction = self
            .store
            .connection
            .is_autocommit()
            .then(|| {
                Transaction::new_unchecked(&self.store.connection, TransactionBehavior::Deferred)
            })
            .transpose()
            .context("begin usage summary snapshot")?;
        let mut total = UsageTotal::default();
        let mut groups: BTreeMap<_, GroupTally> = BTreeMap::new();
        let coverage_by = by.filter(|_| include_coverage);
        // Keys are matched run rowids in this snapshot; the flag records whether
        // a request has also covered that run in its own cohort.
        let mut covered = HashMap::new();
        let grouped_coverage = if let Some(by) = coverage_by {
            let total_runs = self.store.tally_run_groups(by, &mut groups)?;
            let column = run_group_column(by);
            let query = format!("SELECT usage.document, runs.rowid, {column} {USAGE_RUNS}");
            Some((query, total_runs))
        } else {
            None
        };
        let query = grouped_coverage
            .as_ref()
            .map_or(USAGE_ROWS, |(query, _)| query.as_str());
        self.store.read_usage(query, |mut usage, row| {
            add(&mut total, &usage)?;
            if let Some(by) = by {
                let key = take_key(by, &mut usage);
                let matched =
                    coverage_by.is_some() && covers_group(row, key.as_ref(), &mut covered)?;
                let group = groups.entry(key).or_default();
                group.covered_runs += u64::from(matched);
                add(group.usage.get_or_insert_with(UsageTotal::default), &usage)?;
            }
            Ok(())
        })?;
        let (total_runs, covered_runs) = match grouped_coverage {
            Some((_, total_runs)) => (total_runs, covered.len() as u64),
            None => self.store.usage_run_counts()?,
        };
        if let Some(transaction) = transaction {
            transaction.commit().context("end usage summary snapshot")?;
        }
        let mut summary = UsageSummary {
            total,
            covered_runs,
            total_runs,
            ..UsageSummary::default()
        };
        let mut group_coverage = Vec::new();
        for (key, tally) in groups {
            let mut key = key.map(GroupKey::into_label);
            if let Some(total) = tally.usage {
                let key = if include_coverage {
                    key.clone()
                } else {
                    key.take()
                };
                summary.groups.push(UsageGroup { key, total });
            }
            if include_coverage {
                group_coverage.push(UsageGroupCoverage {
                    key,
                    covered_runs: tally.covered_runs,
                    total_runs: tally.total_runs,
                });
            }
        }
        Ok(UsageReport {
            summary,
            group_coverage,
        })
    }
}

impl Store {
    /// List recorded requests in insertion order, without resolving their IDs.
    pub fn list_usage(&self) -> Result<Vec<Usage>> {
        let mut usage = Vec::new();
        self.read_usage(USAGE_ROWS, |row, _| {
            usage.push(row);
            Ok(())
        })?;
        Ok(usage)
    }

    /// List requests in insertion order, resolving harness only by both stored IDs.
    pub fn list_usage_rows(&self) -> Result<Vec<UsageRow>> {
        let query =
            format!("SELECT usage.document, json_extract(runs.document, '$.harness') {USAGE_RUNS}");
        let mut usage = Vec::new();
        self.read_usage(&query, |request, row| {
            usage.push(UsageRow {
                usage: request,
                harness: row.get(1).context("read usage run harness")?,
            });
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

    fn tally_run_groups(
        &self,
        by: UsageBy,
        groups: &mut BTreeMap<Option<GroupKey>, GroupTally>,
    ) -> Result<u64> {
        let column = run_group_column(by);
        let mut statement = self
            .connection
            .prepare(&format!("SELECT {column} FROM runs"))
            .context("prepare usage run groups")?;
        let mut rows = statement.query([]).context("query usage run groups")?;
        let mut total_runs = 0;
        while let Some(row) = rows.next().context("read usage run group")? {
            let value = row.get_ref(0).context("read usage run group")?;
            let value = value.as_str_or_null().context("read run group as text")?;
            let key = match by {
                UsageBy::Day => Some(GroupKey::Day(started_day(value)?)),
                _ => value.map(|label| GroupKey::Text(label.to_owned())),
            };
            groups.entry(key).or_default().total_runs += 1;
            total_runs += 1;
        }
        Ok(total_runs)
    }

    fn read_usage(
        &self,
        query: &str,
        mut consume: impl FnMut(Usage, &Row<'_>) -> Result<()>,
    ) -> Result<()> {
        let mut statement = self
            .connection
            .prepare(query)
            .context("prepare usage listing")?;
        let mut rows = statement.query([]).context("query usage rows")?;
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
