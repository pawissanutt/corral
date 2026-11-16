//! Facts read from corrald's agent rows for the roster, its panel and the session page: the
//! state, the role, the id shown, times, usage and staleness. Every fact comes from the row; what
//! the daemon does not report is said to be unknown, never filled in.

use corral_protocol::board::CostBasis;
use corral_protocol::rows::{self, AgentRow};

use crate::model::daemon::{self, Link};

use super::AgentStatus;

/// The session's state as the protocol reports it.
pub(crate) fn status(status: rows::AgentStatus) -> AgentStatus {
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

/// The id a row shows: the task the session works on, else the session itself.
pub(crate) fn shown_id(row: &AgentRow) -> String {
    row.task.clone().unwrap_or_else(|| row.id.clone())
}

/// Unix seconds of an RFC 3339 time.
pub(super) fn unix(at: &str) -> Option<u64> {
    let stamp: jiff::Timestamp = at.parse().ok()?;
    u64::try_from(stamp.as_second()).ok()
}

/// A daemon time as the design writes it: the local clock (`21:40`) today, with its date
/// (`Nov 14 20:53`) when it fell on an earlier day, its age (`3d ago`) when older than 20 hours;
/// absent before the first clock tick or when the daemon sent none this build can read.
pub(super) fn clock(at: Option<&str>, feed: &daemon::Feed) -> Option<String> {
    let at = at?;
    let when = unix(at)?;
    if feed.now == 0 {
        return None;
    }
    if feed.now >= when + 20 * 3600 {
        return Some(format!("{} ago", daemon::age(Some(when), feed.now)));
    }
    Some(crate::model::work::when(at, feed)).filter(|shown| !shown.is_empty())
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

#[cfg(test)]
mod tests {
    use super::clock;
    use crate::model::daemon::Feed;

    #[test]
    fn daemon_times_show_on_the_device_clock_not_utc() {
        // 2023-11-14T22:13:20Z on a device one hour east of UTC.
        let feed = Feed {
            now: 1_700_000_000,
            date: "2023-11-14".into(),
            clock: "23:13".into(),
            ..Feed::default()
        };
        assert_eq!(
            clock(Some("2023-11-14T22:00:00Z"), &feed).as_deref(),
            Some("23:00")
        );
    }

    #[test]
    fn a_time_from_an_earlier_day_carries_its_date() {
        // 05:00 UTC on Nov 14; the transcript line is from 20:53 the evening before.
        let feed = Feed {
            now: 1_699_938_000,
            date: "2023-11-14".into(),
            clock: "05:00".into(),
            ..Feed::default()
        };
        assert_eq!(
            clock(Some("2023-11-13T20:53:00Z"), &feed).as_deref(),
            Some("Nov 13 20:53")
        );
    }
}
