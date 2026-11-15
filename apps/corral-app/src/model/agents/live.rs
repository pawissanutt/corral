//! The Agents wall from corrald's agent rows: the PM, workers, planners, reviewers and gates, and
//! any assistant session that is not already in the assistant blocks. Every fact comes from the
//! row; what the daemon does not report is said to be unknown, never filled in.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use corral_protocol::board::CostBasis;
use corral_protocol::rows::{self, AgentRow};

use crate::model::daemon::{self, Link};
use crate::model::effect::Env;
use crate::model::{Hue, Id};

use super::{
    Action, AgentStatus, Feed, Fill, Line, LineClass, Note, State, Tile, Wall, line, tail_line,
};

/// Tail lines a tile shows.
const TILE_TAIL: usize = 4;

/// The wall's state as the protocol reports it.
pub(super) fn status(status: rows::AgentStatus) -> AgentStatus {
    match status {
        rows::AgentStatus::Live => AgentStatus::Live,
        rows::AgentStatus::Working => AgentStatus::Working,
        rows::AgentStatus::Asking => AgentStatus::Asking,
        rows::AgentStatus::Gating => AgentStatus::Gating,
        rows::AgentStatus::Reviewing => AgentStatus::Reviewing,
        rows::AgentStatus::Idle => AgentStatus::Idle,
        rows::AgentStatus::Waiting => AgentStatus::Waiting,
        rows::AgentStatus::Paused => AgentStatus::Paused,
        rows::AgentStatus::Ended => AgentStatus::Ended,
        rows::AgentStatus::Unavailable => AgentStatus::Unavailable,
        rows::AgentStatus::Other => AgentStatus::Unknown,
    }
}

/// A role as people say it: `pm` is "PM", `plan_reviewer` "plan reviewer".
pub(super) fn role_label(role: &str) -> String {
    if role.eq_ignore_ascii_case("pm") {
        "PM".into()
    } else {
        role.replace('_', " ")
    }
}

/// Wall order: the PM, then those producing work, then those checking it, then the rest.
fn rank(role: &str) -> u8 {
    match role {
        "pm" => 0,
        "worker" => 1,
        "planner" => 2,
        "reviewer" => 3,
        "plan_reviewer" => 4,
        "gate" => 5,
        "assistant" => 6,
        _ => 7,
    }
}

/// Task ids in number order, then everything else by name.
fn by_id(a: &str, b: &str) -> Ordering {
    let number = |id: &str| id.strip_prefix("T-").and_then(|n| n.parse::<u64>().ok());
    match (number(a), number(b)) {
        (Some(x), Some(y)) => x.cmp(&y).then_with(|| a.cmp(b)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a.cmp(b),
    }
}

/// The id a tile shows: the task the session works on, else the session itself.
pub(super) fn shown_id(row: &AgentRow) -> String {
    row.task.clone().unwrap_or_else(|| row.id.clone())
}

/// Unix seconds of an RFC 3339 time.
pub(super) fn unix(at: &str) -> Option<u64> {
    let stamp: jiff::Timestamp = at.parse().ok()?;
    u64::try_from(stamp.as_second()).ok()
}

/// This device's offset from UTC in minutes, read off the feed's own clock (`now` in UTC and
/// `clock` in local time), so the model stays free of time-zone lookups.
fn offset(feed: &daemon::Feed) -> Option<i64> {
    if feed.now == 0 {
        return None;
    }
    let (hours, minutes) = feed.clock.split_once(':')?;
    let local = hours.trim().parse::<i64>().ok()? * 60 + minutes.trim().parse::<i64>().ok()?;
    let utc = i64::try_from(feed.now / 60).ok()?.rem_euclid(1440);
    let mut delta = (local - utc).rem_euclid(1440);
    if delta > 14 * 60 {
        delta -= 1440;
    }
    // Real offsets are whole quarter hours; the two clocks may straddle a minute.
    Some(((delta as f64) / 15.0).round() as i64 * 15)
}

/// A daemon time as the design writes it: the local clock (`21:40`) today, its age (`3d ago`)
/// when older; absent when the daemon sent none or one this build cannot read.
pub(super) fn clock(at: Option<&str>, feed: &daemon::Feed) -> Option<String> {
    let when = unix(at?)?;
    let offset = offset(feed)?;
    if feed.now >= when + 20 * 3600 {
        return Some(format!("{} ago", daemon::age(Some(when), feed.now)));
    }
    let minutes = (i64::try_from(when / 60).ok()? + offset).rem_euclid(1440);
    Some(format!("{:02}:{:02}", minutes / 60, minutes % 60))
}

/// Cost and tokens as reported (`$0.42 · 81k tokens`); estimated costs carry `~`.
pub(super) fn usage(
    cost: Option<f64>,
    basis: Option<CostBasis>,
    tokens: Option<u64>,
) -> Option<String> {
    let cost = cost.map(|usd| {
        let mark = match basis {
            Some(CostBasis::Reported) => "",
            _ => "~",
        };
        format!("{mark}${usd:.2}")
    });
    let tokens = tokens.map(|n| match n {
        0..1_000 => format!("{n} tokens"),
        1_000..1_000_000 => format!("{}k tokens", n / 1_000),
        _ => format!("{:.1}M tokens", n as f64 / 1_000_000.0),
    });
    let parts: Vec<String> = cost.into_iter().chain(tokens).collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// Whether the agents shown are the current ones, or the last known while the link is down.
pub(super) fn stale(feed: &daemon::Feed, topic: &str) -> bool {
    feed.link != Link::Up || feed.topics.get(topic).is_some_and(|state| state.stale)
}

/// The rows the wall shows, in wall order. Assistant sessions served as assistant rows are in
/// the assistant blocks already.
fn shown(feed: &daemon::Feed) -> Vec<&AgentRow> {
    let mut rows: Vec<&AgentRow> = feed
        .agents
        .values()
        .filter(|row| !(row.role == "assistant" && feed.assistants.contains_key(row.id.as_str())))
        .collect();
    rows.sort_by(|a, b| {
        rank(&a.role)
            .cmp(&rank(&b.role))
            .then_with(|| by_id(&shown_id(a), &shown_id(b)))
            .then_with(|| a.id.cmp(&b.id))
    });
    rows
}

fn tile(row: &AgentRow, feed: &daemon::Feed, stale: bool) -> Tile {
    let state = status(row.status);
    let (chip, hue) = state.chip();
    let since = clock(row.since.as_deref(), feed);
    let (feed_kind, feed_label) = if stale {
        let synced = feed.synced.clone().unwrap_or_else(|| "unknown".into());
        (Feed::Snapshot, format!("as of {synced}"))
    } else {
        (Feed::Live, "latest".to_owned())
    };
    let tail: Vec<Line> = if row.tail.is_empty() {
        vec![line("no output recorded", LineClass::Dim)]
    } else {
        row.tail[row.tail.len().saturating_sub(TILE_TAIL)..]
            .iter()
            .map(|text| tail_line(text))
            .collect()
    };
    Tile {
        id: row.id.clone(),
        label: shown_id(row),
        role: role_label(&row.role),
        title: row.title.clone(),
        doing: if row.now.is_empty() && state == AgentStatus::Paused {
            "paused by you · automated input waits".into()
        } else {
            row.now.clone()
        },
        profile: row
            .profile
            .clone()
            .unwrap_or_else(|| "profile unknown".into()),
        runtime: row
            .runtime
            .clone()
            .unwrap_or_else(|| "runtime unknown".into()),
        pane: row.pane.clone().unwrap_or_else(|| "no pane".into()),
        usage: usage(row.cost_usd, row.cost_basis, row.tokens),
        feed: feed_kind,
        feed_short: feed_label.clone(),
        feed_label,
        since: since
            .as_ref()
            .map(|at| format!("since {at}"))
            .unwrap_or_default(),
        since_short: since.unwrap_or_else(|| "—".into()),
        chip,
        hue,
        doing_hue: if state == AgentStatus::Asking {
            Hue::You
        } else {
            Hue::Neutral
        },
        tail,
        action: Action::OpenSession(Id::from(row.id.as_str())),
    }
}

/// `herdr-core · 6 agents · Herdr TUI (legacy) · 1 agent`: agents per runtime, as reported.
fn runtimes(rows: &[&AgentRow]) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for row in rows {
        *counts
            .entry(row.runtime.as_deref().unwrap_or("runtime unknown"))
            .or_default() += 1;
    }
    counts
        .into_iter()
        .map(|(runtime, n)| format!("{runtime} · {n} agent{}", if n == 1 { "" } else { "s" }))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Build the wall and phone rows from the daemon's agent rows.
pub fn wall(state: &State, feed: &daemon::Feed, env: Env) -> Wall {
    let rows = shown(feed);
    let stale = stale(feed, "agents");
    let topic = feed.topics.get("agents");
    let mut machines: Vec<&str> = rows
        .iter()
        .filter_map(|row| row.machine.as_deref())
        .collect();
    machines.sort_unstable();
    machines.dedup();
    let machine = match machines.as_slice() {
        [] => "Machine unknown".to_owned(),
        [one] => (*one).to_owned(),
        many => format!("{} machines", many.len()),
    };
    let detail = match machines.as_slice() {
        [_, _, ..] => format!("{} · {}", machines.join(", "), runtimes(&rows)),
        _ => runtimes(&rows),
    };
    let fill = match (rows.is_empty(), topic) {
        (false, _) => Fill::Ready,
        (true, Some(topic)) if topic.error.is_some() && !topic.loading => Fill::Failed(
            topic
                .error
                .as_ref()
                .map(|error| {
                    format!(
                        "corrald could not list the agents ({}): {}. The list fills in when it answers.",
                        error.code, error.message
                    )
                })
                .unwrap_or_default(),
        ),
        (true, None) => Fill::Loading,
        (true, Some(topic)) if topic.loading => Fill::Loading,
        (true, Some(_)) => Fill::Empty {
            heading: "No agents running",
            paragraph: "The PM, workers, reviewers and gates appear here while corrald runs them."
                .into(),
        },
    };
    let stale_note = (stale && !rows.is_empty()).then(|| {
        format!(
            "Showing the agents as of {}; corrald is not answering, so states and tails may have moved on.",
            feed.synced.as_deref().unwrap_or("the last update")
        )
    });
    Wall {
        title: "Agents",
        subtitle: "the quick thread first, then the PM, workers, gates and assistant sessions · what each is doing now, and where",
        machine,
        phone_machine_detail: detail.clone(),
        machine_detail: detail,
        machine_note: None,
        other_machines: "Other machines join over SSH later (Settings › Machines)",
        phone_machine_note: None,
        count: rows.len(),
        tiles: rows.iter().map(|row| tile(row, feed, stale)).collect(),
        fill,
        stale: stale_note,
        footer: Note::Plain(
            "Each tail is the last output corrald recorded for that agent; open one for its transcript."
                .into(),
        ),
        mode: state.mode,
        mode_labels: ["Wall", "List"],
        launch: super::controls::launch(state, feed, env),
    }
}

#[cfg(test)]
mod tests {
    use super::clock;
    use crate::model::daemon::Feed;

    #[test]
    fn daemon_times_show_on_the_device_clock_not_utc() {
        // 2023-11-14T22:13:20Z on a device one hour east of UTC.
        let feed = Feed {
            now: 1_700_000_000,
            clock: "23:13".into(),
            ..Feed::default()
        };
        assert_eq!(
            clock(Some("2023-11-14T22:00:00Z"), &feed).as_deref(),
            Some("23:00")
        );
    }
}
