//! Known subtotals retain missing contributions and scan coverage through aggregation.

use corral_protocol::board::CostBasis;
use corral_protocol::detail::UsageCompleteness;

use super::completeness::{complete, completeness_label, merge};
use super::money;

#[derive(Debug, Clone, Default)]
pub(super) struct Sum {
    pub total: f64,
    pub known: bool,
    estimated: bool,
    missing: u64,
    coverage: Option<UsageCompleteness>,
}

impl Sum {
    pub fn add(&mut self, value: Option<f64>, basis: Option<CostBasis>) {
        if let Some(value) = value {
            self.total += value;
            self.known = true;
            self.estimated |= basis != Some(CostBasis::Reported);
        } else {
            self.missing += 1;
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
        self.missing += other.missing;
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

    pub fn figure(&self) -> String {
        if !self.known {
            "unknown".into()
        } else if self.estimated {
            format!("~{}", money(self.total))
        } else {
            money(self.total)
        }
    }

    pub fn note(&self) -> Option<String> {
        self.qualifier(true)
    }

    pub fn contribution_note(&self) -> Option<String> {
        self.qualifier(false)
    }

    fn qualifier(&self, scan: bool) -> Option<String> {
        let partial = self.coverage.as_ref().is_some_and(|coverage| {
            coverage.partial || coverage.unpriced_entries > 0 || coverage.undated_entries > 0
        });
        let mut parts = Vec::new();
        if self.known && (self.missing > 0 || partial) {
            parts.push("known subtotal".into());
        }
        if scan {
            parts.extend(self.coverage.as_ref().and_then(completeness_label));
        } else if partial {
            parts.push("partial".into());
            if let Some(coverage) = &self.coverage {
                if coverage.unpriced_entries > 0 {
                    parts.push(format!("{} unpriced", coverage.unpriced_entries));
                }
                if coverage.undated_entries > 0 {
                    parts.push(format!("{} undated", coverage.undated_entries));
                }
            }
        }
        if self.missing > 0 {
            parts.push(format!("{} unknown totals", self.missing));
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    pub fn label(&self) -> String {
        let figure = self.figure();
        match self.note() {
            Some(note) => format!("{figure} · {note}"),
            None => figure,
        }
    }
}
