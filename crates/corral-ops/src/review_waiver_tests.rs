use super::*;
use corral_core::{
    AutopilotState, CompletionDriver, CompletionIntent, CompletionScope, Cowork, CoworkReview,
    OperatingActionKind, Reviewer, TaskRef as _,
};

fn fixture() -> (tempfile::TempDir, Store, String) {
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
    let id = store
        .create("Co-work", None, Status::Review, "")
        .unwrap()
        .id;
    let root = tmp.path().to_path_buf();
    store
        .update(&id, |t| {
            t.runs.push(serde_json::from_value(json!({
                "id":"R1", "agent":"fake", "worktree":root, "branch":"b", "base":"main",
                "base_sha":"fixture", "started":t.created, "outcome":"done"
            }))?);
            let contract_revision = t.contract_revision()?;
            t.cowork = Some(Cowork {
                completion: Some(CompletionScope {
                    version: 1,
                    contract_revision,
                    intent: CompletionIntent::Ship,
                    driver: CompletionDriver::Assistant {
                        session: "A-1".into(),
                        scratch: "fix".into(),
                    },
                    instruction: "Ship".into(),
                }),
                session: "A-1".into(),
                scratch: "fix".into(),
                run: "R1".into(),
                review: CoworkReview::CompletionV1,
                owner_confirmed: true,
                review_waiver: None,
            });
            Ok(())
        })
        .unwrap();
    (tmp, store, id)
}

#[test]
fn owner_waiver_lands_one_confirmed_finish_after_its_gate_without_review() {
    let (_tmp, store, id) = fixture();
    let config = store.reopen().unwrap().config;
    assert!(apply(&store, &id, "  ").is_err(), "a reason is required");
    apply(&store, &id, "hotfix for red main").unwrap();

    let task = store.get(&id).unwrap();
    let policy = config.task_review_policy(&task);
    assert_eq!(policy.reviewer, Reviewer::None);
    assert!(!policy.gate.is_empty(), "the gate is never waived");
    crate::operating::require(&store, &task, OperatingActionKind::Accept).unwrap();
    let error = apply(&store, &id, "again").unwrap_err();
    assert!(error.to_string().contains("already waived"), "{error}");

    // The waiver binds R1 only: a later run of the same finish is reviewed again.
    store
        .update(&id, |t| {
            let mut run = t.runs[0].clone();
            run.id = "R2".into();
            t.runs.push(run);
            Ok(())
        })
        .unwrap();
    let later = store.get(&id).unwrap();
    assert_eq!(
        config.task_review_policy(&later).reviewer,
        Reviewer::parse("rev")
    );
}

#[test]
fn waiver_is_refused_while_the_assistant_repairs_requested_changes() {
    let (_tmp, store, id) = fixture();
    store
        .update(&id, |t| {
            let at = t.created;
            t.runs[0].autopilot = Some(AutopilotState {
                stage: AutoStage::Changes,
                reason: None,
                sha: None,
                at,
            });
            Ok(())
        })
        .unwrap();
    let error = apply(&store, &id, "skip").unwrap_err();
    assert!(error.to_string().contains("autopilot is"), "{error}");
}
