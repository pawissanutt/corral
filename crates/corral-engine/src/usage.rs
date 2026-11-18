use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
pub use corral_types::{UsageBy, UsageGroup, UsageSummary, UsageTotal};
use jiff::{civil::Date, tz::TimeZone};
use rusqlite::{Transaction, TransactionBehavior, params};

use crate::{Engine, Store, Usage, store::encode};

// Keep identity columns for coverage queries, but do not require a matching run:
// taskless sessions and reports carrying only one identifier still contribute.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS usage(seq INTEGER PRIMARY KEY, task TEXT, run TEXT, document TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS usage_task_run ON usage(task,run);
";

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

fn take_key(by: UsageBy, usage: &mut Usage) -> Option<GroupKey> {
    let label = match by {
        UsageBy::Profile => usage.profile.take(),
        UsageBy::Model => Some(std::mem::take(&mut usage.model)),
        UsageBy::Task => usage.task.take(),
        UsageBy::Day => return Some(GroupKey::Day(usage.at.to_zoned(TimeZone::UTC).date())),
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
        // Count coverage and read requests from the same snapshot, without taking
        // the write lock used by Store::transaction.
        let transaction =
            Transaction::new_unchecked(&self.store.connection, TransactionBehavior::Deferred)
                .context("begin usage summary snapshot")?;
        let (total_runs, covered_runs) = transaction
            .query_row(
                "SELECT count(*), count(*) FILTER (WHERE EXISTS (
                     SELECT 1 FROM usage WHERE usage.task = runs.task AND usage.run = runs.id
                 )) FROM runs",
                [],
                |row| Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)? as u64)),
            )
            .context("count usage run coverage")?;
        let mut total = UsageTotal::default();
        let mut groups: BTreeMap<_, UsageTotal> = BTreeMap::new();
        self.store.read_usage(|mut usage| {
            add(&mut total, &usage)?;
            if let Some(by) = by {
                let key = take_key(by, &mut usage);
                add(groups.entry(key).or_default(), &usage)?;
            }
            Ok(())
        })?;
        transaction.commit().context("end usage summary snapshot")?;
        Ok(UsageSummary {
            total,
            groups: groups
                .into_iter()
                .map(|(key, total)| UsageGroup {
                    key: key.map(GroupKey::into_label),
                    total,
                })
                .collect(),
            covered_runs,
            total_runs,
        })
    }
}

impl Store {
    /// List recorded requests in insertion order, without resolving their IDs.
    pub fn list_usage(&self) -> Result<Vec<Usage>> {
        let mut usage = Vec::new();
        self.read_usage(|row| {
            usage.push(row);
            Ok(())
        })?;
        Ok(usage)
    }

    fn read_usage(&self, mut consume: impl FnMut(Usage) -> Result<()>) -> Result<()> {
        let mut statement = self
            .connection
            .prepare("SELECT document FROM usage ORDER BY seq")
            .context("prepare usage listing")?;
        let mut rows = statement.query([]).context("query usage rows")?;
        while let Some(row) = rows.next().context("read usage row")? {
            let document = row.get_ref(0).context("read usage document")?;
            let document = document.as_str().context("read usage document as text")?;
            consume(serde_json::from_str(document).context("decode usage document")?)?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
