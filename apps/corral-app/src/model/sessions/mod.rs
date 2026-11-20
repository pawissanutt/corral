//! Sessions: one list of every session in scope, grouped by state in the order Blocked,
//! Working, Idle, with Gone folded at the bottom. The session page itself is `model::session`.

use std::collections::BTreeSet;

use crate::model::Id;
use crate::model::agents::session_lines;
use crate::model::daemon::Feed;
use crate::model::list::Section;
use crate::model::projects::Scope;
use crate::model::status::{SessionState, session_look};

/// The list's own state: which of the folding sections are open.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    pub unfolded: BTreeSet<String>,
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
    use crate::model::agents::roster_tests::agent;

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
}
