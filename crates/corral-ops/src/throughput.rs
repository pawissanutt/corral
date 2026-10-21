//! End-to-end output rate per response: output tokens divided by response latency.
//!
//! The rate is *not* decode speed. Latency runs from the triggering transcript entry to the
//! response, so it includes time to first token and provider queueing, and output tokens include
//! reasoning tokens. Entries without usable timing are excluded and counted, never guessed.
//! Each response is rated under its own evidenced model plus the run's effort and harness, so a
//! mid-run model switch never contaminates another model's rate.
//! The collection is pure: it performs no I/O and reads no clock.
use std::collections::BTreeMap;

use anyhow::Result;
use corral_core::{Store, Task, Usage, UsageEntry};
use jiff::Timestamp;
use serde::Serialize;

/// The data-quality statement that must accompany every published rate.
pub const RATE_NOTE: &str = "End-to-end output rate = output tokens / response latency. Latency includes time to \
first token and provider queueing, and output tokens include reasoning tokens, so it is lower \
than decode speed.";

type ModeKey = (Option<String>, Option<String>, Option<String>);

#[derive(Debug, Default, PartialEq, Serialize)]
pub struct Throughput {
    /// Responses with usable timing that contribute a rate.
    pub samples: u32,
    /// Known usage entries excluded for missing or zero latency, including legacy aggregates.
    pub excluded: u32,
    pub p50_tokens_per_second: Option<f64>,
    pub p90_tokens_per_second: Option<f64>,
    /// Summed latency of the timed samples.
    pub active_seconds: f64,
    /// Timed samples that also carry an estimated price.
    pub priced_samples: u32,
    /// Estimated API-equivalent USD over active time, using priced samples only.
    pub estimated_usd_per_active_hour: Option<f64>,
}

/// One mode: the response model with the run's effort and harness. `group` is the JSON-encoded
/// `[model, effort, harness]` tuple, identical to the `StatsBy::Mode` key.
#[derive(Debug, Serialize)]
pub struct ModeThroughput {
    pub group: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub harness: Option<String>,
    pub throughput: Throughput,
}

#[derive(Default)]
pub(crate) struct ThroughputTotals {
    rates: Vec<f64>,
    excluded: u32,
    active_ms: u64,
    priced: u32,
    priced_ms: u64,
    estimated: f64,
}

impl ThroughputTotals {
    pub(crate) fn add(&mut self, entry: &UsageEntry) {
        let Some(ms) = entry.latency_ms.filter(|ms| *ms > 0) else {
            self.excluded = self.excluded.saturating_add(1);
            return;
        };
        let seconds = crate::analytics::number(ms) / 1000.0;
        self.rates
            .push(crate::analytics::number(entry.output) / seconds);
        self.active_ms = self.active_ms.saturating_add(ms);
        if let Some(cost) = entry.estimated_usd {
            self.priced = self.priced.saturating_add(1);
            self.priced_ms = self.priced_ms.saturating_add(ms);
            self.estimated += cost;
        }
    }

    pub(crate) fn finish(mut self) -> Throughput {
        self.rates.sort_by(f64::total_cmp);
        let hours = crate::analytics::number(self.priced_ms) / 3_600_000.0;
        Throughput {
            samples: u32::try_from(self.rates.len()).unwrap_or(u32::MAX),
            excluded: self.excluded,
            p50_tokens_per_second: percentile(&self.rates, 50),
            p90_tokens_per_second: percentile(&self.rates, 90),
            active_seconds: crate::analytics::number(self.active_ms) / 1000.0,
            priced_samples: self.priced,
            estimated_usd_per_active_hour: (self.priced_ms > 0).then(|| self.estimated / hours),
        }
    }
}

/// Pure grouping of implementation responses by mode. Run selection matches `collect_stats`:
/// dispatches after `now` or before the inclusive `window` cutoff are skipped. Reviewer sessions
/// are not rated. A response without model evidence falls back to its run's model.
pub(crate) fn collect_mode_throughput(
    tasks: &[Task],
    window: Option<Timestamp>,
    now: Timestamp,
) -> Result<Vec<ModeThroughput>> {
    let mut groups: BTreeMap<String, (ModeKey, ThroughputTotals)> = BTreeMap::new();
    for run in tasks.iter().flat_map(|task| &task.runs) {
        if run.started > now || window.is_some_and(|cutoff| run.started < cutoff) {
            continue;
        }
        let Some(usage) = Usage::combine(run.usage.clone()) else {
            continue;
        };
        for entry in &usage.entries {
            let key = (
                entry.model.clone().or_else(|| run.model.clone()),
                run.effort.clone(),
                run.harness.clone(),
            );
            let id = serde_json::to_string(&key)?;
            groups
                .entry(id)
                .or_insert_with(|| (key, ThroughputTotals::default()))
                .1
                .add(entry);
        }
    }
    Ok(groups
        .into_iter()
        .map(
            |(group, ((model, effort, harness), totals))| ModeThroughput {
                group,
                model,
                effort,
                harness,
                throughput: totals.finish(),
            },
        )
        .collect())
}

/// Refresh usage timing, then rate every mode in the dispatch cohort selected by `since`.
pub fn mode_throughput(store: &Store, since: Option<Timestamp>) -> Result<Vec<ModeThroughput>> {
    crate::usage::collect_missing(store, since)?;
    collect_mode_throughput(&store.list()?, since, Timestamp::now())
}

/// Linear-interpolation percentile (the "R-7" definition) of an ascending slice, `pct` in 0..=100.
/// Integer rank arithmetic avoids float-to-index casts; `None` for an empty slice.
pub fn percentile(sorted: &[f64], pct: u64) -> Option<f64> {
    let last = sorted.len().checked_sub(1)?;
    let rank = pct.min(100) * u64::try_from(last).ok()?;
    let low = usize::try_from(rank / 100).ok()?;
    let fraction = crate::analytics::number(rank % 100) / 100.0;
    let lower = *sorted.get(low)?;
    let upper = sorted.get(low + 1).copied().unwrap_or(lower);
    Some(lower + (upper - lower) * fraction)
}

/// Human label for a mode group key (`["model","effort","harness"]` JSON, nulls allowed).
/// Unrecognised keys are returned unchanged.
pub fn mode_label(group: &str) -> String {
    match serde_json::from_str::<[Option<String>; 3]>(group) {
        Ok(parts) => parts
            .map(|part| part.unwrap_or_else(|| "-".into()))
            .join(" / "),
        Err(_) => group.to_owned(),
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub fn fixed(value: Option<f64>, places: usize) -> String {
    value.map_or_else(|| "-".into(), |v| format!("{v:.places$}"))
}

/// Self-contained, network-free HTML fragment for the report's throughput section.
/// Output is deterministic for identical input.
pub fn throughput_section_html(rows: &[ModeThroughput]) -> String {
    let mut html = String::from("<section id=\"throughput\">\n<h2>Throughput by mode</h2>\n");
    html.push_str(
        "<table>\n<thead><tr><th>Mode (model / effort / harness)</th><th>Samples</th>\
<th>Excluded</th><th>p50 tok/s</th><th>p90 tok/s</th><th>est. USD / active hour</th></tr></thead>\n<tbody>\n",
    );
    let mut samples = 0u32;
    let mut excluded = 0u32;
    for row in rows {
        let t = &row.throughput;
        samples = samples.saturating_add(t.samples);
        excluded = excluded.saturating_add(t.excluded);
        html.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>\n",
            escape(&mode_label(&row.group)),
            t.samples,
            t.excluded,
            fixed(t.p50_tokens_per_second, 1),
            fixed(t.p90_tokens_per_second, 1),
            fixed(t.estimated_usd_per_active_hour, 2),
        ));
    }
    html.push_str("</tbody>\n</table>\n<h3>Data quality</h3>\n<ul>\n");
    html.push_str(&format!(
        "<li data-note=\"rate-definition\">{} Rates cover {samples} timed responses.</li>\n",
        escape(RATE_NOTE)
    ));
    html.push_str(&format!(
        "<li data-note=\"excluded\" data-count=\"{excluded}\">Usage entries without timing, excluded from these rates: {excluded}.</li>\n"
    ));
    html.push_str(
        "<li data-note=\"scope\">Dollars are est. API-equivalent over priced samples only; implementation \
responses only, reviewer sessions are not included.</li>\n</ul>\n</section>\n",
    );
    html
}

const PAGE_STYLE: &str = "body{font:15px/1.5 system-ui,sans-serif;margin:0 auto;max-width:60rem;padding:1.5rem;\
color:#1b1f24;background:#fff}section{overflow-x:auto}table{border-collapse:collapse;width:100%}\
th,td{padding:.4rem .7rem;border-bottom:1px solid #d0d7de;text-align:right;white-space:nowrap}\
th:first-child,td:first-child{text-align:left}th{font-weight:600}li{margin:.3rem 0}\
@media(prefers-color-scheme:dark){body{color:#e6edf3;background:#0d1117}\
th,td{border-color:#30363d}}";

/// Standalone, network-free HTML page around the throughput section.
pub fn throughput_report_html(rows: &[ModeThroughput]) -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<title>corral throughput</title><style>{PAGE_STYLE}</style></head><body>\n\
<h1>corral throughput</h1>\n{}</body></html>\n",
        throughput_section_html(rows)
    )
}

#[cfg(test)]
#[path = "throughput_tests.rs"]
mod tests;
