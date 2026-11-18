//! corral's recorded usage as the Usage page. The coverage line comes first: how many of the
//! recorded runs reported any usage, and, for the rest, the harnesses they ran on, since a
//! harness that reports no usage leaves its runs out of every total. Then the totals and one
//! table grouped by the selected key, each group's reported totals beside its recorded runs and
//! their time. A cost sums only the requests that reported a price, and says so.

use std::collections::{BTreeMap, BTreeSet};

use corral_protocol::detail::{DetailBody, UsageSeries};
use corral_types::{Run, UsageBy, UsageSummary, UsageTotal};
use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;

use super::{
    Coverage, Figure, GroupRow, Page, Status, UNKNOWN, count, money, series_request, tokens,
};
use crate::model::daemon::Feed;
use crate::model::work::age;

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

/// Runs that reported no usage, by the harness they ran on, when the series' reports and runs
/// account for exactly the summary's uncovered runs.
fn silent_harnesses(series: &UsageSeries, uncovered: u64) -> Option<String> {
    let reported: BTreeSet<(&str, &str)> = series
        .rows
        .iter()
        .filter_map(|row| Some((row.task.as_deref()?, row.run.as_deref()?)))
        .collect();
    let mut harnesses: BTreeMap<&str, u64> = BTreeMap::new();
    for run in &series.runs {
        if !reported.contains(&(run.task.as_str(), run.id.as_str())) {
            *harnesses.entry(run.harness.as_str()).or_default() += 1;
        }
    }
    (harnesses.values().sum::<u64>() == uncovered).then(|| {
        harnesses
            .iter()
            .map(|(harness, runs)| format!("{harness} {runs}"))
            .collect::<Vec<_>>()
            .join(", ")
    })
}

/// "Covers N of M runs", and why the rest are missing.
fn coverage(summary: &UsageSummary, series: Option<&UsageSeries>) -> Coverage {
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
    let harnesses = series
        .and_then(|series| silent_harnesses(series, rest))
        .map(|list| format!(" ({list})"))
        .unwrap_or_default();
    let other = if rest == 1 {
        "The other run".to_owned()
    } else {
        format!("The other {rest} runs")
    };
    Coverage {
        line: format!(
            "Covers {covered} of {total} runs. {other} ran on a harness that reports no \
             usage{harnesses}, so their tokens and cost are not in these totals."
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

/// The key a run counts under, as the engine keys its usage groups (days in UTC).
fn run_key(by: UsageBy, run: &Run) -> Option<String> {
    match by {
        UsageBy::Profile => Some(run.profile.clone()),
        UsageBy::Model => run.model.clone(),
        UsageBy::Task => Some(run.task.clone()),
        UsageBy::Day => Some(run.started.to_zoned(TimeZone::UTC).date().to_string()),
    }
}

/// Runs and their recorded seconds; an open run counts until now.
#[derive(Default)]
struct Tally {
    runs: u64,
    seconds: u64,
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
        UsageBy::Day => "Day (UTC)",
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

fn rows(by: UsageBy, series: &UsageSeries, now: Timestamp, feed: &Feed) -> Vec<GroupRow> {
    let mut groups: BTreeMap<Option<String>, (Option<&UsageTotal>, Tally)> = BTreeMap::new();
    for group in &series.summary.groups {
        groups.entry(group.key.clone()).or_default().0 = Some(&group.total);
    }
    for run in &series.runs {
        let tally = &mut groups.entry(run_key(by, run)).or_default().1;
        tally.runs += 1;
        let end = run.ended.unwrap_or(now);
        tally.seconds += u64::try_from(end.as_second() - run.started.as_second()).unwrap_or(0);
    }
    let mut ordered: Vec<_> = groups.into_iter().collect();
    match by {
        UsageBy::Day => ordered.reverse(),
        UsageBy::Profile | UsageBy::Model | UsageBy::Task => ordered.sort_by(|a, b| {
            let requests = |group: &(Option<&UsageTotal>, Tally)| group.0.map_or(0, |t| t.requests);
            requests(&b.1)
                .cmp(&requests(&a.1))
                .then(b.1.1.runs.cmp(&a.1.1.runs))
                .then(a.0.cmp(&b.0))
        }),
    }
    ordered
        .into_iter()
        .map(|(key, (total, tally))| {
            let (key, named, detail) = match key {
                Some(key) => {
                    let (words, detail) = key_words(by, &key, feed);
                    (words, true, detail)
                }
                None => (missing(by).to_owned(), false, None),
            };
            let figure =
                |value: fn(&UsageTotal) -> String| total.map_or_else(|| UNKNOWN.into(), value);
            GroupRow {
                key,
                named,
                detail,
                runs: count(tally.runs),
                run_time: if tally.runs == 0 {
                    UNKNOWN.into()
                } else {
                    age(Some(0), tally.seconds)
                },
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
    let now = Timestamp::from_second(i64::try_from(feed.now).unwrap_or(0))
        .unwrap_or(Timestamp::UNIX_EPOCH);
    let cache = feed.detail(&series_request(by));
    let series = cache.and_then(|cache| match cache.body() {
        Some(DetailBody::UsageSeries(series)) => Some(series),
        _ => None,
    });
    // The series and its own summary agree with each other; the topic answers sooner.
    let summary = series.map(|series| &series.summary).or(feed.usage.as_ref());
    let rows = series
        .map(|series| rows(by, series, now, feed))
        .unwrap_or_default();
    let series_error = cache
        .and_then(|cache| cache.error.as_ref())
        .map(|error| error.message.as_str());
    let rows_empty = match (series, series_error) {
        (Some(_), _) if rows.is_empty() => Some("corral has recorded no runs or usage yet."),
        (Some(_), _) => None,
        (None, Some(_)) => Some("The grouped totals could not be read."),
        (None, None) => Some("Loading the grouped totals from corrald…"),
    };
    Page {
        status: status(feed),
        coverage: summary.map(|summary| coverage(summary, series)),
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
    use corral_types::{Role, Usage, UsageGroup};
    use std::path::PathBuf;

    fn run(task: &str, id: &str, profile: &str, harness: &str) -> Run {
        Run {
            id: id.into(),
            task: task.into(),
            role: Role::Worker,
            profile: profile.into(),
            harness: harness.into(),
            model: None,
            effort: None,
            worktree: PathBuf::new(),
            branch: String::new(),
            base: String::new(),
            base_sha: String::new(),
            session: None,
            started: "2026-11-15T09:00:00Z".parse().unwrap(),
            ended: Some("2026-11-15T10:30:00Z".parse().unwrap()),
            outcome: None,
            summary: None,
            head_sha: None,
            quiet_since: None,
        }
    }

    /// The header says what the partial totals cover and names the harnesses of the runs that
    /// reported nothing; a profile with only silent runs still lists its runs with unknown
    /// totals rather than vanishing.
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
        let summary = UsageSummary {
            total: total.clone(),
            groups: vec![UsageGroup {
                key: Some("pi".into()),
                total,
            }],
            covered_runs: 1,
            total_runs: 3,
        };
        let series = UsageSeries {
            summary,
            rows: vec![Usage {
                task: Some("T-1".into()),
                run: Some("R1".into()),
                at: "2026-11-15T09:30:00Z".parse().unwrap(),
                profile: Some("pi".into()),
                model: "m".into(),
                input: 1200,
                output: 300,
                cache_read: 0,
                cache_write: 0,
                usd: Some(0.5),
                billing: None,
            }],
            runs: vec![
                run("T-1", "R1", "pi", "pi"),
                run("T-2", "R1", "claude", "claude-code"),
                run("T-3", "R1", "claude", "claude-code"),
            ],
        };
        let target = series_request(UsageBy::Profile);
        let mut feed = Feed {
            now: 1_763_200_000,
            ..Feed::default()
        };
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
        assert!(coverage.line.contains("claude-code 2"));
        assert!(page.figures[0].detail.is_some());
        let claude = page.rows.iter().find(|row| row.key == "claude").unwrap();
        assert_eq!(claude.runs, "2");
        assert_eq!(claude.requests, UNKNOWN);
    }
}
