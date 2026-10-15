use crate::{Config, CostBasis, Run, Task, Usage, UsageEntry};

impl UsageEntry {
    pub fn total_tokens(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }

    pub fn reprice(&mut self, config: &Config) {
        self.estimated_usd = self.model.as_deref().and_then(|model| {
            let price = config.price(model)?;
            // Token counters are disjoint categories in Pi and Claude transcripts.
            let number = |n| serde_json::Number::from(n).as_f64().unwrap_or_default();
            let cost = (number(self.input) * price.input
                + number(self.output) * price.output
                + number(self.cache_read) * price.cache_read.unwrap_or(price.input)
                + number(self.cache_write) * price.cache_write.unwrap_or(price.input))
                / 1_000_000.0;
            cost.is_finite().then_some(cost)
        });
        self.unpriced_tokens = if self.estimated_usd.is_some() {
            0
        } else {
            self.total_tokens()
        };
    }
}

impl Usage {
    pub fn total_tokens(&self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }

    pub fn from_entries(entries: Vec<UsageEntry>) -> Self {
        let mut usage = Usage::default();
        let mut reported = 0.0;
        let mut reported_count = 0;
        let mut estimated = None;
        for entry in &entries {
            usage.input = usage.input.saturating_add(entry.input);
            usage.output = usage.output.saturating_add(entry.output);
            usage.cache_read = usage.cache_read.saturating_add(entry.cache_read);
            usage.cache_write = usage.cache_write.saturating_add(entry.cache_write);
            usage.unpriced_tokens = usage.unpriced_tokens.saturating_add(entry.unpriced_tokens);
            if let Some(cost) = entry.reported_usd {
                reported += cost;
                reported_count += 1;
            }
            if let Some(cost) = entry.estimated_usd {
                *estimated.get_or_insert(0.0) += cost;
            }
        }
        usage.reported_usd =
            (!entries.is_empty() && reported_count == entries.len()).then_some(reported);
        usage.cost_usd = usage.reported_usd;
        usage.estimated_usd = estimated;
        usage.cost_basis = match (reported_count, entries.len(), estimated) {
            (0, _, None) => None,
            (0, _, Some(_)) => Some(CostBasis::Estimated),
            (count, len, _) if count == len => Some(CostBasis::Reported),
            _ => Some(CostBasis::Mixed),
        };
        usage.entries = entries;
        usage
    }

    /// Recompute with today's explicit price table, retaining transcript evidence.
    pub fn reprice(&mut self, config: &Config, fallback_model: Option<&str>) {
        if self.entries.is_empty() {
            self.entries.push(UsageEntry {
                model: fallback_model.map(str::to_owned),
                input: self.input,
                output: self.output,
                cache_read: self.cache_read,
                cache_write: self.cache_write,
                reported_usd: self.reported_usd.or(self.cost_usd),
                ..UsageEntry::default()
            });
        }
        for entry in &mut self.entries {
            entry.reprice(config);
        }
        *self = Self::from_entries(std::mem::take(&mut self.entries));
    }

    pub fn summary(&self) -> String {
        let dollars =
            |cost: Option<f64>| cost.map_or_else(|| "-".to_string(), |cost| format!("${cost:.4}"));
        let basis = match self.cost_basis {
            Some(CostBasis::Reported) => "reported",
            Some(CostBasis::Estimated) => "estimated",
            Some(CostBasis::Mixed) => "mixed",
            None => "unpriced",
        };
        format!(
            "{} tokens; reported {}; est. {} API-equivalent; basis {basis}; {} unpriced tokens",
            self.total_tokens(),
            dollars(self.reported_usd.or(self.cost_usd)),
            dollars(self.estimated_usd),
            self.unpriced_tokens
        )
    }

    pub fn combine(usages: impl IntoIterator<Item = Self>) -> Option<Self> {
        let mut entries = Vec::new();
        let mut found = false;
        for usage in usages {
            found = true;
            if usage.entries.is_empty() {
                entries.push(UsageEntry {
                    model: None,
                    input: usage.input,
                    output: usage.output,
                    cache_read: usage.cache_read,
                    cache_write: usage.cache_write,
                    reported_usd: usage.reported_usd.or(usage.cost_usd),
                    estimated_usd: usage.estimated_usd,
                    unpriced_tokens: if usage.estimated_usd.is_none() {
                        usage.total_tokens()
                    } else {
                        usage.unpriced_tokens
                    },
                });
            } else {
                entries.extend(usage.entries);
            }
        }
        found.then(|| Self::from_entries(entries))
    }
}

impl Run {
    pub fn total_usage(&self) -> Option<Usage> {
        Usage::combine(
            self.usage
                .clone()
                .into_iter()
                .chain(self.review_runs.iter().filter_map(Run::total_usage)),
        )
    }
}

impl Task {
    pub fn total_usage(&self) -> Option<Usage> {
        Usage::combine(self.runs.iter().filter_map(Run::total_usage))
    }
}

#[cfg(test)]
#[path = "usage_tests.rs"]
mod tests;
