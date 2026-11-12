use super::*;
use crate::submission_paths_tests::fixture::{Fx, git};

#[test]
fn external_delivery_reconciles_stale_candidate_without_replaying_or_erasing_history() {
    if isolated(
        "external::external_delivery_reconciles_stale_candidate_without_replaying_or_erasing_history",
    ) {
        return;
    }
    let mut f = Fx::new();
    f.ctx.store.config.review.gate = Some(vec!["false".into()]);
    f.ctx.store.save_config().unwrap();
    let id = f.task("Externally delivered", Status::Ready).id;
    f.start(&id);
    f.done(&id, "obsolete.txt");
    crate::gate_start(&f.ctx, &id).unwrap();
    let failed = crate::gate_collect(&f.ctx, &id).unwrap().unwrap();
    assert_eq!(failed.result, GateResult::Fail);
    let attempt = &failed.identity.as_ref().unwrap().attempt;
    let failed_row = serde_json::to_value(
        f.ctx
            .store
            .gate_attempt(&id, "R1", attempt)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    f.ctx
        .store
        .update(&id, |t| {
            t.rounds = 2;
            t.current_run_mut().unwrap().gate.as_mut().unwrap().result = GateResult::Stale;
            Ok(())
        })
        .unwrap();
    let repo = f.temp.path().join("app");
    std::fs::write(repo.join("delivered.txt"), "independently delivered\n").unwrap();
    git(&repo, &["add", "delivered.txt"]);
    git(
        &repo,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-qm",
            "external delivery",
        ],
    );
    let commit = git(&repo, &["rev-parse", "HEAD"]);
    let tree = git(&repo, &["rev-parse", "HEAD^{tree}"]);
    let source = f.temp.path().join("external-proof.txt");
    std::fs::write(
        &source,
        format!("External gate/review/landing for {commit} tree {tree}"),
    )
    .unwrap();
    let artifact =
        crate::add_artifact(&f.ctx, &id, &source, Some(ArtifactKind::Evidence), None).unwrap();
    let proof = serde_json::json!({"reference": artifact.path, "sha256": artifact.sha256});
    let input: RecoveryInput = serde_json::from_value(serde_json::json!({
        "action": {"action":"reconcile-external", "delivery": {
            "commit":commit, "tree":tree, "gate":proof, "review":proof, "landing":proof
        }},
        "exceptions":["native-candidate", "gate-outcome", "review-outcome"], "notes":""
    }))
    .unwrap();
    let mut history = RepairHistory::new(
        RepairPolicy::approved_starter_policy(),
        "fake",
        Some("rev".into()),
    )
    .unwrap();
    history
        .apply(RepairEvent::Started {
            receipt: RepairStartReceipt {
                attempt_id: RepairAttemptId("initial".into()),
                profile: "fake".into(),
                kind: RepairAttemptKind::InitialImplementation,
                findings: vec![],
            },
        })
        .unwrap();
    let failure = FailureIdentity {
        criterion_or_test: "native-check".into(),
        affected_contract_or_location: "obsolete.txt".into(),
        failure_code: "stale".into(),
    };
    history
        .apply(RepairEvent::Finding {
            finding: RepairFinding {
                observation_id: "obs1".into(),
                finding_id: FindingId("F1".into()),
                fingerprint: failure.fingerprint(),
                identity: failure,
                observed_failure: "Retained unresolved disclosure".into(),
                matching: FindingMatch::New,
                attempted_fixes: vec![],
                resolution: FindingResolution::Unresolved,
            },
        })
        .unwrap();
    history
        .apply(RepairEvent::Started {
            receipt: RepairStartReceipt {
                attempt_id: RepairAttemptId("repair1".into()),
                profile: "fake".into(),
                kind: RepairAttemptKind::CurrentProfileRepair,
                findings: vec![FindingId("F1".into())],
            },
        })
        .unwrap();
    f.ctx
        .store
        .save_record(REPAIR_RUNTIME, &id, &RepairLedger::new(history))
        .unwrap();
    let before = f.ctx.store.get(&id).unwrap();
    let repair_before = f.ctx.store.repair_ledger(&id).unwrap();
    let mut unrelated = f.task("Unrelated unfinished", Status::Running);
    unrelated = f
        .ctx
        .store
        .update(&unrelated.id, |t| {
            t.depends_on.push(id.clone());
            Ok(())
        })
        .unwrap();
    assert!(
        crate::queue::dependency_check(&f.ctx.store, &unrelated)
            .unwrap()
            .is_some()
    );
    assert!(crate::wip::task_in_flight(&before));
    let unrelated_before = f.ctx.store.fixture_task_text(&unrelated.id).unwrap();
    let preview = recover_preview(&f.ctx, &id, &input).unwrap();
    assert!(
        preview.remaining_blockers.is_empty(),
        "{:?}",
        preview.remaining_blockers
    );
    assert!(recover_apply(&f.ctx, &preview, &input, None).is_err());
    let mut blank = confirmation(&preview);
    blank.authorization.reason.clear();
    assert!(recover_apply(&f.ctx, &preview, &input, Some(blank)).is_err());
    // An unfinished integration is not silently published or abandoned by force completion.
    let mut operation = crate::hooks::PostLandOperation {
        version: 1,
        key: "unfinished-fixture".into(),
        project: "app".into(),
        repo: repo.clone(),
        task: unrelated.id.clone(),
        run: "R1".into(),
        base: "main".into(),
        base_before: before.current_run().unwrap().base_sha.clone(),
        landed_sha: commit.clone(),
        policy_digest: "fixture".into(),
        created: corral_core::clock::now(),
        landed: false,
        abandoned: false,
        notified: false,
        notice: None,
        hooks: vec![],
        install_checked: false,
    };
    crate::hooks::save(&f.ctx, &operation).unwrap();
    assert!(recover_preview(&f.ctx, &id, &input).is_err());
    assert!(recover_apply(&f.ctx, &preview, &input, Some(confirmation(&preview))).is_err());
    assert!(
        !crate::hooks::landing_operation(&f.ctx, &operation.key)
            .unwrap()
            .unwrap()
            .abandoned
    );
    operation.abandoned = true;
    crate::hooks::save(&f.ctx, &operation).unwrap();
    // A changed task is not authorized by a historical owner confirmation.
    f.ctx
        .store
        .update(&id, |t| {
            t.rounds += 1;
            Ok(())
        })
        .unwrap();
    assert!(recover_apply(&f.ctx, &preview, &input, Some(confirmation(&preview))).is_err());
    f.ctx
        .store
        .update(&id, |t| {
            t.rounds -= 1;
            Ok(())
        })
        .unwrap();
    for (field, value) in [("commit", "0".repeat(40)), ("tree", "0".repeat(40))] {
        let mut bad = serde_json::to_value(&input).unwrap();
        bad["action"]["delivery"][field] = value.into();
        let bad: RecoveryInput = serde_json::from_value(bad).unwrap();
        assert!(recover_preview(&f.ctx, &id, &bad).is_err());
    }
    let mut unreachable = serde_json::to_value(&input).unwrap();
    let worktree = &before.current_run().unwrap().worktree;
    unreachable["action"]["delivery"]["commit"] = git(worktree, &["rev-parse", "HEAD"]).into();
    unreachable["action"]["delivery"]["tree"] = git(worktree, &["rev-parse", "HEAD^{tree}"]).into();
    assert!(recover_preview(&f.ctx, &id, &serde_json::from_value(unreachable).unwrap()).is_err());
    let mut mismatched = serde_json::to_value(&input).unwrap();
    mismatched["action"]["delivery"]["gate"]["sha256"] = "0".repeat(64).into();
    assert!(recover_preview(&f.ctx, &id, &serde_json::from_value(mismatched).unwrap()).is_err());
    assert!(records(&f.ctx, &id).unwrap().is_empty());
    assert_eq!(f.ctx.store.get(&id).unwrap().status, Status::Review);
    // The destination is CAS-bound too, even when the supplied commit stays reachable.
    git(
        &repo,
        &[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "later unrelated delivery",
        ],
    );
    let destination_head = git(&repo, &["rev-parse", "HEAD"]);
    assert!(recover_apply(&f.ctx, &preview, &input, Some(confirmation(&preview))).is_err());
    let fresh = recover_preview(&f.ctx, &id, &input).unwrap();
    let record = recover_apply(&f.ctx, &fresh, &input, Some(confirmation(&fresh))).unwrap();
    assert_eq!(
        state(&record).unwrap(),
        RecoveryState::Applied,
        "{:?}",
        record.outcomes
    );
    let after = f.ctx.store.get(&id).unwrap();
    assert_eq!(after.status, Status::Done);
    assert!(!crate::wip::task_in_flight(&after));
    assert!(
        crate::queue::dependency_check(&f.ctx.store, &unrelated)
            .unwrap()
            .is_none()
    );
    assert_eq!(after.rounds, before.rounds);
    assert_eq!(f.ctx.store.repair_ledger(&id).unwrap(), repair_before);
    assert_eq!(
        serde_json::to_value(
            f.ctx
                .store
                .gate_attempt(&id, "R1", attempt)
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        failed_row
    );
    assert_eq!(after.runs, before.runs); // The existing run was already done; no fabricated drain or gate.
    assert_eq!(after.landed.as_ref().unwrap().sha, commit);
    assert!(serde_json::to_value(&after.landed).unwrap()["external"].is_object());
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), destination_head);
    assert!(!repo.join("obsolete.txt").exists());
    assert!(before.current_run().unwrap().worktree.exists());
    assert_eq!(
        f.ctx.store.fixture_task_text(&unrelated.id).unwrap(),
        unrelated_before
    );
    assert!(
        crate::artifact_storage::protected(&f.ctx, std::slice::from_ref(&after))
            .unwrap()
            .contains(&artifact.path)
    );
    let committed = f.ctx.store.fixture_task_text(&id).unwrap();
    assert_eq!(recover_apply(&f.ctx, &fresh, &input, None).unwrap(), record);
    assert_eq!(f.ctx.store.fixture_task_text(&id).unwrap(), committed);
    assert_eq!(recover_reconcile(&f.ctx, &id, &record.id).unwrap(), record);
    let approvals: Vec<_> = f
        .ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "approved" && e.task.as_deref() == Some(&id))
        .collect();
    assert_eq!(approvals.len(), 1);
    assert_eq!(approvals[0].detail["acceptance_mode"], "external_forced");
    assert_eq!(approvals[0].detail["policy_satisfied"], false);
    assert_eq!(
        RecoverySummary::of(&recover_history(&f.ctx, &id).unwrap()).external_forced,
        1
    );
    let finishes = crate::finishes(&f.ctx.store, &[after]).unwrap();
    assert_eq!(
        finishes[&id].landed.as_ref().unwrap().strategy,
        "external_forced"
    );
}
