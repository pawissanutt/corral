//! Sessions: one list of every session in scope, grouped by state in the order Blocked,
//! Working, Idle, with Gone folded at the bottom; each row's facts and its one next step, the
//! columns they sit in, the head's summary and the inspector of the selected session. The
//! session page itself is `model::session`.

mod inspect;
mod row;

use std::collections::BTreeSet;

use crate::model::Id;
use crate::model::agents::session_lines;
use crate::model::daemon::Feed;
use crate::model::list::{Column, Section};
use crate::model::projects::Scope;
use crate::model::status::{SessionState, session_look};

pub use inspect::{Inspect, Latest, inspect, latest};
pub use row::{SessionRow, row, summary};

pub const TITLE: &str = "Sessions";
pub const EMPTY: &str = "No sessions in this scope. Start a chat or a build with New.";
pub const LOADING: &str = "Reading sessions…";

/// The list's own state: which of the folding sections are open.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    pub unfolded: BTreeSet<String>,
}

impl State {
    /// Opens or closes a folding section.
    pub fn toggle(&mut self, key: &str) {
        if !self.unfolded.remove(key) {
            self.unfolded.insert(key.to_owned());
        }
    }
}

/// The list's columns. As the list narrows the model leaves first, then the project, the
/// context share, the cost, the age, the role and what it does now; the state, id, name and
/// next step always show.
pub fn columns() -> Vec<Column> {
    vec![
        Column::fixed("state", "", 14.0, 1.0),
        Column::fixed("id", "ID", 78.0, 10.0),
        Column::flex("name", "Session", 140.0, 14.0, 1.2),
        Column::fixed("role", "Role", 64.0, 9.0).gives_way(6),
        Column::flex("now", "Now", 110.0, 14.0, 1.0).gives_way(7),
        Column::fixed("project", "Project", 96.0, 12.0).gives_way(2),
        Column::fixed("model", "Model", 118.0, 16.0).gives_way(1),
        Column::fixed("context", "Ctx", 40.0, 4.0)
            .end()
            .gives_way(3),
        Column::fixed("cost", "Cost", 56.0, 7.0).end().gives_way(4),
        Column::fixed("age", "Age", 40.0, 5.0).end().gives_way(5),
        Column::fixed("next", "Next step", 112.0, 13.0)
            .end()
            .short("Next"),
    ]
}

/// The list's sections: a group per state that has a session, Gone folding.
pub fn sections(feed: &Feed, scope: &Scope, state: &State) -> Vec<Section> {
    let lines = session_lines(feed, &scope.filter());
    SessionState::ORDER
        .into_iter()
        .filter_map(|group| {
            let rows: Vec<Id> = lines
                .iter()
                .filter(|line| line.state == group)
                .map(|line| line.id.clone())
                .collect();
            let open = state.unfolded.contains(group.slug());
            let section = Section {
                key: group.slug().to_owned(),
                label: group.word().to_owned(),
                look: Some(session_look(group)),
                count: rows.len(),
                note: group.note().map(str::to_owned),
                fold: group.folds().then_some(open),
                rows: if group.folds() && !open {
                    Vec::new()
                } else {
                    rows
                },
            };
            section.shown().then_some(section)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use corral_types::{Role, SessionStatus};

    use super::*;
    use crate::model::agents::source_tests::agent;

    /// A stopped host listed among the live ones buried the session that waits on the owner.
    #[test]
    fn gone_sessions_fold_under_the_live_ones() {
        let mut feed = Feed::default();
        let mut blocked = agent("T-1", "R1", Role::Worker);
        blocked.status = SessionStatus::Blocked {
            excerpt: "Overwrite?".into(),
        };
        feed.agents.insert(Id::from("T-1_R1"), blocked);
        let mut gone = agent("T-2", "R1", Role::Worker);
        gone.status = SessionStatus::Gone;
        feed.agents.insert(Id::from("T-2_R1"), gone);
        let mut state = State::default();
        let folded = sections(&feed, &Scope::All, &state);
        let shape: Vec<_> = folded
            .iter()
            .map(|section| (section.key.as_str(), section.count, section.rows.len()))
            .collect();
        assert_eq!(shape, [("blocked", 1, 1), ("gone", 1, 0)]);
        state.unfolded.insert("gone".into());
        assert_eq!(sections(&feed, &Scope::All, &state)[1].rows.len(), 1);
    }

    /// A head that counted an unreported cost as $0.00 said the live sessions spent less than
    /// they did: only reported costs are summed, the sum is marked a lower bound while one is
    /// missing, and without any no sum is said.
    #[test]
    fn the_head_sums_only_the_costs_the_host_reports() {
        use corral_protocol::rows::AssistantKind;

        use crate::model::agents::source_tests::assistant;
        use crate::model::health::daemon_env;
        use crate::model::nav::Form;

        let mut feed = Feed::default();
        feed.agents
            .insert(Id::from("T-1_R1"), agent("T-1", "R1", Role::Worker));
        let summary_of = |feed: &Feed| {
            let env = daemon_env(feed, Form::Desktop);
            let rows: Vec<SessionRow> = sections(feed, &Scope::All, &State::default())
                .iter()
                .flat_map(|section| section.rows.iter())
                .filter_map(|id| row(feed, id, env))
                .collect();
            summary(&rows)
        };
        assert_eq!(summary_of(&feed), "1 live");
        let mut chat = assistant("A-1", AssistantKind::Session);
        chat.cost_usd = Some(1.5);
        feed.assistants.insert(Id::from("A-1"), chat);
        assert_eq!(summary_of(&feed), "2 live · $1.50+ spent by them");
    }
}
