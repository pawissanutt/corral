//! A run's Session menu, on its roster row, its panel and its page head: Reply, Stop and Retry
//! (its task's engine operations, run as the Actions menu runs them, Stop asking once), Focus
//! pane where it can work and why not where it cannot, then its task's whole Actions menu.

use crate::model::Id;
use crate::model::daemon;
use crate::model::work::{Press, Record, Verb, VerbAction, session_task};

use super::pane::focus_refusal;

/// The Session menu trigger's words.
pub const SESSION_MENU_LABEL: &str = "Session";

/// The Session menu of run session `session` (`T-42_R1`); none for an id without a task.
pub fn session_verbs(feed: &daemon::Feed, session: &Id) -> Vec<VerbAction> {
    let Some(task) = session_task(session).map(Id::new) else {
        return Vec::new();
    };
    let record = Record::Task(task.clone());
    let mut verbs: Vec<VerbAction> = [Verb::Reply, Verb::Stop, Verb::Retry]
        .into_iter()
        .map(|verb| VerbAction {
            slug: verb.slug(),
            label: verb.label().into(),
            enabled: true,
            reason: None,
            press: Press::Verb {
                record: record.clone(),
                verb,
            },
        })
        .collect();
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
    use corral_types::{Role, SessionRef};

    use super::super::roster_tests::agent;
    use super::session_verbs;
    use crate::model::Id;
    use crate::model::daemon::Feed;

    /// Focus pane is offered for a herdr pane reached from corrald's own machine, and says why
    /// it cannot work for a headless session or from a remote device; the menu always ends
    /// with the task's Actions menu.
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
            reach: Some(DeviceReach::Unix),
        };
        let mut feed = Feed {
            session: Some(AppSession {
                device,
                seen: Default::default(),
                server_time: String::new(),
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
            assert_eq!(
                verbs.iter().map(|verb| verb.slug).collect::<Vec<_>>(),
                ["reply", "stop", "retry", "focus-pane", "task-actions"]
            );
            (verbs[3].enabled, verbs[3].reason.is_some())
        };
        assert_eq!(focus(&feed, "T-1_R1"), (true, false));
        assert_eq!(focus(&feed, "T-2_R1"), (false, true));
        if let Some(session) = &mut feed.session {
            session.device.reach = Some(DeviceReach::Tailscale);
        }
        assert_eq!(focus(&feed, "T-1_R1"), (false, true));
    }
}
