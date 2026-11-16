//! Known subtotals retain scan coverage through aggregation.

use corral_protocol::board::CostBasis;
use corral_protocol::detail::UsageCompleteness;

use super::completeness::{complete, merge};
use super::{UNKNOWN, money};

#[derive(Debug, Clone, Default)]
pub(super) struct Sum {
    pub total: f64,
    pub known: bool,
    estimated: bool,
    pub coverage: Option<UsageCompleteness>,
}

impl Sum {
    pub fn add(&mut self, value: Option<f64>, basis: Option<CostBasis>) {
        if let Some(value) = value {
            self.total += value;
            self.known = true;
            self.estimated |= basis != Some(CostBasis::Reported);
        }
    }

    pub fn add_total(
        &mut self,
        value: Option<f64>,
        basis: Option<CostBasis>,
        coverage: &UsageCompleteness,
    ) {
        self.add(value, basis);
        merge(self.coverage.get_or_insert_with(complete), coverage);
    }

    pub fn add_sum(&mut self, other: &Self) {
        self.total += other.total;
        self.known |= other.known;
        self.estimated |= other.estimated;
        if let Some(coverage) = &other.coverage {
            merge(self.coverage.get_or_insert_with(complete), coverage);
        }
    }

    /// Series-level coverage overlaps the per-key coverage; don't count those entries twice.
    pub fn qualify(&mut self, coverage: &UsageCompleteness) {
        let into = self.coverage.get_or_insert_with(complete);
        let unpriced = into.unpriced_entries;
        merge(into, coverage);
        into.unpriced_entries = unpriced;
    }

    /// The total in dollars, `~` when any part is estimated, [`UNKNOWN`] when nothing is known.
    pub fn figure(&self) -> String {
        if !self.known {
            UNKNOWN.into()
        } else if self.estimated {
            format!("~{}", money(self.total))
        } else {
            money(self.total)
        }
    }
}
