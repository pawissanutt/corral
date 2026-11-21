//! Every session as the four places list it: one line per session, including live hosts whose
//! status could not be read. Home mixes these
//! with tasks; Sessions groups them by state.

use super::source::{Source, scoped};
use crate::model::session::Asked;
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
    /// What Answer opens while the session is Blocked on its Pi host's card; none for a
    /// session of another harness, whose question is its terminal's, and for a host whose state
    /// briefs no owner question.
    pub asked: Option<Asked>,
}

fn line(source: Source<'_>, feed: &daemon::Feed) -> SessionLine {
    let state = source.state();
    SessionLine {
        id: Id::new(source.id),
        state,
        role: source.role().to_owned(),
        name: source.name(feed).unwrap_or_default(),
        doing: source.doing(feed),
        project: source.project(feed).map(str::to_owned),
        agent: source.agent(),
        task: source.run.map(|row| Id::new(&row.run.task)),
        asked: (state == SessionState::Blocked)
            .then(|| source.asked(feed))
            .flatten(),
    }
}

/// Every session in the project scope, Gone ones included, ordered by state and then by its
/// newest activity.
pub fn session_lines(feed: &daemon::Feed, filter: &ProjectFilter) -> Vec<SessionLine> {
    scoped(feed, filter)
        .into_iter()
        .map(|source| line(source, feed))
        .collect()
}

/// One session's line, whatever the scope; none when the daemon lists no such session.
pub fn session_line(feed: &daemon::Feed, session: &str) -> Option<SessionLine> {
    scoped(feed, &ProjectFilter::All)
        .into_iter()
        .find(|source| source.id == session)
        .map(|source| line(source, feed))
}

/// Whether session `id` is a build stopped here, whose worktree, branch and record Remove…
/// deletes after prune's dry run.
pub fn removable(feed: &daemon::Feed, session: &str) -> bool {
    scoped(feed, &ProjectFilter::All)
        .into_iter()
        .find(|source| source.id == session)
        .is_some_and(|source| source.removable())
}

#[cfg(test)]
mod tests {
    use corral_types::{Role, SessionStatus};

    use super::super::source_tests::agent;
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
