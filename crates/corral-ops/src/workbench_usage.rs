//! Bounded facets from existing collected records; this read never rewrites accounting.
use std::collections::BTreeMap;

use corral_core::{Run, Task, Usage};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct UsagePanel {
    pub source: &'static str,
    pub rows: Vec<UsageRow>,
    pub models_omitted: usize,
}
#[derive(Debug, Serialize)]
pub struct UsageRow {
    pub scope: String,
    pub usage: Option<Usage>,
    pub wall_seconds: Option<i64>,
    pub known_runs: usize,
    pub runs: usize,
}
fn row(scope: String, runs: &[&Run]) -> UsageRow {
    let mut usage = Usage::combine(runs.iter().filter_map(|r| r.usage.clone()));
    // Aggregate rows do not need an unbounded per-response history.
    if let Some(u) = &mut usage {
        u.entries.clear();
    }
    UsageRow {
        scope,
        usage,
        wall_seconds: if runs.len() == 1 {
            runs[0]
                .ended
                .map(|end| end.as_second() - runs[0].started.as_second())
        } else {
            None
        },
        known_runs: runs.iter().filter(|r| r.usage.is_some()).count(),
        runs: runs.len(),
    }
}
pub fn panel(task: &Task, run: &Run) -> UsagePanel {
    let runs: Vec<_> = task.runs.iter().collect();
    let mut rows = vec![
        row(format!("Task {} workers", task.id), &runs),
        row(format!("Run {} worker", run.id), &[run]),
    ];
    let mut models: BTreeMap<String, Vec<corral_core::UsageEntry>> = BTreeMap::new();
    if let Some(usage) = &run.usage {
        for entry in &usage.entries {
            models
                .entry(
                    entry
                        .model
                        .clone()
                        .unwrap_or_else(|| "unknown model".into()),
                )
                .or_default()
                .push(entry.clone());
        }
    }
    let models_omitted = models.len().saturating_sub(100);
    for (model, entries) in models.into_iter().take(100) {
        // Keep timing/provenance as bounded text facets rather than all response records.
        let timed: Vec<_> = entries.iter().filter_map(|e| e.latency_ms).collect();
        let sources: std::collections::BTreeSet<_> = entries
            .iter()
            .filter_map(|e| e.price_source.clone())
            .collect();
        let scope = format!(
            "Model {model} · {}/{} response latencies known · mean {} ms · prices {}",
            timed.len(),
            entries.len(),
            if timed.is_empty() {
                "unavailable".into()
            } else {
                format!(
                    "{:.0}",
                    timed.iter().map(|v| *v as f64).sum::<f64>() / timed.len() as f64
                )
            },
            if sources.is_empty() {
                "unknown".into()
            } else {
                sources.into_iter().collect::<Vec<_>>().join(", ")
            }
        );
        let mut usage = Usage::from_entries(entries);
        usage.entries.clear();
        rows.push(UsageRow {
            scope,
            usage: Some(usage),
            wall_seconds: None,
            known_runs: 1,
            runs: 1,
        });
    }
    UsagePanel {
        source: "Stored collector snapshot; may lag active responses. Worker-only task/run totals; reviews excluded. Model rows cover selected worker run. Missing runs are not zero.",
        rows,
        models_omitted,
    }
}
