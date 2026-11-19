use super::{Cx, Facts, SessionRole, Tab, Ui, actions, shown_tabs};
use crate::model::Id;
use crate::model::daemon::Feed;
use crate::model::health::daemon_env;
use crate::model::nav::Form;

/// A worker's run whose session names no task has nothing to show under Task or Proof: neither
/// tab is offered, neither reads anything, and no catalog row leads to them; the same role with
/// a task has both.
#[test]
fn a_session_without_a_task_has_no_task_or_proof_tab() {
    let feed = Feed::default();
    let id = Id::from("worker-scratch");
    let ui = Ui::default();
    let env = daemon_env(&feed, Form::Desktop);

    let without = Facts {
        role: SessionRole::Worker,
        task: None,
    };
    let cx = Cx::new(&feed, &ui, &id, env, &without);
    let tabs = shown_tabs(&cx);
    assert!(!tabs.contains(&Tab::Task));
    assert!(!tabs.contains(&Tab::Proof));
    assert!(super::task::fetches(&cx).is_empty());
    assert!(super::proof::fetches(&cx).is_empty());
    assert!(actions(&cx).rows().all(|row| {
        !row.id.starts_with("act-task")
            && !row.id.starts_with("act-proof")
            && !row.id.starts_with("act-open-decisions")
            && row.id != "act-view-task"
            && row.id != "act-view-proof"
    }));

    let with = Facts {
        role: SessionRole::Worker,
        task: Some(Id::from("T-12")),
    };
    let cx = Cx::new(&feed, &ui, &id, env, &with);
    let tabs = shown_tabs(&cx);
    assert!(tabs.contains(&Tab::Task));
    assert!(tabs.contains(&Tab::Proof));
}
