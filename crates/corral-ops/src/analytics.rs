//! Pure, versioned analytics over refreshed task and event evidence.
//!
//! Cohorts select dispatches inclusively by start time; reviewer sessions remain
//! children of the dispatch in every grouping. This module performs no I/O.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use corral_core::{Event, Run, Task, Usage};
use jiff::Timestamp;
use serde::Serialize;

use crate::{StatsBy, StatsRow};

pub const ANALYTICS_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize)]
pub struct Analytics {
    pub schema_version: u32,
    pub window: Option<Timestamp>,
    pub snapshot_at: Timestamp,
    pub groups: BTreeMap<StatsBy, Vec<StatsRow>>,
}

/// Collect all groupings using the same accumulator as `stats`, without refreshing
/// transcripts, reading the clock, or changing the supplied evidence.
pub fn collect_analytics(
    tasks: &[Task],
    events: &[Event],
    window: Option<Timestamp>,
    now: Timestamp,
) -> Result<Analytics> {
    let mut groups = BTreeMap::new();
    for by in [
        StatsBy::Task,
        StatsBy::Project,
        StatsBy::Kind,
        StatsBy::Size,
        StatsBy::Model,
        StatsBy::Mode,
        StatsBy::Profile,
        StatsBy::Day,
    ] {
        groups.insert(
            by,
            crate::stats::collect_stats(tasks, events, by, window, now)?,
        );
    }
    Ok(Analytics {
        schema_version: ANALYTICS_SCHEMA_VERSION,
        window,
        snapshot_at: now,
        groups,
    })
}

#[derive(Debug, Default, Serialize)]
pub struct TokenFacets {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

#[derive(Debug, Serialize)]
pub struct PriceCoverage {
    pub priced_tokens: u64,
    pub known_tokens: u64,
    /// Undefined when the known token total is zero, even with known zero usage.
    pub ratio: Option<f64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Measurement {
    Measured,
    Partial,
    NotMeasured,
}

#[derive(Debug, Serialize)]
pub struct RoleFacets {
    pub measurement: Measurement,
    pub sessions: u32,
    pub sessions_without_usage: u32,
    pub estimated_usd: Option<f64>,
    pub reported_usd: Option<f64>,
    pub tokens: Option<TokenFacets>,
    pub price_coverage: Option<PriceCoverage>,
    pub cache_hit_ratio: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct RoleSplit {
    pub implementation: RoleFacets,
    pub review: RoleFacets,
    /// PlanRun and PlanReview do not currently store usage evidence.
    pub plan: Measurement,
}

#[derive(Debug, Serialize)]
pub struct AnalyticsFacets {
    pub schema_version: u32,
    pub estimated_usd: Option<f64>,
    pub reported_usd: Option<f64>,
    pub price_coverage: Option<PriceCoverage>,
    pub runs_without_usage: u32,
    pub tokens: Option<TokenFacets>,
    pub cache_hit_ratio: Option<f64>,
    pub roles: RoleSplit,
    pub landed_tasks: usize,
    pub estimated_usd_per_landed_task: Option<f64>,
    pub rework_estimated_usd: Option<f64>,
    /// No change request implies known zero rework; otherwise response timing
    /// is required. Run wall time must never stand in for response timing.
    pub rework_share: Option<f64>,
}

#[derive(Default)]
struct UsageTotals {
    sessions: u32,
    missing: u32,
    tokens: Option<TokenFacets>,
    estimated: Option<f64>,
    reported: Option<f64>,
    unpriced: u64,
}

fn number(n: u64) -> f64 {
    serde_json::Number::from(n).as_f64().unwrap_or_default()
}

impl UsageTotals {
    fn add(&mut self, usage: Option<&Usage>) {
        self.sessions += 1;
        let Some(usage) = usage else {
            self.missing += 1;
            return;
        };
        let tokens = self.tokens.get_or_insert_with(TokenFacets::default);
        tokens.input = tokens.input.saturating_add(usage.input);
        tokens.output = tokens.output.saturating_add(usage.output);
        tokens.cache_read = tokens.cache_read.saturating_add(usage.cache_read);
        tokens.cache_write = tokens.cache_write.saturating_add(usage.cache_write);
        self.unpriced = self.unpriced.saturating_add(usage.unpriced_tokens);
        if let Some(cost) = usage.estimated_usd {
            *self.estimated.get_or_insert(0.0) += cost;
        }
        if let Some(cost) = usage.reported_usd.or(usage.cost_usd) {
            *self.reported.get_or_insert(0.0) += cost;
        }
    }

    fn finish(self) -> RoleFacets {
        let price_coverage = self.tokens.as_ref().map(|t| {
            let known = t
                .input
                .saturating_add(t.output)
                .saturating_add(t.cache_read)
                .saturating_add(t.cache_write);
            let priced = known.saturating_sub(self.unpriced);
            PriceCoverage {
                priced_tokens: priced,
                known_tokens: known,
                ratio: (known > 0).then(|| number(priced) / number(known)),
            }
        });
        let cache_hit_ratio = self.tokens.as_ref().and_then(|t| {
            let input = t
                .input
                .saturating_add(t.cache_read)
                .saturating_add(t.cache_write);
            (input > 0).then(|| number(t.cache_read) / number(input))
        });
        RoleFacets {
            measurement: match (self.tokens.is_some(), self.missing) {
                (false, _) => Measurement::NotMeasured,
                (true, 0) => Measurement::Measured,
                (true, _) => Measurement::Partial,
            },
            sessions: self.sessions,
            sessions_without_usage: self.missing,
            estimated_usd: self.estimated,
            reported_usd: self.reported,
            tokens: self.tokens,
            price_coverage,
            cache_hit_ratio,
        }
    }
}

#[derive(Default)]
pub(crate) struct FacetTotals {
    usage: UsageTotals,
    implementation: UsageTotals,
    review: UsageTotals,
    landed: BTreeSet<String>,
    rework: f64,
    rework_unavailable: bool,
}

impl FacetTotals {
    pub(crate) fn add(
        &mut self,
        task: &Task,
        run: &Run,
        events: &[&Event],
        first_change: Option<Timestamp>,
    ) {
        let usage = run.total_usage();
        self.usage.add(usage.as_ref());
        self.implementation
            .add(Usage::combine(run.usage.clone()).as_ref());
        let missing_reviews = self.review.missing;
        self.add_reviewers(&run.review_runs);
        if events
            .iter()
            .any(|e| e.kind == "approved" && e.detail["strategy"] != "keep_branch")
        {
            self.landed.insert(task.id.clone());
        }
        if let Some(change) = first_change {
            self.rework_unavailable |= run.usage.is_none() || self.review.missing > missing_reviews;
            match usage {
                Some(usage) => {
                    for entry in &usage.entries {
                        if let Some(cost) = entry.estimated_usd.filter(|c| *c > 0.0) {
                            match entry.at {
                                Some(at) if at > change => self.rework += cost,
                                Some(_) => {}
                                None => self.rework_unavailable = true,
                            }
                        }
                    }
                }
                None => self.rework_unavailable = true,
            }
        }
    }

    fn add_reviewers(&mut self, runs: &[Run]) {
        for run in runs {
            self.review.add(Usage::combine(run.usage.clone()).as_ref());
            self.add_reviewers(&run.review_runs);
        }
    }

    pub(crate) fn finish(self) -> AnalyticsFacets {
        let usage = self.usage.finish();
        let landed = self.landed.len();
        let rework_estimated_usd = usage
            .estimated_usd
            .filter(|_| !self.rework_unavailable)
            .map(|_| self.rework);
        AnalyticsFacets {
            schema_version: ANALYTICS_SCHEMA_VERSION,
            estimated_usd: usage.estimated_usd,
            reported_usd: usage.reported_usd,
            estimated_usd_per_landed_task: usage
                .estimated_usd
                .filter(|_| landed > 0)
                .map(|cost| cost / number(u64::try_from(landed).unwrap_or(u64::MAX))),
            rework_share: rework_estimated_usd
                .zip(usage.estimated_usd)
                .filter(|(_, cost)| *cost > 0.0)
                .map(|(rework, cost)| rework / cost),
            rework_estimated_usd,
            price_coverage: usage.price_coverage,
            runs_without_usage: usage.sessions_without_usage,
            tokens: usage.tokens,
            cache_hit_ratio: usage.cache_hit_ratio,
            roles: RoleSplit {
                implementation: self.implementation.finish(),
                review: self.review.finish(),
                plan: Measurement::NotMeasured,
            },
            landed_tasks: landed,
        }
    }
}
