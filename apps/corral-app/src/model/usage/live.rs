//! corral's recorded usage over the chosen period as the Usage page. The coverage line comes
//! first: how many of the runs started in the period reported any usage, since a harness that
//! reports no usage leaves its runs out of every total, and how many requests reported tokens
//! without a price, since the cost leaves those out. Then the period's totals, the per-day view
//! and one table grouped by the selected key, each group with its own coverage and reasons.

use std::collections::BTreeMap;

use corral_protocol::detail::{DetailBody, UsageSeries};
use corral_types::{UsageBy, UsageGroup, UsageSummary, UsageTotal};
use jiff::civil::Date;

use super::{
    Coverage, CustomRange, DayBar, DayStrip, Days, Figure, GroupRow, Local, MOST_DAYS, Page,
    Period, State, Status, UNKNOWN, count, local, money, plural, series_request, tokens,
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

/// "Covers N of M runs started in this period", and each plain reason the rest is missing.
fn coverage(summary: &UsageSummary) -> Coverage {
    let (covered, total) = (summary.covered_runs, summary.total_runs);
    let mut reasons = Vec::new();
    let line = if total == 0 {
        "No run started in this period, so there is no run for these totals to cover.".to_owned()
    } else if covered >= total {
        format!("Covers all {total} runs started in this period: every one reported its usage.")
    } else {
        let rest = total - covered;
        reasons.push(if rest == 1 {
            "The other run ran on a harness that reports no usage, so its tokens and cost are not \
             in these totals."
                .to_owned()
        } else {
            format!(
                "The other {rest} runs ran on a harness that reports no usage, so their tokens \
                 and cost are not in these totals."
            )
        });
        format!("Covers {covered} of {total} runs started in this period.")
    };
    let missing = unpriced(&summary.total);
    if missing > 0 {
        reasons.push(format!(
            "{} of the {} requests reported tokens but no price: their tokens are counted, \
             their cost is not.",
            count(missing),
            count(summary.total.requests)
        ));
    }
    Coverage {
        line,
        partial: !reasons.is_empty(),
        reasons,
    }
}

/// A cost and, when some requests reported no price, which share it sums.
fn cost(total: &UsageTotal) -> (String, Option<String>) {
    let figure = total.usd.map_or_else(|| UNKNOWN.to_owned(), money);
    let note = match (total.priced_requests, total.requests) {
        (_, 0) => None,
        (0, _) => Some("no request reported a price".to_owned()),
        (priced, requests) if priced < requests => Some(format!(
            "sums the {} of {} requests that reported a price",
            count(priced),
            count(requests)
        )),
        _ => None,
    };
    (figure, note)
}

fn figures(total: &UsageTotal) -> Vec<Figure> {
    let (value, detail) = cost(total);
    let figure = |label, value| Figure {
        label,
        value,
        detail: None,
    };
    vec![
        Figure {
            label: "Cost",
            value,
            detail,
        },
        figure("Tokens in", tokens(total.input)),
        figure("Tokens out", tokens(total.output)),
        figure(
            "Cache",
            format!(
                "{} read · {} written",
                tokens(total.cache_read),
                tokens(total.cache_write)
            ),
        ),
        figure("Requests", count(total.requests)),
        Figure {
            label: "Quota",
            value: "unknown".into(),
            detail: Some("no harness reports quota or remaining allowance".into()),
        },
    ]
}

fn add(sum: &mut UsageTotal, total: &UsageTotal) {
    sum.requests += total.requests;
    sum.input += total.input;
    sum.output += total.output;
    sum.cache_read += total.cache_read;
    sum.cache_write += total.cache_write;
    sum.priced_requests += total.priced_requests;
    if let Some(usd) = total.usd {
        sum.usd = Some(sum.usd.unwrap_or(0.0) + usd);
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
                    "{} ran on a harness that reports no usage",
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
                key,
                named,
                detail,
                covered,
                runs_missing,
                reasons,
                requests: figure(|t| count(t.requests)),
                input: figure(|t| tokens(t.input)),
                output: figure(|t| tokens(t.output)),
                cost: figure(|t| cost(t).0),
            }
        })
        .collect()
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
                hint.push_str(&format!(" · {}", cost(total).0));
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
        period: period_words(days, &local),
        coverage: summary.as_ref().map(coverage),
        figures: summary
            .as_ref()
            .map(|summary| figures(&summary.total))
            .unwrap_or_default(),
        days: series.map(|series| day_strip(days, series)),
        group_title: format!("By {}", header(by).to_lowercase()),
        headers: [
            header(by),
            "Runs covered",
            "Requests",
            "Tokens in",
            "Tokens out",
            "Cost",
        ],
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
        let coverage = page.coverage.unwrap();
        assert!(coverage.partial);
        assert!(coverage.line.starts_with("Covers 1 of 3 runs"));
        assert!(coverage.reasons[0].contains("harness that reports no usage"));
        assert!(coverage.reasons[1].contains("no price"));
        let claude = page.rows.iter().find(|row| row.key == "claude").unwrap();
        assert_eq!(claude.covered, "0 of 2");
        assert_eq!(claude.requests, UNKNOWN);
        assert!(
            claude
                .reasons
                .as_deref()
                .unwrap()
                .contains("harness that reports no usage")
        );
        let pi = page.rows.iter().find(|row| row.key == "pi").unwrap();
        assert_eq!(pi.covered, "1 of 1");
        assert!(pi.reasons.as_deref().unwrap().contains("no price"));
        assert!(!page.headers.contains(&"Run time"));
        let quota = page
            .figures
            .iter()
            .find(|figure| figure.label == "Quota")
            .unwrap();
        assert_eq!(quota.value, "unknown");
        let days = page.days.unwrap();
        assert_eq!(days.bars.len(), 30);
        assert_eq!(
            days.bars.iter().filter(|bar| bar.fraction == 1.0).count(),
            1
        );
    }
}
