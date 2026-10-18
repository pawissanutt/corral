use super::*;
use serde_json::{Value, json};

fn finding() -> RepairFinding {
    let identity = FailureIdentity {
        criterion_or_test: "transaction_is_atomic".into(),
        affected_contract_or_location: "store::commit".into(),
        failure_code: "partial_write".into(),
    };
    RepairFinding {
        observation_id: "observation-1".into(),
        finding_id: FindingId("F-1".into()),
        fingerprint: identity.fingerprint(),
        identity,
        observed_failure: "Second write failed after first write persisted".into(),
        matching: FindingMatch::New,
        attempted_fixes: Vec::new(),
        resolution: FindingResolution::Unresolved,
    }
}

fn start(id: &str, kind: RepairAttemptKind) -> RepairEvent {
    let profile = match &kind {
        RepairAttemptKind::InitialImplementation
        | RepairAttemptKind::CurrentProfileRepair
        | RepairAttemptKind::AvailabilityRecovery { .. } => "general",
        RepairAttemptKind::StrongerProfileRepair => "deep",
        RepairAttemptKind::PmDiagnosis => "pm",
    };
    let findings = match &kind {
        RepairAttemptKind::InitialImplementation => Vec::new(),
        RepairAttemptKind::CurrentProfileRepair
        | RepairAttemptKind::StrongerProfileRepair
        | RepairAttemptKind::AvailabilityRecovery { .. }
        | RepairAttemptKind::PmDiagnosis => vec![FindingId("F-1".into())],
    };
    RepairEvent::Started {
        receipt: RepairStartReceipt {
            attempt_id: RepairAttemptId(id.into()),
            profile: profile.into(),
            kind,
            findings,
        },
    }
}

fn history(policy: RepairPolicy, stronger: Option<&str>) -> RepairHistory {
    let mut history = RepairHistory::new(policy, "general", stronger.map(str::to_owned)).unwrap();
    history
        .apply(start("initial", RepairAttemptKind::InitialImplementation))
        .unwrap();
    history
        .apply(RepairEvent::Finding { finding: finding() })
        .unwrap();
    history
}

fn ready() -> RepairHistory {
    history(RepairPolicy::approved_starter_policy(), Some("deep"))
}

fn current_exhausted() -> RepairHistory {
    let mut history = ready();
    history
        .apply(start("c1", RepairAttemptKind::CurrentProfileRepair))
        .unwrap();
    history
        .apply(start("c2", RepairAttemptKind::CurrentProfileRepair))
        .unwrap();
    history
}

fn exhausted() -> RepairHistory {
    let mut history = current_exhausted();
    history
        .apply(start("s1", RepairAttemptKind::StrongerProfileRepair))
        .unwrap();
    history
        .apply(start("s2", RepairAttemptKind::StrongerProfileRepair))
        .unwrap();
    history
}

fn rejected_without_change(history: &mut RepairHistory, event: RepairEvent) {
    let before = history.clone();
    assert!(history.apply(event).is_err());
    assert_eq!(*history, before);
}

#[test]
fn legacy_policy_remains_absent_when_field_is_omitted() {
    // Given
    #[derive(Default, Deserialize, Serialize)]
    struct ConfigExtension {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repair_policy: Option<RepairPolicy>,
    }
    // When
    let legacy: ConfigExtension = serde_json::from_value(json!({})).unwrap();
    // Then
    assert!(legacy.repair_policy.is_none());
    assert_eq!(serde_json::to_value(legacy).unwrap(), json!({}));
}

#[test]
fn policy_round_trips_only_explicit_starter_limits() {
    // Given
    let policy = RepairPolicy::approved_starter_policy();
    // When
    let value = serde_json::to_value(&policy).unwrap();
    let restored: RepairPolicy = serde_json::from_value(value.clone()).unwrap();
    // Then
    assert_eq!(restored, policy);
    assert_eq!(
        value,
        json!({
            "version": "1", "current_profile_limit": 2, "stronger_profile_limit": 2,
            "overall_limit": 4, "pm_diagnosis_allowance": 1
        })
    );
    restored.validate().unwrap();
}

#[test]
fn invalid_policy_is_rejected_before_history_creation() {
    // Given
    let policies = [
        RepairPolicy::V1 {
            current_profile_limit: 1,
            stronger_profile_limit: 1,
            overall_limit: 3,
            pm_diagnosis_allowance: 1,
        },
        RepairPolicy::V1 {
            current_profile_limit: 2,
            stronger_profile_limit: 2,
            overall_limit: 4,
            pm_diagnosis_allowance: 2,
        },
    ];
    // When / Then
    for policy in policies {
        assert!(RepairHistory::new(policy, "general", None).is_err());
    }
}

#[test]
fn unsupported_policy_versions_and_missing_bounds_are_rejected() {
    // Given
    let values = [
        json!({"version": "2"}),
        json!({"version": "1", "overall_limit": 4}),
    ];
    // When / Then
    for value in values {
        assert!(serde_json::from_value::<RepairPolicy>(value).is_err());
    }
}

#[test]
fn fingerprint_uses_exact_structured_identity_not_failure_prose() {
    // Given
    let original = finding();
    let mut changed_prose = original.clone();
    changed_prose.observed_failure = "Different description".into();
    let mut changed_code = original.identity.clone();
    changed_code.failure_code = "different_failure".into();
    // When
    let fingerprint = original.identity.fingerprint();
    // Then
    assert_eq!(fingerprint, changed_prose.identity.fingerprint());
    assert_ne!(fingerprint, changed_code.fingerprint());
    assert_eq!(fingerprint.len(), 64);
}

#[test]
fn fingerprint_field_boundaries_cannot_collide_by_concatenation() {
    // Given
    let mut left = finding().identity;
    let mut right = left.clone();
    left.criterion_or_test = "ab".into();
    left.affected_contract_or_location = "c".into();
    right.criterion_or_test = "a".into();
    right.affected_contract_or_location = "bc".into();
    // When / Then
    assert_ne!(left.fingerprint(), right.fingerprint());
}

#[test]
fn invalid_finding_fields_are_rejected_atomically() {
    // Given
    let mut invalid = Vec::new();
    let mut bad = finding();
    bad.fingerprint = "invalid".into();
    invalid.push(bad);
    let mut bad = finding();
    bad.resolution = FindingResolution::Resolved {
        evidence: Vec::new(),
    };
    invalid.push(bad);
    let mut bad = finding();
    bad.identity.criterion_or_test = " ".into();
    bad.fingerprint = bad.identity.fingerprint();
    invalid.push(bad);
    let mut bad = finding();
    bad.attempted_fixes.push(AttemptedFix {
        attempt: RepairAttemptId("unknown".into()),
        change: "rollback".into(),
        evidence: Vec::new(),
    });
    invalid.push(bad);
    // When / Then
    for bad in invalid {
        let mut history =
            RepairHistory::new(RepairPolicy::approved_starter_policy(), "general", None).unwrap();
        rejected_without_change(&mut history, RepairEvent::Finding { finding: bad });
    }
}

#[test]
fn duplicate_start_after_restart_preserves_exact_counters_and_bytes() {
    // Given
    let mut history = ready();
    let event = start("c1", RepairAttemptKind::CurrentProfileRepair);
    history.apply(event.clone()).unwrap();
    let encoded = serde_json::to_vec(&history).unwrap();
    let mut restored: RepairHistory = serde_json::from_slice(&encoded).unwrap();
    // When
    let application = restored.apply(event).unwrap();
    // Then
    assert_eq!(application, ReceiptApplication::Duplicate);
    assert_eq!(
        restored.counters().unwrap(),
        RepairCounters {
            initial_implementations: 1,
            current_profile_repairs: 1,
            coding_repairs: 1,
            ..RepairCounters::default()
        }
    );
    assert_eq!(serde_json::to_vec(&restored).unwrap(), encoded);
}

#[test]
fn conflicting_attempt_id_is_rejected_without_recounting() {
    // Given
    let mut history = ready();
    history
        .apply(start("c1", RepairAttemptKind::CurrentProfileRepair))
        .unwrap();
    // When / Then
    rejected_without_change(
        &mut history,
        start("c1", RepairAttemptKind::StrongerProfileRepair),
    );
}

#[test]
fn duplicate_serialized_events_are_replayed_once() {
    // Given
    let history = current_exhausted();
    let mut value = serde_json::to_value(&history).unwrap();
    let events = value["events"].as_array_mut().unwrap();
    events.extend(events.clone());
    // When
    let restored: RepairHistory = serde_json::from_value(value).unwrap();
    // Then
    assert_eq!(restored, history);
}

#[test]
fn invalid_serialized_transition_is_rejected_on_restart() {
    // Given
    let mut value = serde_json::to_value(ready()).unwrap();
    value["events"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::to_value(start("s1", RepairAttemptKind::StrongerProfileRepair)).unwrap());
    // When / Then
    assert!(serde_json::from_value::<RepairHistory>(value).is_err());
}

#[test]
fn unsupported_history_version_is_rejected() {
    // Given
    let mut value = serde_json::to_value(ready()).unwrap();
    value["version"] = json!(2);
    // When / Then
    assert!(serde_json::from_value::<RepairHistory>(value).is_err());
}

#[test]
fn repairs_require_an_initial_implementation() {
    // Given
    let mut history = RepairHistory::new(
        RepairPolicy::approved_starter_policy(),
        "general",
        Some("deep".into()),
    )
    .unwrap();
    history
        .apply(RepairEvent::Finding { finding: finding() })
        .unwrap();
    // When / Then
    rejected_without_change(
        &mut history,
        start("c1", RepairAttemptKind::CurrentProfileRepair),
    );
}

#[test]
fn initial_implementation_cannot_be_started_twice() {
    // Given
    let mut history = ready();
    // When / Then
    rejected_without_change(
        &mut history,
        start("initial-again", RepairAttemptKind::InitialImplementation),
    );
}

#[test]
fn stage_limits_prevent_early_escalation_and_extra_current_repairs() {
    // Given
    let mut early = ready();
    let mut exhausted = current_exhausted();
    // When / Then
    rejected_without_change(
        &mut early,
        start("s1", RepairAttemptKind::StrongerProfileRepair),
    );
    rejected_without_change(
        &mut exhausted,
        start("c3", RepairAttemptKind::CurrentProfileRepair),
    );
}

#[test]
fn stronger_profile_allowance_is_not_transferable_when_missing() {
    // Given
    let mut history = history(RepairPolicy::approved_starter_policy(), None);
    history
        .apply(start("c1", RepairAttemptKind::CurrentProfileRepair))
        .unwrap();
    history
        .apply(start("c2", RepairAttemptKind::CurrentProfileRepair))
        .unwrap();
    // When / Then
    rejected_without_change(
        &mut history,
        start("s1", RepairAttemptKind::StrongerProfileRepair),
    );
    rejected_without_change(
        &mut history,
        start("c3", RepairAttemptKind::CurrentProfileRepair),
    );
    assert_eq!(history.counters().unwrap().coding_repairs, 2);
}

#[test]
fn overall_limit_applies_before_remaining_stronger_stage_allowance() {
    // Given
    let policy = RepairPolicy::V1 {
        current_profile_limit: 1,
        stronger_profile_limit: 3,
        overall_limit: 2,
        pm_diagnosis_allowance: 1,
    };
    let mut history = history(policy, Some("deep"));
    history
        .apply(start("c1", RepairAttemptKind::CurrentProfileRepair))
        .unwrap();
    history
        .apply(start("s1", RepairAttemptKind::StrongerProfileRepair))
        .unwrap();
    // When / Then
    rejected_without_change(
        &mut history,
        start("s2", RepairAttemptKind::StrongerProfileRepair),
    );
    assert_eq!(history.counters().unwrap().coding_repairs, 2);
}

#[test]
fn diagnosis_cannot_start_while_coding_allowance_remains() {
    // Given
    let mut history = ready();
    // When / Then
    rejected_without_change(&mut history, start("pm1", RepairAttemptKind::PmDiagnosis));
}

#[test]
fn diagnosis_has_its_own_bound_and_grants_no_extra_coding() {
    // Given
    let mut history = exhausted();
    history
        .apply(start("pm1", RepairAttemptKind::PmDiagnosis))
        .unwrap();
    let encoded = serde_json::to_vec(&history).unwrap();
    let mut restored: RepairHistory = serde_json::from_slice(&encoded).unwrap();
    // When / Then
    rejected_without_change(&mut restored, start("pm2", RepairAttemptKind::PmDiagnosis));
    rejected_without_change(
        &mut restored,
        start("s3", RepairAttemptKind::StrongerProfileRepair),
    );
    assert_eq!(
        restored.counters().unwrap(),
        RepairCounters {
            initial_implementations: 1,
            current_profile_repairs: 2,
            stronger_profile_repairs: 2,
            coding_repairs: 4,
            pm_diagnoses: 1,
            availability_recoveries: 0,
        }
    );
}

#[test]
fn missing_stronger_profile_allows_diagnosis_without_spending_its_allowance() {
    // Given
    let mut history = history(RepairPolicy::approved_starter_policy(), None);
    history
        .apply(start("c1", RepairAttemptKind::CurrentProfileRepair))
        .unwrap();
    history
        .apply(start("c2", RepairAttemptKind::CurrentProfileRepair))
        .unwrap();
    // When
    history
        .apply(start("pm1", RepairAttemptKind::PmDiagnosis))
        .unwrap();
    // Then
    assert_eq!(history.counters().unwrap().coding_repairs, 2);
    assert_eq!(history.counters().unwrap().pm_diagnoses, 1);
}

#[test]
fn zero_allowances_disable_repairs_and_diagnosis() {
    // Given
    let mut history = history(
        RepairPolicy::V1 {
            current_profile_limit: 0,
            stronger_profile_limit: 0,
            overall_limit: 0,
            pm_diagnosis_allowance: 0,
        },
        None,
    );
    // When / Then
    rejected_without_change(
        &mut history,
        start("c1", RepairAttemptKind::CurrentProfileRepair),
    );
    rejected_without_change(&mut history, start("pm1", RepairAttemptKind::PmDiagnosis));
}

#[test]
fn availability_recovery_does_not_consume_coding_allowance() {
    // Given
    let mut history = ready();
    history
        .apply(start("c1", RepairAttemptKind::CurrentProfileRepair))
        .unwrap();
    let recovery = start(
        "recover1",
        RepairAttemptKind::AvailabilityRecovery {
            interrupted_attempt: RepairAttemptId("c1".into()),
        },
    );
    // When
    history.apply(recovery.clone()).unwrap();
    history.apply(recovery).unwrap();
    // Then
    assert_eq!(
        history.counters().unwrap(),
        RepairCounters {
            initial_implementations: 1,
            current_profile_repairs: 1,
            coding_repairs: 1,
            availability_recoveries: 1,
            ..RepairCounters::default()
        }
    );
}

#[test]
fn recovery_requires_existing_attempt_and_preserves_profile() {
    // Given
    let mut history = ready();
    history
        .apply(start("s0", RepairAttemptKind::CurrentProfileRepair))
        .unwrap();
    let unknown = start(
        "recover1",
        RepairAttemptKind::AvailabilityRecovery {
            interrupted_attempt: RepairAttemptId("missing".into()),
        },
    );
    let mut changed_profile = start(
        "recover2",
        RepairAttemptKind::AvailabilityRecovery {
            interrupted_attempt: RepairAttemptId("s0".into()),
        },
    );
    if let RepairEvent::Started { receipt } = &mut changed_profile {
        receipt.profile = "deep".into();
    }
    // When / Then
    rejected_without_change(&mut history, unknown);
    rejected_without_change(&mut history, changed_profile);
}

#[test]
fn findings_retain_fix_and_resolution_evidence_without_overwriting_history() {
    // Given
    let mut history = ready();
    history
        .apply(start("c1", RepairAttemptKind::CurrentProfileRepair))
        .unwrap();
    let before = history.events().to_vec();
    let mut resolved = finding();
    resolved.observation_id = "observation-2".into();
    resolved.matching = FindingMatch::Exact {
        finding: resolved.finding_id.clone(),
    };
    resolved.attempted_fixes.push(AttemptedFix {
        attempt: RepairAttemptId("c1".into()),
        change: "Commit both writes atomically".into(),
        evidence: vec!["diff:abc123".into()],
    });
    resolved.resolution = FindingResolution::Resolved {
        evidence: vec!["gate:abc123".into()],
    };
    // When
    history
        .apply(RepairEvent::Finding { finding: resolved })
        .unwrap();
    // Then
    assert_eq!(&history.events()[..before.len()], &before);
    rejected_without_change(
        &mut history,
        start("c2", RepairAttemptKind::CurrentProfileRepair),
    );
    let restored: RepairHistory =
        serde_json::from_str(&serde_json::to_string(&history).unwrap()).unwrap();
    assert_eq!(restored, history);
}

#[test]
fn semantic_and_uncertain_matches_remain_explicit_after_restart() {
    // Given
    let mut history = ready();
    let mut semantic = finding();
    semantic.observation_id = "observation-2".into();
    semantic.identity.failure_code = "partial_commit".into();
    semantic.fingerprint = semantic.identity.fingerprint();
    semantic.matching = FindingMatch::Semantic {
        finding: FindingId("F-1".into()),
        reason: "Both signatures describe the same partial commit".into(),
    };
    history
        .apply(RepairEvent::Finding { finding: semantic })
        .unwrap();
    let mut uncertain = finding();
    uncertain.observation_id = "observation-3".into();
    uncertain.finding_id = FindingId("F-2".into());
    uncertain.matching = FindingMatch::Uncertain {
        candidates: vec![FindingId("F-1".into())],
        reason: "Insufficient evidence".into(),
    };
    uncertain.resolution = FindingResolution::Uncertain {
        reason: "No verified result".into(),
        evidence: Vec::new(),
    };
    history
        .apply(RepairEvent::Finding { finding: uncertain })
        .unwrap();
    // When
    let restored: RepairHistory =
        serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
    // Then
    assert_eq!(restored.events(), history.events());
    assert_eq!(restored.counters().unwrap().coding_repairs, 0);
}

#[test]
fn exact_match_cannot_hide_a_changed_failure_identity() {
    // Given
    let mut history = ready();
    let mut changed = finding();
    changed.observation_id = "observation-2".into();
    changed.identity.failure_code = "new_failure".into();
    changed.fingerprint = changed.identity.fingerprint();
    changed.matching = FindingMatch::Exact {
        finding: changed.finding_id.clone(),
    };
    // When / Then
    rejected_without_change(&mut history, RepairEvent::Finding { finding: changed });
}

#[test]
fn progress_and_new_findings_do_not_reset_total_allowance() {
    // Given
    let mut history = exhausted();
    let mut different = finding();
    different.observation_id = "observation-2".into();
    different.finding_id = FindingId("F-2".into());
    different.identity.failure_code = "another_failure".into();
    different.fingerprint = different.identity.fingerprint();
    history
        .apply(RepairEvent::Finding { finding: different })
        .unwrap();
    let mut next = start("s3", RepairAttemptKind::StrongerProfileRepair);
    if let RepairEvent::Started { receipt } = &mut next {
        receipt.findings = vec![FindingId("F-2".into())];
    }
    // When / Then
    rejected_without_change(&mut history, next);
    assert_eq!(history.counters().unwrap().coding_repairs, 4);
}

#[test]
fn history_rejects_unknown_fields_and_invalid_snapshotted_policy() {
    // Given
    let mut unknown = serde_json::to_value(ready()).unwrap();
    unknown["coding_repairs"] = json!(0);
    let mut invalid_policy = serde_json::to_value(ready()).unwrap();
    invalid_policy["policy"]["pm_diagnosis_allowance"] = json!(2);
    // When / Then
    for value in [unknown, invalid_policy] {
        assert!(serde_json::from_value::<RepairHistory>(value).is_err());
    }
}

#[test]
fn history_round_trips_through_yaml_without_counter_fields() {
    // Given
    let history = exhausted();
    // When
    let yaml = serde_yaml_ng::to_string(&history).unwrap();
    let restored: RepairHistory = serde_yaml_ng::from_str(&yaml).unwrap();
    let value: Value = serde_json::to_value(&restored).unwrap();
    // Then
    assert_eq!(restored, history);
    assert_eq!(value.get("counters"), None);
    assert_eq!(restored.policy(), &RepairPolicy::approved_starter_policy());
}
