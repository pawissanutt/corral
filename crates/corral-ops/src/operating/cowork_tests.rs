use super::*;
use serde_json::json;

#[test]
fn finish_instruction_is_one_run_not_a_workspace_grant_and_live_stops_still_apply() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.agents.insert(
        "rev".into(),
        corral_core::AgentCmd {
            command: "true".into(),
        },
    );
    store.config.review.gate = Some(vec!["true".into()]);
    store.config.review.reviewer = Some("rev".into());
    store.save_config().unwrap();
    let mut task = store
        .create("Co-work", None, corral_core::Status::Review, "")
        .unwrap();
    task.runs.push(
        serde_json::from_value(json!({
            "id":"R1", "agent":"fake", "worktree":tmp.path(), "branch":"b", "base":"main",
            "base_sha":"fixture", "started":task.created, "outcome":"done"
        }))
        .unwrap(),
    );
    task.cowork = Some(serde_json::from_value(json!({
        "session":"A-1", "scratch":"fix", "run":"R1", "review":"required", "owner_confirmed":true
    })).unwrap());
    for kind in [
        OperatingActionKind::Gate,
        OperatingActionKind::Review,
        OperatingActionKind::Repair,
        OperatingActionKind::Accept,
    ] {
        super::super::require(&store, &task, kind).unwrap();
        task.hold = Some(corral_core::TaskHold {
            reason: "urgent work".into(),
            at: task.created,
            resume: None,
            focus: None,
        });
        let error = super::super::require(&store, &task, kind).unwrap_err();
        assert!(error.to_string().contains("Held:"), "{kind:?}: {error}");
        task.hold = None;
        super::super::require(&store, &task, kind).unwrap();
    }
    assert!(super::super::require(&store, &task, OperatingActionKind::Dispatch).is_err());
    let mut other = task.clone();
    other.cowork.as_mut().unwrap().run = "R0".into();
    assert!(super::super::require(&store, &other, OperatingActionKind::Gate).is_err());
    other = task.clone();
    other.cowork.as_mut().unwrap().owner_confirmed = false;
    assert!(super::super::require(&store, &other, OperatingActionKind::Accept).is_err());
    store.config.automation.paused = true;
    store.save_config().unwrap();
    assert!(super::super::require(&store, &task, OperatingActionKind::Gate).is_err());
    store.config.automation.paused = false;
    store.config.review.autopilot = Some(false);
    store.save_config().unwrap();
    assert!(super::super::require(&store, &task, OperatingActionKind::Review).is_err());
    store.config.review.autopilot = Some(true);
    store.config.dispatch.paused = true;
    store.save_config().unwrap();
    assert!(super::super::require(&store, &task, OperatingActionKind::Accept).is_err());
}
