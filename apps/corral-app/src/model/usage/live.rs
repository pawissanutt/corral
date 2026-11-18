//! corral's recorded usage as the Usage page. The coverage line comes first: how many of the
//! recorded runs reported any usage, since a harness that reports no usage leaves its runs out
//! of every total. Then the totals and one table grouped by the selected key over the last
//! [`super::DAYS`] local days, each group's reported totals beside its recorded runs. A cost
//! sums only the requests that reported a price, and says so.

use std::collections::BTreeMap;

use corral_protocol::detail::{DetailBody, UsageSeries};
use corral_types::{UsageBy, UsageGroup, UsageSummary, UsageTotal};
use jiff::civil::Date;

use super::{
    Coverage, Figure, GroupRow, Page, Status, UNKNOWN, count, money, series_request, tokens,
};
use crate::model::daemon::Feed;

/// The subscription topic of the usage totals.
const TOPIC: &str = "usage";
const DISCLAIMER: &str = "Figures are what agent hosts reported with `corral usage add`, one \
                          record per model request; — marks a figure corral has not recorded. \
                          No harness reports quota or remaining allowance, so none is shown. \
                          Usage is visibility only: nothing here routes work.";

fn status(feed: &Feed) -> Status {
    match feed.topics.get(TOPIC) {
        None => Status::Loading,
        Some(topic) => match (&topic.error, feed.usage.is_none()) {
            (Some(_), true) => Status::Ready,
            (Some(_), false) => Status::Stale,
            (None, _) if topic.stale => Status::Stale,
            (None, true) if topic.loading => Status::Loading,
            (None, _) => Status::Ready,
        },
    }
}

/// "Covers N of M runs", and why the rest are missing.
fn coverage(summary: &UsageSummary) -> Coverage {
    let (covered, total) = (summary.covered_runs, summary.total_runs);
    if total == 0 {
        return Coverage {
            line: "No runs recorded yet, so there is no run for these totals to cover.".into(),
            partial: false,
        };
    }
    if covered >= total {
        return Coverage {
            line: format!("Covers all {total} runs: every run reported its usage."),
            partial: false,
        };
    }
    let rest = total - covered;
    let other = if rest == 1 {
        "The other run".to_owned()
    } else {
        format!("The other {rest} runs")
    };
    Coverage {
        line: format!(
            "Covers {covered} of {total} runs. {other} ran on a harness that reports no \
             usage, so their tokens and cost are not in these totals."
        ),
        partial: true,
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
    vec![
        Figure {
            label: "Cost",
            value,
            detail,
        },
        Figure {
            label: "Tokens in",
            value: tokens(total.input),
            detail: None,
        },
        Figure {
            label: "Tokens out",
            value: tokens(total.output),
            detail: None,
        },
        Figure {
            label: "Cache",
            value: format!(
                "{} read · {} written",
                tokens(total.cache_read),
                tokens(total.cache_write)
            ),
            detail: None,
        },
        Figure {
            label: "Requests",
            value: count(total.requests),
            detail: None,
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

/// The series' day buckets as one summary: totals, runs and each key's group summed.
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

/// A group's key as shown: a day as its real date, a task with its title when the board has it.
fn key_words(by: UsageBy, key: &str, feed: &Feed) -> (String, Option<String>) {
    match by {
        UsageBy::Day => match key.parse::<Date>() {
            Ok(day) => (day.strftime("%a %-d %b %Y").to_string(), None),
            Err(_) => (key.to_owned(), None),
        },
        UsageBy::Task => (
            key.to_owned(),
            feed.board.task(key).map(|task| task.title().to_owned()),
        ),
        UsageBy::Profile | UsageBy::Model => (key.to_owned(), None),
    }
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
            GroupRow {
                key,
                named,
                detail,
                runs: group.total_runs.map_or_else(|| UNKNOWN.into(), count),
                run_time: UNKNOWN.into(),
                requests: figure(|t| count(t.requests)),
                input: figure(|t| tokens(t.input)),
                output: figure(|t| tokens(t.output)),
                cost: figure(|t| cost(t).0),
            }
        })
        .collect()
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
        (Some(topic), Some(series)) if topic != series => format!("{topic}; by group: {series}"),
        (Some(error), _) | (_, Some(error)) => error.to_owned(),
    };
    vec![format!("Couldn't read usage: {text}")]
}

/// The live page for grouping `by`.
pub fn page(by: UsageBy, feed: &Feed) -> Page {
    let cache = feed.detail(&series_request(by, feed));
    let series = cache.and_then(|cache| match cache.body() {
        Some(DetailBody::UsageSeries(series)) => Some(merged(series)),
        _ => None,
    });
    // The series and its own summary agree with each other; the topic answers sooner.
    let summary = series.as_ref().or(feed.usage.as_ref());
    let rows = series
        .as_ref()
        .map(|series| rows(by, series, feed))
        .unwrap_or_default();
    let series_error = cache
        .and_then(|cache| cache.error.as_ref())
        .map(|error| error.message.as_str());
    let rows_empty = match (&series, series_error) {
        (Some(_), _) if rows.is_empty() => Some("corral has recorded no runs or usage yet."),
        (Some(_), _) => None,
        (None, Some(_)) => Some("The grouped totals could not be read."),
        (None, None) => Some("Loading the grouped totals from corrald…"),
    };
    Page {
        status: status(feed),
        coverage: summary.map(coverage),
        figures: summary
            .map(|summary| figures(&summary.total))
            .unwrap_or_default(),
        group_title: format!("By {}", header(by).to_lowercase()),
        headers: [
            header(by),
            "Runs",
            "Run time",
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::daemon::{DetailCache, detail_key};
    use corral_types::UsageDay;

    /// The header says what the partial totals cover and why the rest are missing; a profile
    /// with only silent runs still lists its runs with unknown totals rather than vanishing, and
    /// the day buckets add up.
    #[test]
    fn partial_totals_say_which_runs_they_cover_and_why_the_rest_are_missing() {
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
                    "2026-11-14",
                    vec![group("claude", UsageTotal::default(), 0, 1)],
                    0,
                    1,
                ),
                day(
                    "2026-11-15",
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
        let target = series_request(UsageBy::Profile, &feed);
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
        let page = page(UsageBy::Profile, &feed);
        let coverage = page.coverage.unwrap();
        assert!(coverage.partial);
        assert!(coverage.line.starts_with("Covers 1 of 3 runs."));
        assert!(page.figures[0].detail.is_some());
        let claude = page.rows.iter().find(|row| row.key == "claude").unwrap();
        assert_eq!(claude.runs, "2");
        assert_eq!(claude.requests, UNKNOWN);
    }
}
