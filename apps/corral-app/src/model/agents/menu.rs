//! A session's Session menu, on its roster row, its panel and its page head: the session
//! controls that apply to it (Cancel turn, Pause, Resume, Rewind…, Terminate or Relaunch),
//! then for a run Reply (its task's engine operation), Focus pane where it can work and why not
//! where it cannot, and its task's whole Actions menu. A chat's menu holds its controls.

use crate::model::Id;
use crate::model::daemon;
use crate::model::work::{Press, Record, Verb, VerbAction, session_task};

use super::control::control_verbs;
use super::pane::focus_refusal;

/// The Session menu trigger's words.
pub const SESSION_MENU_LABEL: &str = "Session";

/// The Session menu of session `session` (`T-42_R1`, `A-3`).
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
    let reason = focus_refusal(feed, feed.agents.get(session.as_str()));
    verbs.push(VerbAction {
        slug: "focus-pane",
        label: "Focus pane".into(),
        enabled: reason.is_none(),
        reason,
        press: Press::Focus(session.clone()),
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
    use corral_protocol::app::AppSession;
    use corral_protocol::device::{DeviceInfo, DevicePlatform, DeviceReach};
    use corral_types::{Role, SessionRef, SessionStatus};

    use super::super::roster_tests::agent;
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
                "focus-pane",
                "task-actions"
            ]
        );
        assert_eq!(slugs(&feed, "T-2_R2")[..2], ["cancel-turn", "terminate"]);
        assert_eq!(slugs(&feed, "T-3_R1")[0], "relaunch");
    }

    /// Focus pane is offered for a herdr pane on a connection corrald reports as local, and
    /// says why it cannot work for a headless session or over a remote connection.
    #[test]
    fn focus_pane_works_only_for_a_local_pane_and_says_why_otherwise() {
        let device = DeviceInfo {
            id: "desk".into(),
            name: "Desk".into(),
            platform: DevicePlatform::Native,
            app_version: "test".into(),
            enrolled_at: String::new(),
            revoked_at: None,
            last_seen: None,
            reach: Some(DeviceReach::Tailscale),
        };
        let mut feed = Feed {
            session: Some(AppSession {
                device,
                seen: Default::default(),
                server_time: String::new(),
                local: Some(true),
            }),
            ..Feed::default()
        };
        let pane = Id::from("T-1_R1");
        feed.agents
            .insert(pane.clone(), agent("T-1", "R1", Role::Worker));
        let mut headless = agent("T-2", "R1", Role::Worker);
        headless.run.session = Some(SessionRef::Process {
            pid: 7,
            dir: "/runs/T-2/R1".into(),
        });
        feed.agents.insert(Id::from("T-2_R1"), headless);
        let focus = |feed: &Feed, id: &str| {
            let verbs = session_verbs(feed, &Id::from(id));
            let focus = verbs
                .iter()
                .find(|verb| verb.slug == "focus-pane")
                .map(|verb| (verb.enabled, verb.reason.is_some()));
            focus.unwrap_or((false, false))
        };
        assert_eq!(focus(&feed, "T-1_R1"), (true, false));
        assert_eq!(focus(&feed, "T-2_R1"), (false, true));
        if let Some(session) = &mut feed.session {
            session.local = Some(false);
        }
        assert_eq!(focus(&feed, "T-1_R1"), (false, true));
    }
}
