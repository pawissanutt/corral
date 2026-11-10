use super::*;

#[test]
fn continuation_context_preserves_task_initial_findings_and_rejects_tampering() {
    let root = tempfile::tempdir().unwrap();
    let store = crate::Store::init(root.path()).unwrap();
    let task = store
        .create("Context", None, crate::Status::Review, "")
        .unwrap();
    let original = crate::RepairLedger::new(current_exhausted());
    store.save_repair_ledger(&task.id, &original).unwrap();
    let mut unanchored = original.clone();
    unanchored.allowances.push(crate::RepairAllowance {
        history: RepairHistory::new(RepairPolicy::approved_starter_policy(), "general", None)
            .unwrap(),
        diagnosis: None,
        renewal: None,
    });
    let legacy: crate::RepairLedger =
        serde_json::from_slice(&serde_json::to_vec(&unanchored).unwrap()).unwrap();
    assert!(matches!(
        legacy.transition().unwrap(),
        crate::RepairTransition::Owner { .. }
    ));
    let renewal = crate::RepairRenewal {
        id: "owner-action".into(),
        previous_digest: original.digest().unwrap(),
        reason: "bounded continuation".into(),
        approved_by: "owner".into(),
        at: store.now(),
    };
    let mut ledger = store
        .renew_repair_allowance(
            &task.id,
            renewal,
            RepairPolicy::V1 {
                current_profile_limit: 1,
                stronger_profile_limit: 0,
                overall_limit: 1,
                pm_diagnosis_allowance: 0,
            },
            "general",
            None,
        )
        .unwrap();
    let new = &mut ledger.active_mut().unwrap().history;
    assert!(new.has_initial_implementation().unwrap());
    assert_eq!(new.counters().unwrap(), RepairCounters::default());
    assert_eq!(
        new.findings(),
        original.active().unwrap().history.findings()
    );
    rejected_without_change(
        new,
        start("new-initial", RepairAttemptKind::InitialImplementation),
    );
    rejected_without_change(new, start("c2", RepairAttemptKind::CurrentProfileRepair));
    rejected_without_change(
        new,
        start(
            "old-recovery",
            RepairAttemptKind::AvailabilityRecovery {
                interrupted_attempt: RepairAttemptId("c2".into()),
            },
        ),
    );
    new.apply(start("third", RepairAttemptKind::CurrentProfileRepair))
        .unwrap();
    let mut repeated = finding();
    repeated.observation_id = "after-third".into();
    repeated.matching = FindingMatch::Exact {
        finding: repeated.finding_id.clone(),
    };
    repeated.attempted_fixes.push(AttemptedFix {
        attempt: RepairAttemptId("c2".into()),
        change: "historical fix retained".into(),
        evidence: vec!["commit:old".into()],
    });
    new.apply(RepairEvent::Finding { finding: repeated })
        .unwrap();
    rejected_without_change(
        new,
        start("fourth", RepairAttemptKind::CurrentProfileRepair),
    );
    store.save_repair_ledger(&task.id, &ledger).unwrap();
    assert_eq!(ledger.allowances[0], original.allowances[0]);
    let counts = ledger.task_counters().unwrap();
    assert_eq!(
        (counts.initial_implementations, counts.coding_repairs),
        (1, 3)
    );
    let bytes = serde_json::to_vec(&ledger).unwrap();
    let restored: crate::RepairLedger = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(restored.task_counters().unwrap(), counts);
    for field in ["version", "initial", "findings", "predecessor_digest"] {
        let mut value = serde_json::to_value(&ledger).unwrap();
        let context = &mut value["allowances"][1]["history"]["context"];
        context[field] = match field {
            "version" => json!(999),
            "initial" => json!({}),
            "findings" => json!([]),
            _ => json!("0".repeat(64)),
        };
        assert!(
            match serde_json::from_value::<crate::RepairLedger>(value) {
                Err(_) => true,
                Ok(l) => l.validate().is_err(),
            },
            "accepted tampered {field}"
        );
    }
    let mut v1 = serde_json::to_value(&ledger).unwrap();
    v1["version"] = json!(1);
    assert!(
        serde_json::from_value::<crate::RepairLedger>(v1)
            .unwrap()
            .validate()
            .is_err()
    );
    let chained = store
        .renew_repair_allowance(
            &task.id,
            crate::RepairRenewal {
                id: "separate-future-owner-decision".into(),
                previous_digest: ledger.digest().unwrap(),
                reason: "version chain fixture, no coding granted".into(),
                approved_by: "owner".into(),
                at: store.now(),
            },
            RepairPolicy::V1 {
                current_profile_limit: 0,
                stronger_profile_limit: 0,
                overall_limit: 0,
                pm_diagnosis_allowance: 0,
            },
            "general",
            None,
        )
        .unwrap();
    assert_eq!(chained.task_counters().unwrap(), counts);
    let mut invalid = serde_json::to_value(&chained).unwrap();
    invalid["allowances"][2]["history"]["context"]["predecessor_version"] = json!(1);
    assert!(
        serde_json::from_value::<crate::RepairLedger>(invalid)
            .unwrap()
            .validate()
            .is_err()
    );
}
