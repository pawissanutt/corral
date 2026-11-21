//! corral's recorded usage over the chosen period as the Usage page. Run coverage counts
//! only requests on each run's start day; costs also include requests on later days. A note
//! says how many requests reported tokens without a price. Then the per-day view and one table
//! grouped by the selected key, each group with its own coverage and reasons, the period's
//! total under it.

use std::collections::BTreeMap;

use corral_protocol::detail::{DetailBody, UsageSeries};
use corral_types::{UsageBy, UsageGroup, UsageSummary, UsageTotal};
use jiff::civil::Date;

use super::{
    Coverage, CustomRange, DayBar, DayStrip, Days, GroupRow, Local, MOST_DAYS, Page, Period,
    SUBTITLE, State, Status, UNKNOWN, count, local, money, plural, series_request, tokens,
};
use crate::model::daemon::{DetailCache, Feed};

/// The subscription topic of the usage totals.
const TOPIC: &str = "usage";
const DISCLAIMER: &str = "Figures are what agent hosts reported with `corral usage add`, one \
                          record per model request, counted on the local day it was made; a \
                          run counts toward the day it started. — marks a figure corral has \
                          not recorded. Usage is visibility only: nothing here routes work.";

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

/// Requests that reported tokens but no price.
fn unpriced(total: &UsageTotal) -> u64 {
    total.requests.saturating_sub(total.priced_requests)
}

/// The head's line names runs with no usage evidence.
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

/// Requests left out of the API-equivalent estimate because their token evidence was
/// incomplete; they are not evidence that no rate exists.
fn missing_tokens(total: &UsageTotal) -> u64 {
    total
        .estimate_missing_token_requests
        .min(total.requests.saturating_sub(total.estimated_requests))
}

/// Requests left out of the estimate with complete token evidence: no applicable rate was
/// known. Requests with incomplete token evidence are counted by [`missing_tokens`] instead.
fn unrated(total: &UsageTotal) -> u64 {
    total
        .requests
        .saturating_sub(total.estimated_requests)
        .saturating_sub(missing_tokens(total))
}

/// Explain missing prices and the start-day limitation of run coverage.
fn coverage(summary: &UsageSummary) -> Option<Coverage> {
    let total = &summary.total;
    let mut reasons = Vec::new();
    let missing = unpriced(total);
    if missing > 0 {
        reasons.push(format!(
            "{} of the {} requests reported tokens but no price: their tokens are counted, \
             their reported cost is not.",
            count(missing),
            count(total.requests)
        ));
    }
    let no_tokens = missing_tokens(total);
    let no_rate = unrated(total);
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

/// The API-equivalent estimate, or [`UNKNOWN`] where no request could be priced.
fn estimate(total: &UsageTotal) -> String {
    total.estimate_usd.map_or_else(|| UNKNOWN.to_owned(), money)
}

/// Said under the table: what the estimate column is and is not, beside the reported one.
const ESTIMATE_NOTE: &str = "Reported is the price agent hosts recorded. API est. is an \
                             API-equivalent estimate: the tokens at configured or known API \
                             rates. It is not a charge or a subscription quota, and is never \
                             added to Reported.";

/// Said under the table: corral holds no quota for any harness.
const QUOTA: &str = "Quota is unknown: no harness reports quota or remaining allowance.";

fn add(sum: &mut UsageTotal, total: &UsageTotal) {
    sum.requests += total.requests;
    sum.input += total.input;
    sum.output += total.output;
    sum.cache_read += total.cache_read;
    sum.cache_write += total.cache_write;
    sum.priced_requests += total.priced_requests;
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
            format!("{covered} of {total}")
        }
        _ => UNKNOWN.to_owned(),
    };
    let unpriced = unpriced(&group.total);
    if unpriced > 0 {
        reasons.push(format!(
            "{} with tokens but no price",
            plural(unpriced, "request", "requests")
        ));
    }
    let no_tokens = missing_tokens(&group.total);
    if no_tokens > 0 {
        reasons.push(format!(
            "{} not in the estimate (incomplete token evidence)",
            plural(no_tokens, "request", "requests")
        ));
    }
    let no_rate = unrated(&group.total);
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
        input: figure(|t| tokens(t.input)),
        output: figure(|t| tokens(t.output)),
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
    // The head and the note above say why the totals are partial; the row does not repeat it.
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
            let mut hint = format!(
                "{}: {} tokens · {}",
                short_day(*day),
                tokens(used(summary)),
                plural(total.requests, "request", "requests")
            );
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
                hint,
            }
        })
        .collect();
    let note = match busiest {
        Some((day, summary)) => format!(
            "busiest {}: {} tokens",
            short_day(*day),
            tokens(used(summary))
        ),
        None => "no day reported any tokens".to_owned(),
    };
    DayStrip {
        title: "Tokens per day",
        spoken: format!("Tokens per day, {} days; {note}", days.count()),
        note,
        bars,
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

/// "Thu 22 Oct – Wed 18 Nov 2026 · 28 days · local time, Europe/Warsaw (UTC+01:00)".
fn period_words(days: Days, local: &Local) -> String {
    let range = if days.first == days.last {
        long_day(days.first)
    } else if days.first.year() == days.last.year() {
        format!("{} – {}", short_day(days.first), long_day(days.last))
    } else {
        format!("{} – {}", long_day(days.first), long_day(days.last))
    };
    let zone = match &local.zone {
        Some(zone) => format!("{zone} ({})", offset_words(local.offset_minutes)),
        None => offset_words(local.offset_minutes),
    };
    format!(
        "{range} · {} · local time, {zone}",
        plural(u64::try_from(days.count()).unwrap_or(0), "day", "days")
    )
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
    Page {
        status: status(cache),
        head: summary.as_ref().map_or_else(|| SUBTITLE.to_owned(), covers),
        period: period_words(days, &local),
        coverage: summary.as_ref().and_then(coverage),
        days: series.map(|series| day_strip(days, series)),
        group_title: format!("By {}", header(by).to_lowercase()),
        headers: [
            header(by),
            "Runs",
            "Input",
            "Output",
            "Reported",
            "API est.",
        ],
        total: summary.as_ref().filter(|_| !rows.is_empty()).map(total_row),
        quota: QUOTA,
        estimate_note: summary.as_ref().map_or_else(
            || ESTIMATE_NOTE.to_owned(),
            |summary| {
                let sources = &summary.total.estimate_sources;
                if sources.is_empty() {
                    ESTIMATE_NOTE.to_owned()
                } else {
                    format!(
                        "{ESTIMATE_NOTE} Rate sources: {}.",
                        sources.iter().cloned().collect::<Vec<_>>().join(", ")
                    )
                }
            },
        ),
        rows,
        rows_empty,
        disclaimer: DISCLAIMER,
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
mod tests {
    use super::*;
    use crate::model::daemon::detail_key;
    use corral_types::UsageDay;

    /// The header says what the partial totals cover and why the rest are missing; each group
    /// states its own coverage with its reason (a run on a harness that reports no usage, a
    /// request with tokens but no price); a profile with only silent runs still lists its runs
    /// rather than vanishing; the day buckets add up; and quota is said to be unknown.
    #[test]
    fn coverage_is_stated_overall_and_per_group_with_the_reason_the_rest_is_missing() {
        let total = UsageTotal {
            requests: 4,
            input: 1200,
            output: 300,
            priced_requests: 2,
            usd: Some(0.5),
            ..UsageTotal::default()
        };
        let day = |day: &str, groups: Vec<UsageGroup>, covered, runs| UsageDay {
            day: day.into(),
            summary: UsageSummary {
                total: groups.iter().fold(UsageTotal::default(), |mut sum, group| {
                    add(&mut sum, &group.total);
                    sum
                }),
                groups,
                covered_runs: covered,
                total_runs: runs,
            },
        };
        let group = |key: &str, total: UsageTotal, covered, runs| UsageGroup {
            key: Some(key.into()),
            total,
            covered_runs: Some(covered),
            total_runs: Some(runs),
        };
        let series = UsageSeries {
            days: vec![
                day(
                    "2025-11-14",
                    vec![group("claude", UsageTotal::default(), 0, 1)],
                    0,
                    1,
                ),
                day(
                    "2025-11-15",
                    vec![
                        group("pi", total, 1, 1),
                        group("claude", UsageTotal::default(), 0, 1),
                    ],
                    1,
                    2,
                ),
            ],
        };
        let mut feed = Feed {
            now: 1_763_200_000,
            ..Feed::default()
        };
        let state = State::default();
        let target = series_request(&state, &local(&feed));
        feed.details.insert(
            detail_key(&target),
            DetailCache {
                target,
                pages: Vec::new(),
                loading: false,
                stale: false,
                error: None,
                merged: Some(DetailBody::UsageSeries(series)),
            },
        );
        let page = page(&state, &feed);
        assert!(page.head.contains("1 of 3 runs"), "{}", page.head);
        assert!(page.head.contains("reported usage on their start day"));
        assert!(page.coverage.unwrap().reasons[0].contains("no price"));
        let total = page.total.as_ref().unwrap();
        assert_eq!(total.covered, "1 of 3");
        let claude = page.rows.iter().find(|row| row.key == "claude").unwrap();
        assert_eq!(claude.covered, "0 of 2");
        assert_eq!(claude.requests, UNKNOWN);
        assert!(
            claude
                .reasons
                .as_deref()
                .unwrap()
                .contains("without matching usage in this group on the start day")
        );
        let pi = page.rows.iter().find(|row| row.key == "pi").unwrap();
        assert_eq!(pi.covered, "1 of 1");
        assert!(pi.reasons.as_deref().unwrap().contains("no price"));
        assert!(!page.headers.contains(&"Run time"));
        let days = page.days.unwrap();
        assert_eq!(days.bars.len(), 30);
        assert_eq!(
            days.bars.iter().filter(|bar| bar.fraction == 1.0).count(),
            1
        );
    }

    /// A request left out of the estimate for incomplete token evidence is never described as
    /// having no rate; only requests with complete tokens and no rate say that. Both the
    /// missing-token count and the split survive the sum across days.
    #[test]
    fn missing_token_evidence_is_not_reported_as_a_missing_rate() {
        let group = |requests, estimated, missing_tokens| UsageGroup {
            key: Some("pi".into()),
            total: UsageTotal {
                requests,
                estimated_requests: estimated,
                estimate_missing_token_requests: missing_tokens,
                estimate_usd: (estimated > 0).then_some(1.0),
                ..UsageTotal::default()
            },
            covered_runs: Some(1),
            total_runs: Some(1),
        };
        let day = |day: &str, group: UsageGroup| UsageDay {
            day: day.into(),
            summary: UsageSummary {
                total: group.total.clone(),
                groups: vec![group],
                covered_runs: 1,
                total_runs: 1,
            },
        };
        // Day one: 1 estimated, 2 missing tokens. Day two: 1 estimated, 1 missing tokens, 1 no rate.
        let series = UsageSeries {
            days: vec![
                day("2025-11-14", group(3, 1, 2)),
                day("2025-11-15", group(3, 1, 1)),
            ],
        };
        let merged = merged(&series);
        assert_eq!(merged.total.estimate_missing_token_requests, 3);
        assert_eq!(missing_tokens(&merged.total), 3);
        assert_eq!(unrated(&merged.total), 1);
        let reasons = coverage(&merged).unwrap().reasons.join(" ");
        assert!(
            reasons.contains("covers 2 of the 6 requests; 3 had incomplete token evidence and 1 had no API rate known"),
            "{reasons}"
        );
        let (_, _, row) = group_coverage(&merged.groups[0]);
        let row = row.unwrap();
        assert!(
            row.contains("3 requests not in the estimate (incomplete token evidence)"),
            "{row}"
        );
        assert!(
            row.contains("1 request not in the estimate (no API rate known)"),
            "{row}"
        );
        // Only missing tokens: nothing claims a rate is missing.
        let only = group_coverage(&group(2, 0, 2)).2.unwrap();
        assert!(only.contains("incomplete token evidence"), "{only}");
        assert!(!only.contains("no API rate"), "{only}");
        let only = coverage(&UsageSummary {
            total: group(2, 0, 2).total,
            ..UsageSummary::default()
        })
        .unwrap()
        .reasons
        .join(" ");
        assert!(!only.contains("no API rate"), "{only}");
    }

    /// The estimate is its own column: it sums across days without ever joining the
    /// reported cost, stays unknown where nothing could be priced, and its coverage names the
    /// requests without a rate and the start-day limitation of run coverage.
    #[test]
    fn estimate_is_kept_apart_from_reported_cost_and_its_coverage_is_stated() {
        let pi = |requests, priced, usd, estimated, estimate| UsageGroup {
            key: Some("pi".into()),
            total: UsageTotal {
                requests,
                priced_requests: priced,
                usd,
                estimated_requests: estimated,
                estimate_usd: estimate,
                ..UsageTotal::default()
            },
            covered_runs: Some(1),
            total_runs: Some(2),
        };
        let codex = UsageGroup {
            key: Some("codex".into()),
            total: UsageTotal {
                requests: 1,
                ..UsageTotal::default()
            },
            covered_runs: Some(1),
            total_runs: Some(1),
        };
        let day = |day: &str, groups: Vec<UsageGroup>| UsageDay {
            day: day.into(),
            summary: UsageSummary {
                total: groups.iter().fold(UsageTotal::default(), |mut sum, group| {
                    add(&mut sum, &group.total);
                    sum
                }),
                covered_runs: groups.iter().filter_map(|g| g.covered_runs).sum(),
                total_runs: groups.iter().filter_map(|g| g.total_runs).sum(),
                groups,
            },
        };
        let series = UsageSeries {
            days: vec![
                day("2025-11-14", vec![pi(2, 2, Some(0.5), 2, Some(1.25))]),
                day("2025-11-15", vec![pi(2, 0, None, 1, Some(0.75)), codex]),
            ],
        };
        let mut feed = Feed {
            now: 1_763_200_000,
            ..Feed::default()
        };
        let state = State::default();
        let target = series_request(&state, &local(&feed));
        feed.details.insert(
            detail_key(&target),
            DetailCache {
                target,
                pages: Vec::new(),
                loading: false,
                stale: false,
                error: None,
                merged: Some(DetailBody::UsageSeries(series)),
            },
        );
        let page = page(&state, &feed);
        assert_eq!(page.headers[4..], ["Reported", "API est."]);
        assert!(page.estimate_note.contains("not a charge"));
        let total = page.total.as_ref().unwrap();
        assert_eq!(total.cost, "$0.50");
        assert_eq!(total.estimate, "$2.00");
        assert_eq!(
            estimate(&UsageTotal {
                estimate_usd: Some(0.002),
                ..UsageTotal::default()
            }),
            "<$0.01"
        );
        assert_eq!(
            estimate(&UsageTotal {
                estimate_usd: Some(0.0),
                ..UsageTotal::default()
            }),
            "$0.00"
        );
        let pi = page.rows.iter().find(|row| row.key == "pi").unwrap();
        assert_eq!((pi.cost.as_str(), pi.estimate.as_str()), ("$0.50", "$2.00"));
        assert!(
            pi.reasons
                .as_deref()
                .unwrap()
                .contains("not in the estimate")
        );
        let codex = page.rows.iter().find(|row| row.key == "codex").unwrap();
        assert_eq!(
            (codex.cost.as_str(), codex.estimate.as_str()),
            (UNKNOWN, UNKNOWN)
        );
        let reasons = page.coverage.unwrap().reasons.join(" ");
        assert!(reasons.contains("covers 3 of the 5 requests"), "{reasons}");
        assert!(
            reasons.contains("later requests are still included"),
            "{reasons}"
        );
    }
}
