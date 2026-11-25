//! Sessions: one list of every session in scope, grouped by state in the order Blocked,
//! Working, Idle, Status unavailable, with Gone folded at the bottom; each row's facts and its one next step, the
//! columns they sit in, the head's summary and the inspector of the selected session. The
//! session page itself is `model::session`.

mod filter;
mod inspect;
mod listing;
pub use listing::{Listing, Sort, listing};

pub const FILTER_LABEL: &str = "Filter sessions";
pub const NO_MATCH: &str = "No sessions match this filter.";
mod row;

use std::collections::{BTreeMap, BTreeSet};

use crate::model::daemon::{Feed, Link};
use crate::model::list::Column;
#[cfg(test)]
use crate::model::{Id, list::Section, projects::Scope};

pub use inspect::{Inspect, Latest, inspect, latest};
pub use row::{SessionRow, row, summary};

pub const TITLE: &str = "Sessions";
pub const EMPTY: &str = "No sessions in this scope. Start a chat or a build with New.";
pub const LOADING: &str = "Reading sessions…";
/// The subscriptions the list's sessions come from, as the feed keys them.
const SOURCES: [&str; 2] = ["agents", "assistant"];

/// The list's own state: which of the folding sections are open.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    pub filter: String,
    pub sorts: BTreeMap<String, Sort>,
    pub unfolded: BTreeSet<String>,
}

impl State {
    pub fn sort(&self, group: &str) -> Sort {
        self.sorts.get(group).copied().unwrap_or_default()
    }

    pub fn set_mode(&mut self, mode: &str) {
        self.filter = filter::Query::without_mode_tokens(&self.filter);
        if mode != "all" {
            if !self.filter.is_empty() {
                self.filter.push(' ');
            }
            self.filter.push_str(&format!("mode:{mode}"));
        }
    }

    /// Opens or closes a folding section.
    pub fn toggle(&mut self, key: &str) {
        if !self.unfolded.remove(key) {
            self.unfolded.insert(key.to_owned());
        }
    }
}

/// The list's columns. As the list narrows the model leaves first, then the project, the
/// context share, the cost, the age, the role and what it does now; the state, id, name and
/// next step always show. The id holds a build's twelve characters (`B-mwpdbr8hg8`), the age
/// `11h46m`, the cost `$123.45+`, the step `Remove…` with More; the name and what it does now
/// stop growing at a readable line.
pub fn columns() -> Vec<Column> {
    vec![
        Column::fixed("state", "", 14.0, 1.0),
        Column::fixed("id", "ID", 88.0, 12.0),
        Column::flex("name", "Session", 140.0, 12.0, 1.2).most(420.0, 48.0),
        Column::fixed("role", "Role", 64.0, 9.0).gives_way(6),
        Column::flex("now", "Now", 110.0, 14.0, 1.0)
            .most(360.0, 40.0)
            .gives_way(7),
        Column::fixed("project", "Project", 96.0, 12.0).gives_way(2),
        Column::fixed("model", "Model", 104.0, 16.0).gives_way(1),
        Column::fixed("context", "Ctx", 40.0, 4.0)
            .end()
            .gives_way(3),
        Column::fixed("cost", "Cost", 60.0, 8.0).end(),
        Column::fixed("activity", "Last activity", 80.0, 8.0)
            .end()
            .short("Activity"),
        Column::fixed("age", "Started", 55.0, 7.0).end(),
        Column::fixed("next", "Next step", 120.0, 13.0)
            .end()
            .short("Next"),
    ]
}

/// What stands in for the rows while the list has none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fill {
    /// Nothing has arrived from corrald yet.
    Loading,
    /// Corrald reported no session in this scope.
    Empty,
    /// A subscription failed: corrald's message and code.
    Failed(String),
}

impl Fill {
    /// What the list says in its rows' place.
    pub fn words(&self) -> String {
        match self {
            Self::Loading => LOADING.to_owned(),
            Self::Empty => EMPTY.to_owned(),
            Self::Failed(words) => words.clone(),
        }
    }
}

/// What the list says besides its rows: the fill while it has none, and whether the rows shown
/// are the last known ones.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Standing {
    pub fill: Option<Fill>,
    pub stale: Option<String>,
}

/// The list's standing from the two subscriptions its sessions come from: runs (`agents`) and
/// chats and builds (`assistant`). Without rows, a failed subscription says why before
/// anything else, since the sessions it would have listed are unknown, not absent; the list is
/// loading until one has answered and while either reads again. A stale line stands over what the list shows from the
/// last update while the link is down or a subscription is out of date.
pub fn standing(feed: &Feed, has_rows: bool) -> Standing {
    let topics = SOURCES.map(|topic| feed.topics.get(topic));
    let failed = topics
        .iter()
        .flatten()
        .find_map(|topic| topic.error.as_ref());
    let fill = if has_rows {
        None
    } else if let Some(error) = failed {
        Some(Fill::Failed(format!(
            "Sessions unavailable: {} ({})",
            error.message, error.code
        )))
    } else if topics.iter().all(Option::is_none)
        || topics.iter().flatten().any(|topic| topic.loading)
    {
        Some(Fill::Loading)
    } else {
        Some(Fill::Empty)
    };
    let stale = (fill.is_none() || fill == Some(Fill::Empty))
        && (feed.link != Link::Up || topics.iter().flatten().any(|topic| topic.stale));
    Standing {
        fill,
        stale: stale.then(|| {
            format!(
                "! Showing sessions from {} · corrald unreachable",
                feed.synced.as_deref().unwrap_or("an unknown time")
            )
        }),
    }
}

#[cfg(test)]
pub fn sections(feed: &Feed, scope: &Scope, state: &State) -> Vec<Section> {
    let env = crate::model::health::daemon_env(feed, crate::model::Form::Desktop);
    listing(feed, scope, state, env, |_| false).sections
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
        feed.agents.insert(Id::from("T-3_R1"), {
            let mut gone = agent("T-3", "R1", Role::Reviewer);
            gone.status = SessionStatus::Gone;
            gone
        });
        state.filter = "state:gone task:T-2".into();
        let filtered = sections(&feed, &Scope::All, &state);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].count, 1);
        assert!(filtered[0].rows.is_empty());
        state.unfolded.insert("gone".into());
        assert_eq!(
            sections(&feed, &Scope::All, &state)[0].rows,
            [Id::from("T-2_R1")]
        );
    }

    #[test]
    fn mode_chips_replace_only_mode_and_count_other_matches() {
        let mut feed = Feed::default();
        for (task, role) in [
            ("T-1", Role::Worker),
            ("T-2", Role::Reviewer),
            ("T-3", Role::Worker),
        ] {
            let mut run = agent(task, "R1", role);
            run.status = if task == "T-3" {
                SessionStatus::Gone
            } else {
                SessionStatus::Working
            };
            feed.agents.insert(Id::from(format!("{task}_R1")), run);
        }
        let env = crate::model::health::daemon_env(&feed, crate::model::Form::Desktop);
        let mut state = State {
            filter: "state:working MODE:reviewer".into(),
            ..State::default()
        };
        state.set_mode("worker");
        let shown = listing(&feed, &Scope::All, &state, env, |_| false);
        assert_eq!(shown.sections[0].rows, [Id::from("T-1_R1")]);
        assert_eq!(shown.sections.len(), 1);
        for mode in ["worker", "reviewer"] {
            let chip = shown.chips.iter().find(|chip| chip.mode == mode).unwrap();
            assert_eq!(chip.count, 1);
            assert_eq!(chip.selected, mode == "worker");
        }
        state.set_mode("all");
        assert_eq!(sections(&feed, &Scope::All, &state)[0].count, 2);
    }

    #[test]
    fn section_sort_uses_the_selected_key_without_changing_other_sections() {
        use crate::model::agents::source_tests::assistant;
        use corral_protocol::rows::{AssistantKind, AssistantState};
        let mut feed = Feed::default();
        for (id, start, activity, cost, name) in [
            ("A-1", 1, 6, Some(1.0), "Zulu"),
            ("A-2", 3, 4, None, "Beta"),
            ("A-3", 2, 5, Some(3.0), "alpha"),
        ] {
            let mut chat = assistant(id, AssistantKind::Session);
            chat.state = AssistantState::Ready;
            chat.created = Some(format!("2025-01-01T00:00:0{start}Z"));
            chat.updated = Some(format!("2025-01-01T00:00:0{activity}Z"));
            chat.cost_usd = cost;
            chat.title = name.into();
            feed.assistants.insert(Id::from(id), chat);
        }
        let mut state = State::default();
        for (sort, expected) in [
            (Sort::Activity, ["A-1", "A-3", "A-2"]),
            (Sort::Started, ["A-2", "A-3", "A-1"]),
            (Sort::Cost, ["A-3", "A-1", "A-2"]),
            (Sort::Name, ["A-3", "A-2", "A-1"]),
        ] {
            state.sorts.insert("idle".into(), sort);
            assert_eq!(
                sections(&feed, &Scope::All, &state)[0].rows,
                expected.map(Id::from)
            );
            assert_eq!(state.sort("working"), Sort::Activity);
        }
    }

    /// A failed sessions subscription read as an empty list, so the owner was told there were
    /// no sessions when corrald could not say.
    #[test]
    fn a_failed_subscription_is_not_an_empty_list() {
        use crate::model::daemon::{ClientError, TopicState};

        let mut feed = Feed::default();
        feed.topics
            .insert("assistant".into(), TopicState::default());
        feed.topics.insert(
            "agents".into(),
            TopicState {
                loading: false,
                stale: true,
                error: Some(ClientError {
                    code: "unavailable".into(),
                    message: "agents unavailable".into(),
                }),
            },
        );
        assert!(matches!(standing(&feed, false).fill, Some(Fill::Failed(_))));
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
