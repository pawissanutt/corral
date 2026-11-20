//! Every session as the four places list it: one line per session with the state the engine's
//! `SessionStatus` names (Blocked, Working, Idle, Gone), whatever its role. Home mixes these
//! with tasks; Sessions groups them by state.

use super::roster::{Show, Source, scoped};
use crate::model::status::SessionState;
use crate::model::{Id, ProjectFilter, daemon};

/// One session of any role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLine {
    pub id: Id,
    pub state: SessionState,
    /// `worker`, `reviewer`, `build`, `assistant`.
    pub role: String,
    /// The run's task title, a build's name or a chat's title; empty when the host named none.
    pub name: String,
    /// What it is doing or asking, when the daemon says.
    pub doing: Option<String>,
    pub project: Option<String>,
    /// `<model> · <effort>`, when served.
    pub agent: Option<String>,
    /// The run's task; none for a chat or a build.
    pub task: Option<Id>,
}

/// The state of a roster group: the roster orders needs-you, working, idle, then the two
/// kinds of stopped host, which the redesign names Gone.
fn state_of(source: Source<'_>) -> SessionState {
    match source.state().1 {
        0 => SessionState::Blocked,
        1 => SessionState::Working,
        2 => SessionState::Idle,
        _ => SessionState::Gone,
    }
}

fn line(source: Source<'_>, feed: &daemon::Feed) -> SessionLine {
    SessionLine {
        id: Id::new(source.id),
        state: state_of(source),
        role: source.role().to_owned(),
        name: source.name(feed).unwrap_or_default(),
        doing: source.doing(feed),
        project: source.project(feed).map(str::to_owned),
        agent: source.agent(),
        task: source.run.map(|row| Id::new(&row.run.task)),
    }
}

/// Every session in the project scope, Gone ones included, ordered by state and then by its
/// newest activity.
pub fn session_lines(feed: &daemon::Feed, filter: &ProjectFilter) -> Vec<SessionLine> {
    scoped(feed, filter, Show::All)
        .into_iter()
        .map(|source| line(source, feed))
        .collect()
}

/// One session's line, whatever the scope; none when the daemon lists no such session.
pub fn session_line(feed: &daemon::Feed, session: &str) -> Option<SessionLine> {
    scoped(feed, &ProjectFilter::All, Show::All)
        .into_iter()
        .find(|source| source.id == session)
        .map(|source| line(source, feed))
}

#[cfg(test)]
mod tests {
    use corral_types::{Role, SessionStatus};

    use super::super::roster_tests::agent;
    use super::*;

    /// A run whose host waits on an answer is Blocked like a task is: Home's Needs you is
    /// built from that word, so a session filed under another state would never surface.
    #[test]
    fn a_run_waiting_on_an_answer_is_blocked_and_a_lost_host_is_gone() {
        let mut feed = daemon::Feed::default();
        let mut waiting = agent("T-1", "R1", Role::Worker);
        waiting.status = SessionStatus::Blocked {
            excerpt: "Overwrite?".into(),
        };
        feed.agents.insert(Id::from("T-1_R1"), waiting);
        let mut lost = agent("T-2", "R1", Role::Worker);
        lost.status = SessionStatus::Gone;
        feed.agents.insert(Id::from("T-2_R1"), lost);
        let states: Vec<_> = session_lines(&feed, &ProjectFilter::All)
            .into_iter()
            .map(|line| (line.id.to_string(), line.state))
            .collect();
        assert_eq!(
            states,
            [
                ("T-1_R1".to_owned(), SessionState::Blocked),
                ("T-2_R1".to_owned(), SessionState::Gone)
            ]
        );
        assert_eq!(
            session_line(&feed, "T-2_R1").map(|line| line.state),
            Some(SessionState::Gone)
        );
        assert_eq!(session_line(&feed, "A-9"), None);
    }
}
