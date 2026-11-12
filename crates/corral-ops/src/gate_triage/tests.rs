use super::*;
use serde_json::json;

fn gate(result: &str, head: &str, attempt: &str) -> GateRecord {
    serde_json::from_value(json!({
        "result": result, "sha": head, "at": "2026-01-01T00:00:00Z",
        "identity": {"version":2,"task":"T-001","run":"R1","attempt":attempt,
            "head":head,"tree":"tree","integration_base":"base","dispatch_base":"base",
            "spec_digest":"spec","policy_digest":"policy","inputs_digest":"inputs"}
    }))
    .unwrap()
}

#[test]
fn unexplained_red_survives_green_run_and_driver_changes_with_one_diagnostic_attempt() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let failed = gate("fail", "candidate", "first");
    assert!(matches!(
        route(&store, "T-001", &failed).unwrap(),
        Route::Block(_, true)
    ));
    admit(&store, "T-001", Some(&failed), "candidate", "diagnostic").unwrap();
    // Re-entering the same durable start is idempotent; a different launch is refused.
    admit(&store, "T-001", Some(&failed), "candidate", "diagnostic").unwrap();
    let reopened = store.reopen().unwrap();
    let green = gate("pass", "candidate", "diagnostic");
    observe(&reopened, "T-001", &green).unwrap();
    assert!(require_pass(&reopened, "T-001", Some(&green)).is_err());
    assert!(
        check_retry(
            &reopened,
            "T-001",
            Some(&green),
            "new-candidate",
            Some("again")
        )
        .is_err()
    );
    for class in [FailureClass::Mixed, FailureClass::Flaky] {
        let mut state: GateTriage = reopened.record(AREA, "T-001").unwrap().unwrap();
        state.failures[0].diagnoses.push(GateDiagnosis {
            attempt: "first".into(),
            class,
            explanation: "uncertainty retained".into(),
            evidence: vec!["proof".into()],
            baseline: None,
            retry: false,
        });
        reopened.save_record(AREA, "T-001", &state).unwrap();
        observe(&reopened, "T-001", &green).unwrap();
        assert!(require_pass(&reopened, "T-001", Some(&green)).is_err());
        assert!(matches!(
            route(&reopened, "T-001", &failed).unwrap(),
            Route::Block(_, true)
        ));
    }
}

#[test]
fn stale_diagnostic_releases_only_its_own_retry() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let failed = gate("fail", "candidate", "first");
    observe(&store, "T-001", &failed).unwrap();
    admit(&store, "T-001", Some(&failed), "candidate", "diagnostic").unwrap();
    observe(&store, "T-001", &gate("stale", "candidate", "other")).unwrap();
    assert!(check_retry(&store, "T-001", None, "rebased", Some("retry")).is_err());
    let stale = gate("stale", "candidate", "diagnostic");
    let state = observe(&store, "T-001", &stale).unwrap();
    assert!(state.diagnostic_attempt.is_none());
    assert!(state.retry_requested);
    assert_eq!(state.diagnostic_history, ["diagnostic"]);
    admit(&store, "T-001", Some(&stale), "rebased", "retry").unwrap();
    // Replaying the old stale receipt cannot release the replacement attempt.
    observe(&store, "T-001", &stale).unwrap();
    let retry = gate("fail", "rebased", "retry");
    observe(&store, "T-001", &retry).unwrap();
    assert!(check_retry(&store, "T-001", Some(&retry), "rebased", Some("third")).is_err());
}

#[test]
fn resolved_infrastructure_allows_repair_verification_without_renewing_diagnosis() {
    for class in [FailureClass::Environment, FailureClass::RedBase] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::init(root.path()).unwrap();
        let failed = gate("fail", "candidate", "infrastructure");
        let mut state = observe(&store, "T-001", &failed).unwrap();
        state.failures[0].diagnoses.push(GateDiagnosis {
            attempt: "infrastructure".into(),
            class,
            explanation: "evidenced infrastructure cause".into(),
            evidence: vec!["retained-proof".into()],
            baseline: None,
            retry: true,
        });
        store.save_record(AREA, "T-001", &state).unwrap();
        admit(&store, "T-001", Some(&failed), "candidate", "diagnostic").unwrap();
        let recovered = gate("pass", "candidate", "diagnostic");
        observe(&store, "T-001", &recovered).unwrap();
        require_pass(&store, "T-001", Some(&recovered)).unwrap();
        let reopened = store.reopen().unwrap();
        // Independent review requests code repair. Assistant finish and board fresh-session
        // continuation both publish a fresh run without a previous gate.
        assert!(
            !check_retry(&reopened, "T-001", None, "review-repair", Some("fresh-run")).unwrap(),
            "{class:?}"
        );
        admit(&reopened, "T-001", None, "review-repair", "fresh-run").unwrap();
        // A new, attributed candidate failure must not inherit the old infrastructure block,
        // in either the current run or another replacement run. Unchanged code is not repair.
        let candidate = gate("fail", "review-repair", "candidate-failure");
        let mut state = observe(&reopened, "T-001", &candidate).unwrap();
        state.failures[1].diagnoses.push(GateDiagnosis {
            attempt: "candidate-failure".into(),
            class: FailureClass::Candidate,
            explanation: "new candidate defect".into(),
            evidence: vec!["candidate-proof".into()],
            baseline: None,
            retry: false,
        });
        reopened.save_record(AREA, "T-001", &state).unwrap();
        assert!(matches!(
            route(&reopened, "T-001", &candidate).unwrap(),
            Route::Repair
        ));
        assert!(
            check_retry(
                &reopened,
                "T-001",
                Some(&candidate),
                "review-repair",
                Some("unchanged")
            )
            .is_err()
        );
        for previous in [Some(&candidate), None] {
            assert!(
                !check_retry(&reopened, "T-001", previous, "fixed", Some("repair-gate")).unwrap()
            );
        }
        admit(&reopened, "T-001", Some(&candidate), "fixed", "repair-gate").unwrap();
        let repaired = gate("pass", "fixed", "repair-gate");
        observe(&reopened, "T-001", &repaired).unwrap();
        require_pass(&reopened, "T-001", Some(&repaired)).unwrap();
        let state: GateTriage = reopened.record(AREA, "T-001").unwrap().unwrap();
        assert_eq!(state.failures.len(), 2);
        assert_eq!(state.failures[0].gate, failed);
        assert_eq!(state.failures[0].resolved_by.as_ref(), Some(&recovered));
        assert_eq!(state.failures[1].gate, candidate);
        assert_eq!(state.failures[1].resolved_by.as_ref(), Some(&repaired));
        assert_eq!(state.diagnostic_attempt.as_deref(), Some("diagnostic"));
        assert_eq!(state.diagnostic_history, ["diagnostic"]);
        assert!(state.owner_investigations.is_empty());
        // Recovery grants no new diagnostic allowance for a later unexplained failure.
        let unknown = gate("fail", "next", "unexplained");
        observe(&reopened, "T-001", &unknown).unwrap();
        let green = gate("pass", "next", "unexplained-green");
        observe(&reopened, "T-001", &green).unwrap();
        assert!(require_pass(&reopened, "T-001", Some(&green)).is_err());
        assert!(check_retry(&reopened, "T-001", None, "next-run", Some("again")).is_err());
    }
}

#[test]
fn owner_investigation_replay_cannot_renew_a_diagnostic_retry_twice() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let task = store
        .create("diagnose", None, corral_core::Status::Review, "scope")
        .unwrap();
    let failed = gate("fail", "candidate", "first");
    store.update(&task.id, |task| {
        task.runs.push(serde_json::from_value(json!({"id":"R1","agent":"fake","worktree":root.path(),
            "branch":"b","base":"main","base_sha":"base","started":task.created,"outcome":"done","gate":failed}))?);
        Ok(())
    }).unwrap();
    let ctx = Ctx::from_store(store.clone());
    let proof = root.path().join("diagnosis.md");
    std::fs::write(
        &proof,
        "Launcher/environment diagnosis for this exact failed attempt.",
    )
    .unwrap();
    let artifact = crate::add_artifact(
        &ctx,
        &task.id,
        &proof,
        Some(corral_core::ArtifactKind::Report),
        Some("diagnosis"),
    )
    .unwrap();
    admit(&store, &task.id, Some(&failed), "candidate", "diagnostic").unwrap();
    let report = GateDiagnosis {
        attempt: "first".into(),
        class: FailureClass::Environment,
        explanation: "Explicit bounded owner investigation after correcting launcher configuration"
            .into(),
        evidence: vec![artifact.path],
        baseline: None,
        retry: true,
    };
    diagnose(&ctx, &task.id, report.clone(), true).unwrap();
    admit(
        &store,
        &task.id,
        Some(&failed),
        "candidate",
        "owner-investigation",
    )
    .unwrap();
    // Lost acknowledgement: the identical observation is historical readback, not more consent.
    diagnose(&ctx, &task.id, report, true).unwrap();
    let state = status(&ctx, &task.id).unwrap();
    assert_eq!(
        state.diagnostic_history,
        ["diagnostic", "owner-investigation"]
    );
    assert_eq!(state.owner_investigations.len(), 1);
    assert!(
        check_retry(
            &store,
            &task.id,
            Some(&failed),
            "new-candidate",
            Some("again")
        )
        .is_err()
    );
}

#[test]
fn delegated_pm_attributes_settled_needs_me_without_coding_or_gate_retry() {
    use corral_core::{OperatingActionKind, Status, TaskRef as _};

    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("app")).unwrap();
    let mut store = Store::init(root.path()).unwrap();
    store.config.projects.insert(
        "app".into(),
        corral_core::Project {
            path: "app".into(),
            dispatch: Default::default(),
            default_profile: None,
            base: None,
            review: Default::default(),
            artifact_retention: None,
            post_land: Vec::new(),
        },
    );
    store.save_config().unwrap();
    let task = store
        .create("Repaired candidate", Some("app"), Status::Review, "scope")
        .unwrap();
    let outside = store
        .create("Not delegated", Some("app"), Status::NeedsMe, "scope")
        .unwrap();
    let preview = store
        .preview_operating_policy(&corral_core::OperatingSelection {
            tasks: [task.id.clone()].into(),
            authority: corral_core::OperatingAuthority {
                dispatch: true,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    store.config = store
        .confirm_operating_policy(&preview, "fixture owner", store.now())
        .unwrap();
    let failed = gate("fail", "broken", "original-red");
    let repaired = gate("pass", "fixed", "repaired-green");
    observe(&store, &task.id, &failed).unwrap();
    admit(&store, &task.id, Some(&failed), "fixed", "repaired-green").unwrap();
    store
        .update(&task.id, |task| {
            task.status = Status::NeedsMe;
            task.runs.push(serde_json::from_value(
                json!({"id":"R1","agent":"fake","worktree":root.path(),
            "branch":"b","base":"main","base_sha":"base","started":task.created,"outcome":"done",
            "gate":repaired,"autopilot":{"stage":"escalated","at":task.created}}),
            )?);
            Ok(())
        })
        .unwrap();
    store.set_attribution(corral_core::Actor::Pm, "pm").unwrap();
    let ctx = Ctx::from_store(store.clone());
    let proof = root.path().join("attribution.md");
    std::fs::write(
        &proof,
        "Retained red and repaired candidate receipt explain the fixture defect.",
    )
    .unwrap();
    let artifact = crate::add_artifact(
        &ctx,
        &task.id,
        &proof,
        Some(corral_core::ArtifactKind::Report),
        None,
    )
    .unwrap();
    let report = GateDiagnosis {
        attempt: "original-red".into(),
        class: FailureClass::Candidate,
        explanation: "The repaired fixture fixes the evidenced candidate defect".into(),
        evidence: vec![artifact.path],
        baseline: None,
        retry: false,
    };
    // Diagnosis is delegated, not coding eligibility or an owner impersonation.
    let error = diagnose(&ctx, &outside.id, report.clone(), false).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("outside the confirmed workspace grant"),
        "{error:#}"
    );
    assert!(
        crate::operating::require(
            &store,
            &store.head(&task.id).unwrap(),
            OperatingActionKind::Repair
        )
        .is_err()
    );
    diagnose(&ctx, &task.id, report.clone(), false).unwrap();
    let head = store.head(&task.id).unwrap();
    assert_eq!(head.status, Status::Review);
    let run = head.current_run_head().unwrap().unwrap();
    assert_eq!(run.gate.as_ref(), Some(&repaired));
    assert_eq!(
        run.autopilot.as_ref().unwrap().stage,
        corral_core::AutoStage::Gating
    );
    require_pass(&store, &task.id, Some(&repaired)).unwrap();
    assert!(require_pass(&store, &task.id, Some(&gate("pass", "broken", "unchanged"))).is_err());
    drop(head);
    let reopened = Ctx::from_store(store.reopen().unwrap());
    observe(&reopened.store, &task.id, &repaired).unwrap();
    let state = diagnose(&reopened, &task.id, report.clone(), false).unwrap();
    assert_eq!(state.failures[0].gate, failed);
    assert_eq!(state.failures[0].resolved_by.as_ref(), Some(&repaired));
    assert_eq!(
        state.failures[0].diagnoses.as_slice(),
        std::slice::from_ref(&report)
    );
    assert_eq!(state.diagnostic_history, ["repaired-green"]);
    assert_eq!(state.diagnostic_attempt.as_deref(), Some("repaired-green"));
    assert!(!state.retry_requested);
    assert!(state.owner_investigations.is_empty());
    let task_now = reopened.store.get(&task.id).unwrap();
    assert_eq!(task_now.runs.len(), 1);
    assert_eq!(task_now.rounds, 0);
    assert!(reopened.store.repair_ledger(&task.id).unwrap().is_none());
    let events = reopened
        .store
        .events_of(Some(&task.id), &["gate_diagnosis"])
        .unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].actor, Some(corral_core::Actor::Pm));
    assert_eq!(events[0].via.as_deref(), Some("pm"));
    let mut changed = report.clone();
    changed.class = FailureClass::Unknown;
    assert!(diagnose(&reopened, &task.id, changed, false).is_err());
    let mut invalid = report.clone();
    invalid.evidence = vec!["not-attached".into()];
    assert!(diagnose(&reopened, &task.id, invalid, false).is_err());
    // Live holds, pause and revocation fence even identical historical delivery.
    crate::hold::hold(&reopened, &task.id, Some("pause recovery")).unwrap();
    assert!(diagnose(&reopened, &task.id, report.clone(), false).is_err());
    crate::hold::release(&reopened, &task.id).unwrap();
    reopened.store.pause_operating_policy(true).unwrap();
    assert!(diagnose(&reopened, &task.id, report.clone(), false).is_err());
    reopened.store.pause_operating_policy(false).unwrap();
    reopened.store.assist_operating_policy().unwrap();
    assert!(diagnose(&reopened, &task.id, report, false).is_err());
}

#[test]
fn attribution_routes_without_rewriting_receipts_and_requires_fresh_candidate_success() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let failed = gate("fail", "candidate", "first");
    for class in [
        FailureClass::Environment,
        FailureClass::RedBase,
        FailureClass::Candidate,
    ] {
        let state = GateTriage {
            failures: vec![FailureEpisode {
                gate: failed.clone(),
                resolved_by: None,
                diagnoses: vec![GateDiagnosis {
                    attempt: "first".into(),
                    class,
                    explanation: "evidence".into(),
                    evidence: vec!["proof".into()],
                    baseline: None,
                    retry: false,
                }],
            }],
            ..Default::default()
        };
        store.save_record(AREA, "T-001", &state).unwrap();
        let route = route(&store, "T-001", &failed).unwrap();
        assert_eq!(
            matches!(route, Route::Repair),
            class == FailureClass::Candidate
        );
        assert!(require_pass(&store, "T-001", Some(&failed)).is_err());
        let green = gate("pass", "candidate", "fresh");
        assert_eq!(
            require_pass(&store, "T-001", Some(&green)).is_ok(),
            class != FailureClass::Candidate
        );
        let repaired = gate("pass", "fixed-candidate", "fresh");
        require_pass(&store, "T-001", Some(&repaired)).unwrap();
        assert_eq!(state.failures[0].gate.result, GateResult::Fail);
    }
}
