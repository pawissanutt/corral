use anyhow::{Context, Result};
use corral_core::Store;
use jiff::{SignedDuration, Timestamp};

use super::Check;

#[cfg(test)]
#[path = "pricing_tests.rs"]
mod tests;

pub(super) fn check(store: &Store, now: Timestamp) -> Result<Check> {
    let since = now
        .checked_sub(SignedDuration::from_secs(30 * 86400))
        .context("computing pricing coverage cutoff")?;
    let report = corral_ops::pricing_coverage(store, Some(since))?;
    Ok(Check {
        id: "pricing_coverage",
        ok: report.unpriced_tokens == 0,
        hard: false,
        message: format!(
            "{} unpriced tokens in runs started in the last 30 days; catalog {}; inspect with `corral pricing check`.",
            report.unpriced_tokens, report.catalog_date
        ),
        fix: None,
    })
}
