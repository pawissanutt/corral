//! The dashboard's numbers and panels: the six tiles, tasks by status, the projects (or the
//! scoped project's facts) and today's cost. Each figure is what corrald serves; a figure it
//! does not serve reads as unknown, never as zero.

use corral_protocol::detail::{DetailBody, DetailRef};
use corral_types::{Status, UsageBy, UsageSummary};

use super::{Since, needs_you, task_needs_you, task_working, working};
use crate::model::Id;
use crate::model::agents::session_lines;
use crate::model::daemon::Feed;
use crate::model::nav::Destination;
use crate::model::projects::Scope;
use crate::model::status::{OPEN, SessionState, status_word};
use crate::model::tasks::{self, Board};
use crate::model::usage::{self, Period};

/// Stands in for a figure corrald does not report.
const UNKNOWN: &str = "—";

/// One number at the top of the dashboard.
#[derive(Debug, Clone, PartialEq)]
pub struct Tile {
    /// `stat-<key>`.
    pub key: &'static str,
    pub label: String,
    pub value: String,
    /// The line under the number.
    pub sub: String,
    /// The sentence behind the number.
    pub hint: String,
    /// Something waits on the owner: drawn in the needs-you hue.
    pub you: bool,
    /// The page holding the detail; none for the cards beside it.
    pub go: Option<Destination>,
    /// Slots in use of the limit, drawn as a meter.
    pub meter: Option<(u64, u64)>,
    /// Bars of the last seven days, against the busiest.
    pub spark: Vec<f32>,
}

/// The six tiles: needs you, working with the slots, ready, sessions by state, landed since
/// the last visit, cost today. Needs you, Working and Ready count tasks, so they are unknown
/// until the board is complete.
pub fn tiles(feed: &Feed, scope: &Scope, since: &Since, cost: &Cost) -> Vec<Tile> {
    let unknown = match tasks::board(feed) {
        Board::Complete => None,
        Board::Loading => Some("tasks loading"),
        Board::Unavailable => Some("tasks unavailable"),
    };
    let count = |count: usize| {
        if unknown.is_some() {
            UNKNOWN.to_owned()
        } else {
            count.to_string()
        }
    };
    let need = needs_you(feed, scope);
    let need_tasks = need
        .iter()
        .filter(|id| feed.board.task(id).is_some())
        .count();
    let slots = slots(feed, scope);
    let ready: Vec<_> = feed
        .board
        .tasks()
        .iter()
        .filter(|task| scope.matches(task.project()) && task.status() == Status::Ready)
        .collect();
    let held = ready
        .iter()
        .filter(|task| {
            task.row.dependencies.as_ref().is_some_and(|dependencies| {
                dependencies
                    .iter()
                    .any(|dependency| dependency.status != Some(Status::Done))
            })
        })
        .count();
    let free = slots.map(|(used, limit)| limit.saturating_sub(used));
    let mut ready_sub = vec![match free {
        Some(0) => "no slot free".to_owned(),
        Some(1) => "1 slot free".to_owned(),
        Some(free) => format!("{free} slots free"),
        None => "slots unknown".to_owned(),
    }];
    if held > 0 {
        ready_sub.push(format!("{held} held by a dependency"));
    }
    let sessions = session_lines(feed, &scope.filter());
    let by = |state| sessions.iter().filter(|line| line.state == state).count();
    let at = since.at.clone();
    let tile = |key, label: String, value: String, sub: String, hint: String| Tile {
        key,
        label,
        value,
        sub,
        hint,
        you: false,
        go: None,
        meter: None,
        spark: Vec::new(),
    };
    vec![
        Tile {
            you: unknown.is_none() && !need.is_empty(),
            ..tile(
                "needs",
                "Needs you".into(),
                count(need.len()),
                format!(
                    "{} · {} sessions",
                    unknown.map_or_else(|| format!("{need_tasks} tasks"), str::to_owned),
                    need.len() - need_tasks
                ),
                "Blocked tasks, reviews ready to land and sessions waiting on an answer".into(),
            )
        },
        Tile {
            go: Some(Destination::Timeline),
            meter: slots,
            ..tile(
                "working",
                "Working".into(),
                count(working(feed, scope).len()),
                slots.map_or_else(
                    || "slots unknown".to_owned(),
                    |(used, limit)| format!("slots {used}/{limit}"),
                ),
                "Running, Blocked and Review tasks hold a slot, even while waiting to land. \
                 Working counts running tasks, reviews being checked and working sessions."
                    .into(),
            )
        },
        Tile {
            go: Some(Destination::Tasks),
            ..tile(
                "ready",
                "Ready".into(),
                count(ready.len()),
                unknown.map_or_else(|| ready_sub.join(" · "), str::to_owned),
                "Ready tasks wait for a dispatch and a free slot".into(),
            )
        },
        Tile {
            go: Some(Destination::Sessions),
            ..tile(
                "sessions",
                "Sessions".into(),
                sessions
                    .iter()
                    .filter(|line| line.state != SessionState::Gone)
                    .count()
                    .to_string(),
                [
                    SessionState::Blocked,
                    SessionState::Working,
                    SessionState::Idle,
                    SessionState::Unavailable,
                ]
                .into_iter()
                .map(|state| format!("{} {}", by(state), state.word().to_lowercase()))
                .collect::<Vec<_>>()
                .join(" · "),
                "Every session whose host has not gone, by state".into(),
            )
        },
        Tile {
            go: Some(Destination::Feed),
            ..tile(
                "landed",
                at.as_ref().map_or_else(
                    || "Landed since last visit".to_owned(),
                    |at| format!("Landed since {at}"),
                ),
                since.landed.to_string(),
                format!(
                    "{} dropped · {} created · {} forced",
                    since.dropped, since.created, since.forced
                ),
                "Counted from the events this device has loaded".into(),
            )
        },
        Tile {
            go: Some(Destination::Usage),
            spark: cost.week.clone(),
            ..tile(
                "cost",
                "Cost today".into(),
                cost.today.clone(),
                if cost.week.is_empty() {
                    cost.note.clone()
                } else {
                    "last 7 days".into()
                },
                cost.note.clone(),
            )
        },
    ]
}

/// The running slots in use of the limit: a project with its own limit counts its own, else
/// the workspace's; none while corrald does not report them.
fn slots(feed: &Feed, scope: &Scope) -> Option<(u64, u64)> {
    let slots = feed.health.as_ref()?.slots.as_ref()?;
    let count = scope
        .project()
        .and_then(|project| slots.projects.get(project.as_str()))
        .unwrap_or(&slots.global);
    Some((count.used, count.limit))
}

/// One open status in the Tasks by status panel.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusBar {
    pub status: Status,
    pub word: &'static str,
    pub count: usize,
    /// Against the largest count.
    pub fraction: f32,
}

/// The open tasks in scope by status, in the engine's order, and how many are open; until the
/// board is complete, what the panel says instead.
pub fn by_status(feed: &Feed, scope: &Scope) -> Result<(usize, Vec<StatusBar>), &'static str> {
    if let Some(unknown) = super::unknown(feed) {
        return Err(unknown);
    }
    let count = |status| {
        feed.board
            .tasks()
            .iter()
            .filter(|task| scope.matches(task.project()) && task.status() == status)
            .count()
    };
    let counts: Vec<(Status, usize)> = OPEN.iter().map(|&status| (status, count(status))).collect();
    let most = counts.iter().map(|(_, n)| *n).max().unwrap_or(0).max(1);
    let open = counts.iter().map(|(_, n)| n).sum();
    Ok((
        open,
        counts
            .into_iter()
            .map(|(status, count)| StatusBar {
                status,
                word: status_word(status),
                count,
                fraction: count as f32 / most as f32,
            })
            .collect(),
    ))
}

/// One project in the Projects panel: pressing it scopes every page to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectLine {
    pub id: Id,
    /// What waits on the owner, what works, and the open tasks as corrald counts them.
    pub you: usize,
    pub working: usize,
    pub open: u64,
}

/// Every project corrald lists, with what waits and works in it.
pub fn projects(feed: &Feed) -> Vec<ProjectLine> {
    feed.projects
        .iter()
        .map(|(id, row)| {
            let mine = || {
                feed.board
                    .tasks()
                    .iter()
                    .filter(move |task| task.project() == Some(id.as_str()))
            };
            ProjectLine {
                id: id.clone(),
                you: mine().filter(|task| task_needs_you(task)).count(),
                working: mine().filter(|task| task_working(task)).count(),
                open: row.open,
            }
        })
        .collect()
}

/// The scoped project's facts as the engine resolves them: base, gate, reviewer, profile and
/// the two automation switches; unknown where corrald does not resolve one.
pub fn project_facts(feed: &Feed, id: &Id) -> Vec<(&'static str, String)> {
    let settings = feed.projects.get(id).and_then(|row| row.effective.as_ref());
    let text = |value: Option<&String>| value.cloned().unwrap_or_else(|| "unknown".into());
    let switch = |value: Option<bool>| {
        value
            .map_or("unknown", |on| if on { "on" } else { "off" })
            .to_owned()
    };
    vec![
        ("base", text(settings.and_then(|s| s.base.as_ref()))),
        (
            "gate",
            settings.map_or_else(
                || "unknown".into(),
                |s| {
                    if s.gate.is_empty() {
                        "none".into()
                    } else {
                        s.gate.join(" && ")
                    }
                },
            ),
        ),
        ("reviewer", text(settings.and_then(|s| s.reviewer.as_ref()))),
        ("profile", text(settings.and_then(|s| s.profile.as_ref()))),
        ("auto dispatch", switch(settings.map(|s| s.auto_dispatch))),
        ("auto land", switch(settings.map(|s| s.auto_land))),
    ]
}

/// One profile's cost today.
#[derive(Debug, Clone, PartialEq)]
pub struct CostRow {
    pub key: String,
    pub cost: String,
    pub fraction: f32,
}

/// Today's cost as the agent hosts reported it, for every project (the usage series is not
/// read per project).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Cost {
    /// `$4.20`, or unknown when nothing reported a price today.
    pub today: String,
    /// What the figure covers, or why there is none.
    pub note: String,
    /// Today by profile, the costliest first.
    pub rows: Vec<CostRow>,
    /// Said in place of the rows.
    pub empty: Option<&'static str>,
    /// The last seven days' cost, against the costliest; empty until loaded.
    pub week: Vec<f32>,
}

/// The usage series Home reads: the last seven local days by profile; none before the clock
/// is known.
pub fn cost_request(feed: &Feed) -> Option<DetailRef> {
    if feed.now == 0 {
        return None;
    }
    let state = usage::State {
        period: Period::Week,
        by: UsageBy::Profile,
        ..usage::State::default()
    };
    Some(usage::series_request(&state, &usage::local(feed)))
}

/// Dollars: whole above ten (`$12`), cents below (`$7.60`).
fn money(value: f64) -> String {
    if value >= 10.0 {
        format!("${:.0}", value)
    } else {
        format!("${value:.2}")
    }
}

/// Today's cost from the loaded series.
pub fn cost(feed: &Feed) -> Cost {
    let series = cost_request(feed)
        .and_then(|request| feed.detail(&request))
        .and_then(|cache| match cache.body() {
            Some(DetailBody::UsageSeries(series)) => Some(series),
            _ => None,
        });
    let Some(series) = series else {
        return Cost {
            today: UNKNOWN.into(),
            note: "usage not loaded".into(),
            empty: Some("Usage is not loaded."),
            ..Cost::default()
        };
    };
    let today = usage::local(feed).today.to_string();
    let empty = UsageSummary::default();
    let summary = series
        .days
        .iter()
        .find(|day| day.day == today)
        .map_or(&empty, |day| &day.summary);
    let mut rows: Vec<(String, f64)> = summary
        .groups
        .iter()
        .filter_map(|group| {
            let usd = group.total.usd.filter(|usd| *usd > 0.0)?;
            Some((
                group.key.clone().unwrap_or_else(|| "no profile".into()),
                usd,
            ))
        })
        .collect();
    rows.sort_by(|a, b| b.1.total_cmp(&a.1));
    let most = rows.first().map_or(1.0, |row| row.1).max(f64::EPSILON);
    let week: Vec<f64> = series
        .days
        .iter()
        .map(|day| day.summary.total.usd.unwrap_or(0.0))
        .collect();
    let busiest = week.iter().copied().fold(0.0, f64::max).max(f64::EPSILON);
    Cost {
        today: summary.total.usd.map_or_else(|| UNKNOWN.into(), money),
        note: match summary.total.usd {
            Some(_) => "reported by the agent hosts today, all projects".into(),
            None if summary.total.requests > 0 => "no request reported a price today".into(),
            None => "no usage reported today".into(),
        },
        rows: rows
            .into_iter()
            .map(|(key, usd)| CostRow {
                key,
                cost: money(usd),
                fraction: (usd / most) as f32,
            })
            .collect(),
        empty: (summary.total.usd.is_none()).then_some("No cost recorded today."),
        week: week.into_iter().map(|usd| (usd / busiest) as f32).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A workspace whose hosts report no prices showed a cost of $0.00 today, as if the agents
    /// had cost nothing: a figure corrald does not hold reads as unknown.
    #[test]
    fn cost_today_without_a_reported_price_is_unknown_not_zero() {
        let feed = Feed::default();
        assert_eq!(cost(&feed).today, UNKNOWN);
        let since = Since::default();
        let tiles = tiles(&feed, &Scope::All, &since, &cost(&feed));
        let cost_tile = tiles
            .iter()
            .find(|tile| tile.key == "cost")
            .expect("cost tile");
        assert_eq!(cost_tile.value, UNKNOWN);
        let working = tiles
            .iter()
            .find(|tile| tile.key == "working")
            .expect("tile");
        assert_eq!(working.meter, None);
    }
}
