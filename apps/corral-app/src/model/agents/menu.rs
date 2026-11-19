//! A session's Session menu, on its roster row and its page head: the run controls that apply to
//! it (Cancel turn, Pause, Resume, Rewind…, Terminate or Stop, Relaunch or Resume), then for a
//! run Reply (its task's engine operation) and its task's whole Actions menu. A chat's or a
//! build's menu holds its controls.

use crate::model::Id;
use crate::model::daemon;
use crate::model::work::{Press, Record, Verb, VerbAction, session_task};

use super::control::control_verbs;

/// The Session menu trigger's words.
pub const SESSION_MENU_LABEL: &str = "Session";

/// The Session menu of session `session` (`T-42_R1`, `A-3`, `B-k3x9`).
pub fn session_verbs(feed: &daemon::Feed, session: &Id) -> Vec<VerbAction> {
    let mut verbs = control_verbs(feed, session);
    let Some(task) = session_task(session).map(Id::new) else {
        return verbs;
    };
    let record = Record::Task(task.clone());
    verbs.push(VerbAction {
        slug: Verb::Reply.slug(),
        label: Verb::Reply.label().into(),
        enabled: true,
        reason: None,
        press: Press::Verb {
            record: record.clone(),
            verb: Verb::Reply,
        },
    });
    verbs.push(VerbAction {
        slug: "task-actions",
        label: format!("Task {task}"),
        enabled: true,
        reason: None,
        press: Press::Menu(record),
    });
    verbs
}

#[cfg(test)]
mod tests {
    use corral_protocol::rows::{AssistantKind, AssistantState};
    use corral_types::{Role, SessionStatus};

    use super::super::roster_tests::{agent, assistant};
    use super::session_verbs;
    use crate::model::Id;
    use crate::model::daemon::Feed;

    fn slugs(feed: &Feed, id: &str) -> Vec<&'static str> {
        session_verbs(feed, &Id::from(id))
            .iter()
            .map(|verb| verb.slug)
            .collect()
    }

    /// A working worker offers every control; a reviewer is never paused or rewound; a lost
    /// host is relaunched rather than terminated.
    #[test]
    fn each_role_offers_only_the_controls_that_apply() {
        let mut feed = Feed::default();
        let mut worker = agent("T-1", "R1", Role::Worker);
        worker.status = SessionStatus::Working;
        feed.agents.insert(Id::from("T-1_R1"), worker);
        let mut reviewer = agent("T-2", "R2", Role::Reviewer);
        reviewer.status = SessionStatus::Working;
        feed.agents.insert(Id::from("T-2_R2"), reviewer);
        let mut lost = agent("T-3", "R1", Role::Worker);
        lost.status = SessionStatus::Gone;
        feed.agents.insert(Id::from("T-3_R1"), lost);
        assert_eq!(
            slugs(&feed, "T-1_R1"),
            [
                "cancel-turn",
                "pause",
                "resume",
                "rewind",
                "terminate",
                "reply",
                "task-actions"
            ]
        );
        assert_eq!(slugs(&feed, "T-2_R2")[..2], ["cancel-turn", "terminate"]);
        assert_eq!(slugs(&feed, "T-3_R1")[0], "relaunch");
    }

    /// A stopped build is resumed, never offered Terminate or a turn control.
    #[test]
    fn a_stopped_build_offers_only_resume() {
        let mut feed = Feed::default();
        let mut build = assistant("B-k3x9", AssistantKind::Build);
        build.state = AssistantState::Cold;
        feed.assistants.insert("B-k3x9".into(), build);
        assert_eq!(slugs(&feed, "B-k3x9"), ["relaunch"]);
    }
}
