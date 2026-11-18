//! Facts read from corrald's live runs (`AgentRow { run, status }`) for the roster, its panel and
//! the session page: the state, the role, the id shown, times and staleness. Every fact comes from
//! the row; what the daemon does not report is said to be unknown, never filled in.

use corral_protocol::rows::AgentRow;
use corral_types::{SessionRef, SessionStatus};

use crate::model::daemon::{self, Link};

use super::AgentStatus;

/// The session's state as the runner last observed it.
pub(crate) fn status(status: &SessionStatus) -> AgentStatus {
    match status {
        SessionStatus::Working => AgentStatus::Working,
        SessionStatus::Blocked { .. } => AgentStatus::Blocked,
        SessionStatus::Idle => AgentStatus::Idle,
        SessionStatus::Gone => AgentStatus::Gone,
    }
}

/// What a blocked session's screen shows it waiting on, when the runner captured it.
pub(super) fn excerpt(row: &AgentRow) -> Option<&str> {
    match &row.status {
        SessionStatus::Blocked { excerpt } if !excerpt.trim().is_empty() => Some(excerpt),
        _ => None,
    }
}

/// A role as people say it: `plan_reviewer` is "plan reviewer".
pub(super) fn role_label(role: &str) -> String {
    role.replace('_', " ")
}

/// The id a row shows: the task the run works on.
pub(crate) fn shown_id(row: &AgentRow) -> String {
    row.run.task.clone()
}

/// Where a session runs: a pi session's placement is the pane or process its host runs in.
pub(crate) fn placement(session: &SessionRef) -> &SessionRef {
    match session {
        SessionRef::Pi { inner, .. } => placement(inner),
        other => other,
    }
}

/// Whether the run's host speaks the pi session socket (cancel, pause, rewind, live text).
pub(crate) fn is_pi(row: &AgentRow) -> bool {
    matches!(row.run.session, Some(SessionRef::Pi { .. }))
}

/// The run's pane (`w1:p2`) or process (`pid 4120`), when the runner recorded one.
pub(super) fn session_name(row: &AgentRow) -> Option<String> {
    row.run
        .session
        .as_ref()
        .map(|session| match placement(session) {
            SessionRef::Herdr { pane, .. } => pane.clone(),
            SessionRef::Process { pid, .. } => format!("pid {pid}"),
            SessionRef::Pi { .. } => String::new(),
        })
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

/// [`clock`] of an engine timestamp.
pub(super) fn clock_at(at: jiff::Timestamp, feed: &daemon::Feed) -> Option<String> {
    clock(Some(&at.to_string()), feed)
}

/// How long the session has shown no new output (`quiet 12m`), from the runner's `quiet_since`;
/// absent while it keeps writing, or before the first clock tick.
pub(super) fn quiet(row: &AgentRow, feed: &daemon::Feed) -> Option<String> {
    let since = u64::try_from(row.run.quiet_since?.as_second()).ok()?;
    (feed.now > 0 && since <= feed.now)
        .then(|| format!("quiet {}", daemon::age(Some(since), feed.now)))
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
