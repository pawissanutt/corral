//! The period at a glance: four tiles (tokens, the estimate, the reported cost, the runs) and
//! what the totals leave out, in one line and, behind Details, in full.

use corral_types::UsageSummary;

use super::{
    LEGEND, cost, coverage, covers, estimate, estimate_known, figure_of, input_evidence,
    output_evidence, token_reasons, unpriced, used_words,
};
use crate::model::home::Tile;
use crate::model::usage::{ALL_COVERED, Gaps, GroupRow, count, plural};

/// `words` with its first letter in capitals.
fn capital(words: &str) -> String {
    let mut chars = words.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn tile(key: &'static str, label: &str, value: String, sub: String, hint: String) -> Tile {
    Tile {
        key,
        label: label.to_owned(),
        value,
        sub,
        hint,
        you: false,
        go: None,
        meter: None,
        spark: Vec::new(),
    }
}

/// Tokens in and out, the API-equivalent estimate, the reported cost and the runs.
pub(super) fn stats(summary: &UsageSummary) -> Vec<Tile> {
    let total = &summary.total;
    vec![
        tile(
            "usage-tokens",
            "Tokens",
            used_words(total),
            format!(
                "{} in · {} out",
                figure_of(input_evidence(total), total.input),
                figure_of(output_evidence(total), total.output)
            ),
            "Input and output tokens the agent hosts reported in this period.".to_owned(),
        ),
        tile(
            "usage-est",
            "Est. cost",
            estimate(total),
            "API-equivalent".to_owned(),
            LEGEND.to_owned(),
        ),
        tile(
            "usage-reported",
            "Reported cost",
            cost(total),
            format!(
                "{} of {} requests",
                count(total.priced_requests),
                count(total.requests)
            ),
            "The price agent hosts recorded with corral usage add.".to_owned(),
        ),
        tile(
            "usage-runs",
            "Runs",
            count(summary.total_runs),
            plural(total.requests, "request", "requests"),
            format!("{}.", capital(&covers(summary))),
        ),
    ]
}

/// What the period's totals leave out: one line of the parts that apply, every reason in
/// full, each partial row's reasons, and the notes on where the figures come from.
pub(super) fn gaps(
    summary: &UsageSummary,
    rows: &[GroupRow],
    open: bool,
    notes: Vec<String>,
) -> Gaps {
    let total = &summary.total;
    let mut parts = Vec::new();
    if unpriced(total) > 0 {
        parts.push(format!(
            "cost reported for {} of {} requests",
            count(total.priced_requests),
            count(total.requests)
        ));
    }
    if !estimate_known(total) {
        parts.push("estimate coverage unknown".to_owned());
    } else if total.estimated_requests < total.requests {
        parts.push(format!(
            "estimate covers {} of {}",
            count(total.estimated_requests),
            count(total.requests)
        ));
    }
    if !token_reasons(total).is_empty() {
        parts.push("token counts incomplete".to_owned());
    }
    if summary.covered_runs < summary.total_runs {
        parts.push(format!(
            "{} of {} runs reported no usage on their start day",
            count(summary.total_runs - summary.covered_runs),
            count(summary.total_runs)
        ));
    }
    let partial = !parts.is_empty();
    let line = if partial {
        capital(&parts.join(" · "))
    } else {
        ALL_COVERED.to_owned()
    };
    let mut reasons = vec![format!("{}.", capital(&covers(summary)))];
    reasons.extend(coverage(summary).map(|c| c.reasons).unwrap_or_default());
    let groups = rows
        .iter()
        .filter_map(|row| row.reasons.clone().map(|why| (row.key.clone(), why)))
        .collect();
    Gaps {
        line,
        partial,
        open,
        reasons,
        groups,
        notes,
    }
}
