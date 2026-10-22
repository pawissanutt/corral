mod recovery_support;

use corral_core::{
    AcceptanceMode, CheckDisposition, GateResult, RecoveryAcceptance, RecoveryAction,
    RecoveryCheck, RecoveryCheckRecord, RecoveryEffect, RecoveryLink, RecoveryObservation,
    RecoveryReceipt, RecoveryRecord, RecoveryReference, RecoveryState, Status, Store, Verdict,
};
use recovery_support::{append_outcome, recovery};
use serde_json::json;

#[test]
fn approved_overridden_waived_and_unknown_remain_distinct() {
    // Given: an actual approve, an overridden predicate, a missing review and uncertain delivery.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut record = recovery(&store);
    let approve = RecoveryCheckRecord {
        check: RecoveryCheck::ReviewOutcome,
        observed: RecoveryObservation::Review {
            verdict: Some(Verdict::Approve),
        },
        disposition: CheckDisposition::Satisfied,
        evidence: vec!["actual-review-notes".into()],
    };
    let mut waived = approve.clone();
    waived.check = RecoveryCheck::IndependentPlanReview;
    waived.observed = RecoveryObservation::Missing;
    waived.disposition = CheckDisposition::Waived;
    record.preview.checks.extend([approve, waived]);
    append_outcome(&mut record, RecoveryState::Applying);
    append_outcome(&mut record, RecoveryState::Unknown);
    // When
    let json = serde_json::to_value(&record).unwrap();
    let restored: RecoveryRecord = serde_json::from_value(json.clone()).unwrap();
    // Then
    assert_eq!(restored, record);
    assert_eq!(json["preview"]["checks"][0]["disposition"], "overridden");
    assert_eq!(json["preview"]["checks"][2]["disposition"], "satisfied");
    assert_eq!(
        json["preview"]["checks"][2]["observed"]["verdict"],
        "approve"
    );
    assert_eq!(json["preview"]["checks"][3]["disposition"], "waived");
    assert_eq!(json["preview"]["checks"][3]["observed"]["kind"], "missing");
    assert_eq!(json["outcomes"][2]["state"], "unknown");
    assert!(json["outcomes"][2]["actual_effect"].is_null());
}

#[test]
fn acceptance_modes_preserve_legacy_absence_and_recovery_provenance() {
    // Given
    let reference = RecoveryReference {
        operation: "REC1".into(),
        contract_digest: "contract".into(),
    };
    let cases = [
        RecoveryAcceptance {
            acceptance_mode: AcceptanceMode::Policy,
            policy_satisfied: true,
            recovery: None,
            reason: None,
            checks: vec![],
        },
        RecoveryAcceptance {
            acceptance_mode: AcceptanceMode::OwnerOverride,
            policy_satisfied: false,
            recovery: None,
            reason: None,
            checks: vec![],
        },
        RecoveryAcceptance {
            acceptance_mode: AcceptanceMode::OwnerRecovery,
            policy_satisfied: false,
            recovery: Some(reference),
            reason: Some("Bound exception".into()),
            checks: vec![],
        },
    ];
    // When
    let values = cases
        .iter()
        .map(|case| {
            case.validate().unwrap();
            serde_json::to_value(case).unwrap()
        })
        .collect::<Vec<_>>();
    // Then
    assert_eq!(values[0]["acceptance_mode"], "policy");
    assert_eq!(values[1]["acceptance_mode"], "owner_override");
    assert!(values[1]["reason"].is_null());
    assert_eq!(values[2]["acceptance_mode"], "owner_recovery");
    assert_eq!(values[2]["recovery"]["operation"], "REC1");
}

#[test]
fn overridden_failed_gate_is_not_serialized_as_passing() {
    // Given
    let receipt = RecoveryAcceptance {
        acceptance_mode: AcceptanceMode::OwnerRecovery,
        policy_satisfied: false,
        recovery: Some(RecoveryReference {
            operation: "REC1".into(),
            contract_digest: "contract".into(),
        }),
        reason: Some("Accept the completed current gate outcome".into()),
        checks: vec![RecoveryCheckRecord {
            check: RecoveryCheck::GateOutcome,
            observed: RecoveryObservation::Gate {
                result: GateResult::Fail,
            },
            disposition: CheckDisposition::Overridden,
            evidence: vec!["original-failing-gate-log".into()],
        }],
    };
    // When
    receipt.validate().unwrap();
    let json = serde_json::to_value(&receipt).unwrap();
    // Then
    assert_eq!(json["checks"][0]["observed"]["result"], "fail");
    assert_eq!(json["checks"][0]["disposition"], "overridden");
    assert_eq!(json["policy_satisfied"], false);
    let mut forged = receipt;
    forged.policy_satisfied = true;
    assert!(forged.validate().is_err());
}

#[test]
fn future_actions_effects_and_dispositions_fail_closed() {
    // Given / When / Then: an unknown wire value cannot mean an authorized generic action.
    assert!(serde_json::from_value::<RecoveryAction>(json!({"action": "force"})).is_err());
    assert!(
        serde_json::from_value::<RecoveryEffect>(json!({"kind": "shell", "command": "anything"}))
            .is_err()
    );
    assert!(serde_json::from_value::<CheckDisposition>(json!("passed_by_waiver")).is_err());
    assert!(serde_json::from_value::<RecoveryState>(json!("retry_automatically")).is_err());
}

#[test]
fn reason_versions_preview_binding_and_target_are_required() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    store
        .create("Bound recovery", None, Status::NeedsMe, "")
        .unwrap();
    store.upgrade_schema().unwrap();
    let original = recovery(&store);
    // When / Then
    for kind in 0..7 {
        let mut changed = original.clone();
        match kind {
            0 => changed.authorization.reason = " \t".into(),
            1 => changed.authorization.preview_digest = "different".into(),
            2 => changed.version = 2,
            3 => changed.preview.request.version = 2,
            4 => changed.preview.version = 2,
            5 => changed.authorization.version = 2,
            6 => changed.preview.request.target.task = "T-002".into(),
            _ => unreachable!(),
        }
        assert!(
            store.save_recovery_record("T-001", &changed).is_err(),
            "{kind}"
        );
    }
    assert!(store.task_record("T-001").unwrap().recoveries.is_empty());
}

#[test]
fn retries_are_idempotent_and_immutable_history_cannot_be_rewritten() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store.create("History", None, Status::NeedsMe, "").unwrap();
    store.upgrade_schema().unwrap();
    let record = recovery(&store);
    store.save_recovery_record(&task.id, &record).unwrap();
    let bytes = std::fs::read(&task.path).unwrap();
    // When
    store.save_recovery_record(&task.id, &record).unwrap();
    // Then
    assert_eq!(std::fs::read(&task.path).unwrap(), bytes);
    for kind in 0..4 {
        let mut changed = record.clone();
        match kind {
            0 => changed.authorization.reason.push_str(" broaden"),
            1 => changed.preview.policy.digest = "new-policy".into(),
            2 => changed.outcomes.clear(),
            3 => changed.outcomes[0].detail = "replacement".into(),
            _ => unreachable!(),
        }
        assert!(store.save_recovery_record(&task.id, &changed).is_err());
    }
    assert_eq!(std::fs::read(&task.path).unwrap(), bytes);
}

#[test]
fn unknown_outcome_is_preserved_when_reconciled_once() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    store
        .create("Uncertain", None, Status::NeedsMe, "")
        .unwrap();
    store.upgrade_schema().unwrap();
    let mut record = recovery(&store);
    append_outcome(&mut record, RecoveryState::Applying);
    append_outcome(&mut record, RecoveryState::Unknown);
    store.save_recovery_record("T-001", &record).unwrap();
    append_outcome(&mut record, RecoveryState::Applied);
    // When
    store.save_recovery_record("T-001", &record).unwrap();
    // Then
    let saved = store.task_record("T-001").unwrap();
    assert_eq!(
        saved.recoveries[0].outcomes[2].state,
        RecoveryState::Unknown
    );
    assert_eq!(
        saved.recoveries[0].outcomes[3].state,
        RecoveryState::Applied
    );
    append_outcome(&mut record, RecoveryState::Applying);
    assert!(store.save_recovery_record("T-001", &record).is_err());
}

#[test]
fn exact_receipt_links_keep_policy_exceptions_and_original_verdict_separate() {
    // Given
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    store
        .create("Receipt chain", None, Status::NeedsMe, "")
        .unwrap();
    store.upgrade_schema().unwrap();
    let mut record = recovery(&store);
    let reference = RecoveryReference {
        operation: record.id.clone(),
        contract_digest: record.preview.contract_digest.clone(),
    };
    record.links.push(RecoveryLink {
        recovery: reference.clone(),
        receipt: RecoveryReceipt::PlanReview {
            plan: "P1".into(),
            attempt: "PR2".into(),
        },
        acceptance: None,
    });
    record.links.push(RecoveryLink {
        recovery: reference.clone(),
        receipt: RecoveryReceipt::PlanApproval {
            plan: "P1".into(),
            at: "2026-10-18T11:05:00Z".parse().unwrap(),
        },
        acceptance: Some(RecoveryAcceptance {
            acceptance_mode: AcceptanceMode::OwnerRecovery,
            policy_satisfied: false,
            recovery: Some(reference),
            reason: Some(record.authorization.reason.clone()),
            checks: record.preview.checks.clone(),
        }),
    });
    // When
    store.save_recovery_record("T-001", &record).unwrap();
    // Then
    let reopened = Store::open_unobserved(tmp.path())
        .unwrap()
        .task_record("T-001")
        .unwrap();
    assert_eq!(reopened.recoveries[0].links, record.links);
    record.links[0].recovery.contract_digest = "unrelated-contract".into();
    assert!(store.save_recovery_record("T-001", &record).is_err());
}
