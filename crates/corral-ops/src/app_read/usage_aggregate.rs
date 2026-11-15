//! Bounded UTC-day rollups retain attribution without retaining response rows.
use std::collections::BTreeMap;

use corral_core::UsageEntry;
use corral_protocol::board::CostBasis;
use corral_protocol::detail::UsageCompleteness;
use jiff::Timestamp;

use super::{DAY, budget::Budget};

pub(super) type Key = (String, Option<String>);
pub(super) type Groups = BTreeMap<Key, BTreeMap<Option<i64>, Total>>;

pub(super) struct Aggregate {
    pub(super) groups: Groups,
    pub(super) budget: Budget,
    pub(super) future: bool,
}

impl Aggregate {
    pub(super) fn new(max: usize) -> Self {
        Self {
            groups: BTreeMap::new(),
            budget: Budget::new(max),
            future: false,
        }
    }

    pub(super) fn add(
        &mut self,
        connection: String,
        profile: Option<String>,
        mut entry: UsageEntry,
        cutoff: Timestamp,
        now: Timestamp,
    ) {
        if let Some(at) = entry.at {
            self.budget.observe(at);
            if at > now {
                self.future = true;
                return;
            }
            if at < cutoff {
                return;
            }
        } else {
            self.budget.coverage.undated_entries += 1;
        }
        let day = entry.at.map(|at| at.as_second().div_euclid(DAY));
        let profile = entry.profile.take().or(profile);
        let total = self
            .groups
            .entry((connection, profile))
            .or_default()
            .entry(day)
            .or_default();
        if total.add(&entry) {
            self.budget.coverage.unpriced_entries += 1;
        }
    }

    pub(super) fn merge(&mut self, other: &Self) {
        self.future |= other.future;
        let coverage = &other.budget.coverage;
        if coverage.partial {
            self.budget
                .partial(coverage.reason.as_deref().unwrap_or("partial_source"));
        }
        for at in [other.budget.first, other.budget.last]
            .into_iter()
            .flatten()
        {
            self.budget.observe(at);
        }
        self.budget.coverage.unpriced_entries += coverage.unpriced_entries;
        self.budget.coverage.undated_entries += coverage.undated_entries;
        for (key, days) in &other.groups {
            let group = self.groups.entry(key.clone()).or_default();
            for (day, total) in days {
                group.entry(*day).or_default().merge(total);
            }
        }
    }
}

fn valid_cost(entry: &UsageEntry) -> Option<(f64, bool)> {
    entry
        .reported_usd
        .filter(|cost| cost.is_finite() && *cost >= 0.0)
        .map(|cost| (cost, false))
        .or_else(|| {
            entry
                .estimated_usd
                .filter(|cost| cost.is_finite() && *cost >= 0.0)
                .map(|cost| (cost, true))
        })
}

#[derive(Default)]
pub(super) struct Total {
    usd: f64,
    priced: u64,
    unpriced: u64,
    count: u64,
    pub(super) tokens: u64,
    reported: bool,
    estimated: bool,
}

impl Total {
    pub(super) fn add(&mut self, entry: &UsageEntry) -> bool {
        self.count += 1;
        self.tokens = self.tokens.saturating_add(entry.total_tokens());
        let cost = valid_cost(entry);
        if let Some((cost, estimated)) = cost {
            self.usd += cost;
            self.priced += 1;
            self.estimated |= estimated;
            self.reported |= !estimated;
            if entry.unpriced_tokens > 0 {
                self.unpriced += 1;
            }
        } else {
            self.unpriced += 1;
        }
        entry.unpriced_tokens > 0 || cost.is_none()
    }

    pub(super) fn merge(&mut self, other: &Self) {
        self.count += other.count;
        self.usd += other.usd;
        self.priced += other.priced;
        self.unpriced += other.unpriced;
        self.tokens = self.tokens.saturating_add(other.tokens);
        self.reported |= other.reported;
        self.estimated |= other.estimated;
    }

    pub(super) fn cost(&self) -> Option<f64> {
        (self.priced > 0 && self.unpriced == 0 && self.usd.is_finite()).then_some(self.usd)
    }

    pub(super) fn basis(&self) -> Option<CostBasis> {
        self.cost()?;
        match (self.reported, self.estimated) {
            (true, true) => Some(CostBasis::Mixed),
            (true, false) => Some(CostBasis::Reported),
            (false, true) => Some(CostBasis::Estimated),
            (false, false) => None,
        }
    }

    pub(super) fn completeness(&self, scan: &UsageCompleteness, undated: u64) -> UsageCompleteness {
        let mut result = scan.clone();
        result.unpriced_entries = self.unpriced;
        result.undated_entries = undated;
        if self.unpriced > 0 || undated > 0 {
            result.partial = true;
            if result.reason.is_none() {
                result.reason = Some(
                    if undated > 0 {
                        "undated_usage"
                    } else {
                        "unpriced_usage"
                    }
                    .into(),
                );
            }
        }
        result
    }

    pub(super) fn entries(&self) -> u64 {
        self.count
    }
}
