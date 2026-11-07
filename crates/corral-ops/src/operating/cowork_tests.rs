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
    let legacy = task.clone();
    task.cowork.as_mut().unwrap().completion = Some(corral_core::CompletionScope {
        version: 1,
        contract_revision: task.contract_revision().unwrap(),
        intent: corral_core::CompletionIntent::Verify,
        driver: corral_core::CompletionDriver::Assistant {
            session: "A-1".into(),
            scratch: "fix".into(),
        },
        instruction: "Check only".into(),
    });
    task.cowork.as_mut().unwrap().review = corral_core::CoworkReview::CompletionV1;
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum LegacyReview {
        None,
        Required,
    }
    let wire = serde_json::to_value(task.cowork.as_ref().unwrap()).unwrap();
    assert!(
        serde_json::from_value::<LegacyReview>(wire["review"].clone()).is_err(),
        "an old controller must refuse, not interpret Verify as legacy Ship"
    );
    let mut unfenced = task.cowork.clone().unwrap();
    unfenced.review = corral_core::CoworkReview::Required;
    assert!(
        serde_json::to_vec(&unfenced).is_err(),
        "unfenced scopes must fail before persistence"
    );
    super::super::require(&store, &task, OperatingActionKind::Gate).unwrap();
    let error = super::super::require(&store, &task, OperatingActionKind::Accept).unwrap_err();
    assert!(error.to_string().contains("check_only"));
    task.title.push_str(" widened");
    assert!(
        super::super::require(&store, &task, OperatingActionKind::Gate)
            .unwrap_err()
            .to_string()
            .contains("scope changed")
    );
    task.title = legacy.title.clone();
    task.cowork
        .as_mut()
        .unwrap()
        .completion
        .as_mut()
        .unwrap()
        .driver = corral_core::CompletionDriver::Board;
    // Transfer changes responsibility, not candidate/run, and does not forge a workspace grant.
    assert!(task.cowork_run().unwrap().assistant_driver().is_none());
    assert_eq!(task.current_run(), legacy.current_run());
    assert!(super::super::require(&store, &task, OperatingActionKind::Gate).is_err());
    let mut replacement = task.current_run().unwrap().clone();
    replacement.id = "R2".into();
    task.runs.push(replacement);
    assert!(task.cowork_run().is_some());
    assert!(
        super::super::require_completion(&task)
            .unwrap_err()
            .to_string()
            .contains("check_only")
    );
    task = legacy;
    store.config.review.max_rounds = Some(0);
    store.save_config().unwrap();
    super::super::require_diagnosis(&store, &task, false).unwrap();
    assert!(
        super::super::require(&store, &task, OperatingActionKind::Repair).is_err(),
        "diagnosis cannot enlarge coding allowance"
    );
    store.config.review.max_rounds = None;
    store.save_config().unwrap();
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
