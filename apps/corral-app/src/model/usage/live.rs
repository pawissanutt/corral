//! corral's recorded usage over the chosen period as the Usage page. Run coverage counts
//! only requests on each run's start day; costs also include requests on later days. The
//! period's figures as tiles, what they leave out in one line (`summary`) with every reason
//! behind Details, then the per-day view and one table grouped by the selected key, each
//! partial group marked with its reasons, the period's total under it.

mod summary;

use std::collections::BTreeMap;

use corral_protocol::detail::{DetailBody, UsageSeries};
use corral_types::{UsageBy, UsageGroup, UsageSummary, UsageTotal};
use jiff::civil::Date;

use super::{
    CustomRange, DayBar, DayStrip, Days, GroupRow, Local, MOST_DAYS, Page, Period, State, Status,
    UNKNOWN, count, local, money, plural, series_request, tokens,
};
use crate::model::daemon::{DetailCache, Feed};

/// What the totals leave out besides the runs: tokens, prices and estimate coverage, each
/// reason a sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Coverage {
    reasons: Vec<String>,
}

/// The subscription topic of the usage totals.
const TOPIC: &str = "usage";
const DISCLAIMER: &str = "Figures are what agent hosts reported with `corral usage add`, one \
                          record per model request, counted on the local day it was made; a \
                          run counts toward the day it started. — marks an unknown figure. \
                          Usage is visibility only: nothing here routes work.";

fn status(cache: Option<&DetailCache>) -> Status {
    match cache {
        None => Status::Loading,
        Some(cache) => match (cache.body(), &cache.error) {
            (None, None) => Status::Loading,
            // The page's notice says the failure once; there is nothing older to call stale.
            (None, Some(_)) => Status::Ready,
            (Some(_), Some(_)) => Status::Stale,
            (Some(_), None) if cache.stale && !cache.loading => Status::Stale,
            (Some(_), None) => Status::Ready,
        },
    }
}

/// Requests with no reported cost.
fn unpriced(total: &UsageTotal) -> u64 {
    total.requests.saturating_sub(total.priced_requests)
}

/// Which runs reported usage on their start day, and how many did not.
fn covers(summary: &UsageSummary) -> String {
    let (covered, total) = (summary.covered_runs, summary.total_runs);
    if total == 0 {
        "no run started in this period".to_owned()
    } else if covered >= total {
        format!("all {total} runs reported usage on their start day")
    } else {
        let rest = total - covered;
        let other = if rest == 1 {
            "the other one".to_owned()
        } else {
            format!("the other {rest}")
        };
        format!("{covered} of {total} runs reported usage on their start day; {other} did not")
    }
}

/// Whether the estimate's coverage is known: a total from before the estimate metadata (or
/// merged with one) has no honest count of what the estimate covers or why requests are out.
/// A total without requests has nothing to cover.
fn estimate_known(total: &UsageTotal) -> bool {
    total.requests == 0 || total.estimate_metadata_available
}

/// Requests left out of the API-equivalent estimate because their token evidence was
/// incomplete; they are not evidence that no rate exists. `None` where the coverage is unknown.
fn missing_tokens(total: &UsageTotal) -> Option<u64> {
    estimate_known(total).then(|| {
        total
            .estimate_missing_token_requests
            .min(total.requests.saturating_sub(total.estimated_requests))
    })
}

/// Requests left out of the estimate with complete token evidence: no applicable rate was
/// known. Requests with incomplete token evidence are counted by [`missing_tokens`] instead.
/// `None` where the coverage is unknown.
fn unrated(total: &UsageTotal) -> Option<u64> {
    Some(
        total
            .requests
            .saturating_sub(total.estimated_requests)
            .saturating_sub(missing_tokens(total)?),
    )
}

/// Said where the estimate's coverage is unknown, in place of any count or rate claim.
const ESTIMATE_UNKNOWN: &str = "API-equivalent estimate coverage is unknown: some usage \
                                predates estimate metadata, so how many requests the figure \
                                includes, and why the rest are left out, is not known; any \
                                figure shown is a lower bound";

/// How much of a total's requests carried evidence for one token count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Evidence {
    /// No request did; a zero sum is then not a recorded zero.
    Missing,
    /// Some did, or how many did is unknown (a daemon older than the evidence counts): the sum
    /// is a lower bound.
    Partial(Option<u64>),
    /// A legacy zero sum with requests: whether any reported is unknown, so it is neither
    /// "none reported" nor a recorded zero.
    Unknown,
    /// Every request did, so an explicit zero is a zero.
    All,
}

fn evidence(requests: u64, reported: Option<u64>, sum: u64) -> Evidence {
    match reported {
        _ if requests == 0 => Evidence::Missing,
        Some(0) => Evidence::Missing,
        Some(n) if n >= requests => Evidence::All,
        Some(n) => Evidence::Partial(Some(n)),
        // A legacy positive sum is some evidence but not full coverage; a legacy zero is unknown.
        None if sum > 0 => Evidence::Partial(None),
        None => Evidence::Unknown,
    }
}

/// A count rounded down to the precision [`tokens`] shows, for a `≥` lower bound: rounding
/// to nearest could print a bound above the sum (1,051 as `1.1k`).
fn floor_tokens(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..1_000_000 => format!("{}.{}k", count / 1_000, count % 1_000 / 100),
        _ => format!("{}.{}M", count / 1_000_000, count % 1_000_000 / 100_000),
    }
}

fn input_evidence(total: &UsageTotal) -> Evidence {
    evidence(total.requests, total.input_requests, total.input)
}

fn output_evidence(total: &UsageTotal) -> Evidence {
    evidence(total.requests, total.output_requests, total.output)
}

/// A token count as its evidence allows: [`UNKNOWN`] with none, `≥` before a lower bound.
fn figure_of(evidence: Evidence, sum: u64) -> String {
    match evidence {
        Evidence::Missing | Evidence::Unknown => UNKNOWN.to_owned(),
        Evidence::Partial(_) => format!("≥{}", floor_tokens(sum)),
        Evidence::All => tokens(sum),
    }
}

/// Input and output together, for the day strip: complete only when both are.
fn used_words(total: &UsageTotal) -> String {
    let sum = total.input + total.output;
    let none = |evidence| matches!(evidence, Evidence::Missing | Evidence::Unknown);
    match (input_evidence(total), output_evidence(total)) {
        (input, output) if none(input) && none(output) => UNKNOWN.to_owned(),
        (Evidence::All, Evidence::All) => tokens(sum),
        _ => format!("≥{}", floor_tokens(sum)),
    }
}

/// Why a total's input and output tokens are unknown or partial; empty when both are complete.
fn token_reasons(total: &UsageTotal) -> Vec<String> {
    let requests = total.requests;
    let (input, output) = (input_evidence(total), output_evidence(total));
    if requests == 0 || (input == Evidence::All && output == Evidence::All) {
        return Vec::new();
    }
    if input == Evidence::Missing && output == Evidence::Missing {
        return vec!["no request reported input or output tokens".to_owned()];
    }
    [("input", input), ("output", output)]
        .into_iter()
        .filter_map(|(name, evidence)| match evidence {
            Evidence::All => None,
            Evidence::Missing => Some(format!("no request reported {name} tokens")),
            Evidence::Unknown => Some(format!(
                "{name} tokens: whether any request reported them is unknown"
            )),
            Evidence::Partial(Some(n)) => Some(format!(
                "{name} tokens reported on {n} of {} requests",
                count(requests)
            )),
            Evidence::Partial(None) => Some(format!(
                "{name} tokens: how many requests reported them is unknown"
            )),
        })
        .collect()
}

/// A part's reported-request count for one token count where it is known. A legacy part
/// (no count) with requests is unknown even at a zero sum, as it may have reported zeros; a
/// part without requests reported none.
fn known(reported: Option<u64>, requests: u64) -> Option<u64> {
    match reported {
        Some(n) => Some(n),
        None if requests == 0 => Some(0),
        None => None,
    }
}

/// Two parts' counts, each with its requests, added; unknown if either is. The token sums
/// keep their lower bound.
fn joined(a: (Option<u64>, u64), b: (Option<u64>, u64)) -> Option<u64> {
    Some(known(a.0, a.1)? + known(b.0, b.1)?)
}

/// Explain missing prices and the start-day limitation of run coverage.
fn coverage(summary: &UsageSummary) -> Option<Coverage> {
    let total = &summary.total;
    let mut reasons = Vec::new();
    let partial = token_reasons(total);
    if !partial.is_empty() {
        reasons.push(format!(
            "Input and output tokens are incomplete: {}.",
            partial.join("; ")
        ));
    }
    let missing = unpriced(total);
    if missing > 0 {
        reasons.push(format!(
            "{} of the {} requests reported no cost; any available tokens are counted, \
             but their reported cost is unknown.",
            count(missing),
            count(total.requests)
        ));
    }
    if !estimate_known(total) {
        reasons.push(format!("{ESTIMATE_UNKNOWN}."));
    }
    let no_tokens = missing_tokens(total).unwrap_or(0);
    let no_rate = unrated(total).unwrap_or(0);
    if no_tokens > 0 || no_rate > 0 {
        let mut why = Vec::new();
        if no_tokens > 0 {
            why.push(format!(
                "{} had incomplete token evidence",
                count(no_tokens)
            ));
        }
        if no_rate > 0 {
            why.push(format!(
                "{} had no API rate known for their model",
                count(no_rate)
            ));
        }
        reasons.push(format!(
            "The API-equivalent estimate covers {} of the {} requests; {}.",
            count(total.estimated_requests),
            count(total.requests),
            why.join(" and ")
        ));
    }
    if summary.covered_runs < summary.total_runs && total.estimate_usd.is_some() {
        reasons.push(
            "Run coverage counts only usage on each run's start day; later requests are still included. Runs without any reported usage are not included."
                .to_owned(),
        );
    }
    (!reasons.is_empty()).then_some(Coverage { reasons })
}

/// A reported cost, or [`UNKNOWN`] where no request reported a price.
fn cost(total: &UsageTotal) -> String {
    total.usd.map_or_else(|| UNKNOWN.to_owned(), money)
}

/// The API-equivalent estimate, or [`UNKNOWN`] where no request could be priced. Where the
/// coverage is unknown the sum is only a lower bound, shown floored after `≥`.
fn estimate(total: &UsageTotal) -> String {
    match total.estimate_usd {
        None => UNKNOWN.to_owned(),
        Some(usd) if estimate_known(total) => money(usd),
        Some(usd) => format!("≥{}", floor_money(usd)),
    }
}

/// Dollars rounded down to the precision [`money`] shows, for a `≥` lower bound.
fn floor_money(value: f64) -> String {
    if value >= 10.0 {
        format!("${}", count(value.floor() as u64))
    } else {
        format!("${:.2}", (value * 100.0).floor() / 100.0)
    }
}

/// Said behind Details: what the estimate column is and is not, beside the reported one.
const ESTIMATE_NOTE: &str = "Reported is the price agent hosts recorded. Est. cost is an \
                             API-equivalent estimate: the tokens at configured or known API \
                             rates. It is not a charge or a subscription quota, and is never \
                             added to Reported.";

/// Said under the table: the estimate column in one line.
const LEGEND: &str = "Est. cost is an API-equivalent estimate at known rates: not a charge or \
                      a quota, and never added to Reported.";

/// Said behind Details: corral holds no quota for any harness.
const QUOTA: &str = "Quota is unknown: no harness reports quota or remaining allowance.";

fn add(sum: &mut UsageTotal, total: &UsageTotal) {
    // The evidence counts are joined before the request counts they qualify change.
    let join = |a: Option<u64>, b: Option<u64>| joined((a, sum.requests), (b, total.requests));
    // Estimate coverage is known only where every part with requests knows it; a part without
    // requests has nothing to cover and is neutral.
    let estimate_metadata_available = match (sum.requests, total.requests) {
        (_, 0) => sum.estimate_metadata_available,
        (0, _) => total.estimate_metadata_available,
        _ => sum.estimate_metadata_available && total.estimate_metadata_available,
    };
    let input_requests = join(sum.input_requests, total.input_requests);
    let output_requests = join(sum.output_requests, total.output_requests);
    let cache_read_requests = join(sum.cache_read_requests, total.cache_read_requests);
    let cache_write_requests = join(sum.cache_write_requests, total.cache_write_requests);
    sum.requests += total.requests;
    sum.input += total.input;
    sum.output += total.output;
    sum.cache_read += total.cache_read;
    sum.cache_write += total.cache_write;
    sum.input_requests = input_requests;
    sum.output_requests = output_requests;
    sum.cache_read_requests = cache_read_requests;
    sum.cache_write_requests = cache_write_requests;
    sum.priced_requests += total.priced_requests;
    sum.estimate_metadata_available = estimate_metadata_available;
    sum.estimated_requests += total.estimated_requests;
    sum.estimate_missing_token_requests += total.estimate_missing_token_requests;
    sum.estimate_sources
        .extend(total.estimate_sources.iter().cloned());
    if let Some(usd) = total.usd {
        sum.usd = Some(sum.usd.unwrap_or(0.0) + usd);
    }
    // An unknown part adds nothing: the sum is unknown only where every part is.
    if let Some(usd) = total.estimate_usd {
        sum.estimate_usd = Some(sum.estimate_usd.unwrap_or(0.0) + usd);
    }
}

fn add_runs(sum: &mut Option<u64>, runs: Option<u64>) {
    if let Some(runs) = runs {
        *sum.get_or_insert(0) += runs;
    }
}

/// The series' day buckets as one summary: totals, runs and each key's group summed. A run
/// belongs to the one day it started, so the days' run counts add up.
fn merged(series: &UsageSeries) -> UsageSummary {
    let mut summary = UsageSummary::default();
    let mut groups: BTreeMap<Option<String>, UsageGroup> = BTreeMap::new();
    for day in &series.days {
        add(&mut summary.total, &day.summary.total);
        summary.covered_runs += day.summary.covered_runs;
        summary.total_runs += day.summary.total_runs;
        for group in &day.summary.groups {
            let sum = groups.entry(group.key.clone()).or_default();
            add(&mut sum.total, &group.total);
            add_runs(&mut sum.covered_runs, group.covered_runs);
            add_runs(&mut sum.total_runs, group.total_runs);
        }
    }
    summary.groups = groups
        .into_iter()
        .map(|(key, group)| UsageGroup { key, ..group })
        .collect();
    summary
}

fn missing(by: UsageBy) -> &'static str {
    match by {
        UsageBy::Profile => "no profile recorded",
        UsageBy::Model => "no model recorded",
        UsageBy::Task => "no task recorded",
        UsageBy::Day => "no day recorded",
    }
}

fn header(by: UsageBy) -> &'static str {
    match by {
        UsageBy::Profile => "Profile",
        UsageBy::Model => "Model",
        UsageBy::Task => "Task",
        UsageBy::Day => "Day",
    }
}

fn long_day(day: Date) -> String {
    day.strftime("%a %-d %b %Y").to_string()
}

fn short_day(day: Date) -> String {
    day.strftime("%a %-d %b").to_string()
}

/// A group's key as shown: a day as its real date, a task with its title when the board has it.
fn key_words(by: UsageBy, key: &str, feed: &Feed) -> (String, Option<String>) {
    match by {
        UsageBy::Day => match key.parse::<Date>() {
            Ok(day) => (long_day(day), None),
            Err(_) => (key.to_owned(), None),
        },
        UsageBy::Task => (
            key.to_owned(),
            feed.board.task(key).map(|task| task.title().to_owned()),
        ),
        UsageBy::Profile | UsageBy::Model => (key.to_owned(), None),
    }
}

/// A group's runs covered of those started in the period, whether some of those runs are
/// missing, and why the rest and any unpriced requests are missing.
fn group_coverage(group: &UsageGroup) -> (String, bool, Option<String>) {
    let mut reasons = Vec::new();
    let mut runs_missing = false;
    let covered = match (group.covered_runs, group.total_runs) {
        (Some(covered), Some(total)) if total > 0 => {
            if covered < total {
                runs_missing = true;
                reasons.push(format!(
                    "{} without matching usage in this group on the start day",
                    plural(total - covered, "run", "runs")
                ));
            }
            if covered >= total {
                covered.to_string()
            } else {
                format!("{covered} of {total}")
            }
        }
        _ => UNKNOWN.to_owned(),
    };
    reasons.extend(token_reasons(&group.total));
    let unpriced = unpriced(&group.total);
    if unpriced > 0 {
        reasons.push(format!(
            "{} with no reported cost",
            plural(unpriced, "request", "requests")
        ));
    }
    if !estimate_known(&group.total) {
        reasons.push("estimate coverage unknown (usage predates estimate metadata)".to_owned());
    }
    let no_tokens = missing_tokens(&group.total).unwrap_or(0);
    if no_tokens > 0 {
        reasons.push(format!(
            "{} not in the estimate (incomplete token evidence)",
            plural(no_tokens, "request", "requests")
        ));
    }
    let no_rate = unrated(&group.total).unwrap_or(0);
    if no_rate > 0 {
        reasons.push(format!(
            "{} not in the estimate (no API rate known)",
            plural(no_rate, "request", "requests")
        ));
    }
    let reasons = (!reasons.is_empty()).then(|| reasons.join(" · "));
    (covered, runs_missing, reasons)
}

fn rows(by: UsageBy, summary: &UsageSummary, feed: &Feed) -> Vec<GroupRow> {
    let mut ordered: Vec<&UsageGroup> = summary
        .groups
        .iter()
        .filter(|group| group.total.requests > 0 || group.total_runs.is_some_and(|runs| runs > 0))
        .collect();
    match by {
        UsageBy::Day => ordered.reverse(),
        UsageBy::Profile | UsageBy::Model | UsageBy::Task => ordered.sort_by(|a, b| {
            b.total
                .requests
                .cmp(&a.total.requests)
                .then(b.total_runs.cmp(&a.total_runs))
                .then(a.key.cmp(&b.key))
        }),
    }
    ordered
        .into_iter()
        .map(|group| {
            let (key, named, detail) = match &group.key {
                Some(key) => {
                    let (words, detail) = key_words(by, key, feed);
                    (words, true, detail)
                }
                None => (missing(by).to_owned(), false, None),
            };
            let id = format!("usage-row-{}", group.key.as_deref().unwrap_or("none"));
            group_row(id, key, named, detail, group)
        })
        .collect()
}

/// One group's figures beside the runs it covers.
fn group_row(
    id: String,
    key: String,
    named: bool,
    detail: Option<String>,
    group: &UsageGroup,
) -> GroupRow {
    let reported = group.total.requests > 0;
    let figure = |value: fn(&UsageTotal) -> String| {
        if reported {
            value(&group.total)
        } else {
            UNKNOWN.into()
        }
    };
    let (covered, runs_missing, reasons) = group_coverage(group);
    GroupRow {
        id,
        key,
        named,
        detail,
        covered,
        runs_missing,
        reasons,
        requests: figure(|t| count(t.requests)),
        input: figure(|t| figure_of(input_evidence(t), t.input)),
        output: figure(|t| figure_of(output_evidence(t), t.output)),
        cost: figure(cost),
        estimate: figure(estimate),
    }
}

/// The period's totals as the table's last row, its runs covered of those started.
fn total_row(summary: &UsageSummary) -> GroupRow {
    let group = UsageGroup {
        key: None,
        total: summary.total.clone(),
        covered_runs: Some(summary.covered_runs),
        total_runs: Some(summary.total_runs),
    };
    // The coverage line above says why the totals are partial; the row does not repeat it.
    GroupRow {
        reasons: None,
        ..group_row("usage-row-total".into(), "Total".into(), true, None, &group)
    }
}

/// The period's days by tokens reported, against its busiest day.
fn day_strip(days: Days, series: &UsageSeries) -> DayStrip {
    let by_day: BTreeMap<&str, &UsageSummary> = series
        .days
        .iter()
        .map(|day| (day.day.as_str(), &day.summary))
        .collect();
    let empty = UsageSummary::default();
    let facts: Vec<(Date, &UsageSummary)> = days
        .each()
        .map(|day| {
            let key = day.to_string();
            (day, by_day.get(key.as_str()).copied().unwrap_or(&empty))
        })
        .collect();
    let used = |summary: &UsageSummary| summary.total.input + summary.total.output;
    let busiest = facts
        .iter()
        .max_by_key(|(day, summary)| (used(summary), std::cmp::Reverse(*day)))
        .filter(|(_, summary)| used(summary) > 0);
    let peak = busiest.map_or(0, |(_, summary)| used(summary));
    let bars = facts
        .iter()
        .map(|(day, summary)| {
            let total = &summary.total;
            let mut hint = if total.requests == 0 {
                format!("{}: no requests reported", short_day(*day))
            } else {
                format!(
                    "{}: {} tokens · {} in · {} out · {}",
                    short_day(*day),
                    used_words(total),
                    figure_of(input_evidence(total), total.input),
                    figure_of(output_evidence(total), total.output),
                    plural(total.requests, "request", "requests")
                )
            };
            if total.usd.is_some() {
                hint.push_str(&format!(" · reported {}", cost(total)));
            }
            if total.estimate_usd.is_some() {
                hint.push_str(&format!(" · API-equivalent estimate {}", estimate(total)));
            }
            if summary.total_runs > 0 {
                hint.push_str(&format!(
                    " · covers {} of {} runs",
                    summary.covered_runs, summary.total_runs
                ));
            }
            DayBar {
                fraction: if peak == 0 {
                    0.0
                } else {
                    used(summary) as f32 / peak as f32
                },
                tokens: used(summary),
                tick: day.strftime("%-d %b").to_string(),
                hint,
            }
        })
        .collect();
    let reporting = || {
        facts
            .iter()
            .filter(|(_, summary)| summary.total.requests > 0)
    };
    let complete = |summary: &UsageSummary| {
        input_evidence(&summary.total) == Evidence::All
            && output_evidence(&summary.total) == Evidence::All
    };
    let note = match busiest {
        Some((day, summary)) if reporting().all(|(_, summary)| complete(summary)) => format!(
            "busiest {}: {} tokens",
            short_day(*day),
            tokens(used(summary))
        ),
        // Another day may have more tokens than it reported, so this one is only the most
        // reported, and its own figure is a lower bound unless its evidence is complete.
        Some((day, summary)) => format!(
            "most reported {}: {} tokens",
            short_day(*day),
            used_words(&summary.total)
        ),
        None => {
            let seen = |want: fn(Evidence) -> bool| {
                reporting().any(|(_, summary)| {
                    want(input_evidence(&summary.total)) || want(output_evidence(&summary.total))
                })
            };
            if seen(|evidence| evidence == Evidence::Unknown) {
                "token reporting unknown".to_owned()
            } else if seen(|evidence| matches!(evidence, Evidence::All | Evidence::Partial(_))) {
                "every token count reported was zero".to_owned()
            } else {
                "no day reported any tokens".to_owned()
            }
        }
    };
    DayStrip {
        title: "Tokens per day",
        spoken: format!(
            "Tokens per day, {} days; {note}{}",
            days.count(),
            if busiest.is_some() && !reporting().all(|(_, summary)| complete(summary)) {
                "; some days' tokens are incomplete or unknown, so the busiest may differ"
            } else {
                ""
            }
        ),
        note,
        bars,
        peak,
        prompt: "Point at, touch or focus the chart and use ← → for a day's figures",
        first: days.first.strftime("%-d %b").to_string(),
        last: days.last.strftime("%-d %b").to_string(),
    }
}

/// `UTC+01:00`, `UTC−05:00`, or `UTC` itself.
fn offset_words(minutes: i16) -> String {
    if minutes == 0 {
        return "UTC".into();
    }
    let sign = if minutes < 0 { '−' } else { '+' };
    let minutes = minutes.unsigned_abs();
    format!("UTC{sign}{:02}:{:02}", minutes / 60, minutes % 60)
}

/// "Thu 22 Oct – Wed 18 Nov 2026 · 28 days".
fn range_words(days: Days) -> String {
    let range = if days.first == days.last {
        long_day(days.first)
    } else if days.first.year() == days.last.year() {
        format!("{} – {}", short_day(days.first), long_day(days.last))
    } else {
        format!("{} – {}", long_day(days.first), long_day(days.last))
    };
    format!(
        "{range} · {}",
        plural(u64::try_from(days.count()).unwrap_or(0), "day", "days")
    )
}

/// "Days are counted in local time, Europe/Warsaw (UTC+01:00)."
fn zone_words(local: &Local) -> String {
    let zone = match &local.zone {
        Some(zone) => format!("{zone} ({})", offset_words(local.offset_minutes)),
        None => offset_words(local.offset_minutes),
    };
    format!("Days are counted in local time, {zone}.")
}

/// The estimate note with the rate sources the period's estimate used.
fn estimate_words(summary: &UsageSummary) -> String {
    let sources = &summary.total.estimate_sources;
    if sources.is_empty() {
        ESTIMATE_NOTE.to_owned()
    } else {
        format!(
            "{ESTIMATE_NOTE} Rate sources: {}.",
            sources.iter().cloned().collect::<Vec<_>>().join(", ")
        )
    }
}

/// Suppress repeated transport wrappers without hiding the actual daemon cause.
fn cause(mut error: &str) -> &str {
    while let Some((wrapper, rest)) = error.split_once(": ") {
        if !["app topic unavailable", "unavailable", "usage unavailable"]
            .iter()
            .any(|prefix| wrapper.eq_ignore_ascii_case(prefix))
        {
            break;
        }
        error = rest;
    }
    error
}

fn notices(topic: Option<&str>, series: Option<&str>) -> Vec<String> {
    let text = match (topic.map(cause), series.map(cause)) {
        (None, None) => return vec![],
        (Some(topic), Some(series)) if topic != series => {
            format!("{topic}; for the period: {series}")
        }
        (Some(error), _) | (_, Some(error)) => error.to_owned(),
    };
    vec![format!("Couldn't read usage: {text}")]
}

fn custom(state: &State) -> Option<CustomRange> {
    (state.period == Period::Custom).then(|| CustomRange {
        from: "From",
        to: "To",
        placeholder: "YYYY-MM-DD",
        apply: "Apply",
        refused: state.refused.clone(),
        hint: format!("both days included, at most {MOST_DAYS} days"),
    })
}

/// The live page for the chosen period and grouping.
pub fn page(state: &State, feed: &Feed) -> Page {
    let by = state.by;
    let local = local(feed);
    let days = state.days(local.today);
    let cache = feed.detail(&series_request(state, &local));
    let series = cache.and_then(|cache| match cache.body() {
        Some(DetailBody::UsageSeries(series)) => Some(series),
        _ => None,
    });
    let summary = series.map(merged);
    let rows = summary
        .as_ref()
        .map(|summary| rows(by, summary, feed))
        .unwrap_or_default();
    let series_error = cache
        .and_then(|cache| cache.error.as_ref())
        .map(|error| error.message.as_str());
    let rows_empty = match (&summary, series_error) {
        (Some(_), _) if rows.is_empty() => {
            Some("No run started and no usage was reported in this period.")
        }
        (Some(_), _) => None,
        (None, Some(_)) => Some("The period's totals could not be read."),
        (None, None) => Some("Loading the period's totals from corrald…"),
    };
    let gaps = summary.as_ref().map(|summary| {
        let notes = vec![
            estimate_words(summary),
            QUOTA.to_owned(),
            DISCLAIMER.to_owned(),
            zone_words(&local),
        ];
        summary::gaps(summary, &rows, state.details, notes)
    });
    Page {
        status: status(cache),
        head: range_words(days),
        stats: summary.as_ref().map(summary::stats).unwrap_or_default(),
        gaps,
        days: series.map(|series| day_strip(days, series)),
        group_title: format!("By {}", header(by).to_lowercase()),
        headers: [
            header(by),
            "Runs",
            "Input",
            "Output",
            "Est. cost",
            "Reported",
        ],
        total: summary.as_ref().filter(|_| !rows.is_empty()).map(total_row),
        estimate_note: LEGEND.to_owned(),
        rows,
        rows_empty,
        notices: notices(
            feed.topics
                .get(TOPIC)
                .and_then(|topic| topic.error.as_ref())
                .map(|error| error.message.as_str()),
            series_error,
        ),
        custom: custom(state),
    }
}

#[cfg(test)]
#[path = "live_tests.rs"]
mod tests;
