use super::*;

#[test]
fn retained_coordination_decision_history_and_corruption_are_not_fabricated_success() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(corral_core::Store::init(tmp.path()).unwrap());
    let mut action = crate::pm_next::pm_select(&[], "corral");
    action.kind = PmActionKind::Owner;
    action.why = "Confirm the bounded workspace scope".into();
    let decision = PmDecision {
        id: "decision-exact".into(),
        task: "workspace".into(),
        run: None,
        source: "workspace".into(),
        request_digest: "request-exact".into(),
        action,
        presentation: None,
    };
    let mut records = Decisions::default();
    records.records.insert(
        decision.id.clone(),
        Record {
            decision,
            answer: None,
        },
    );
    save(&ctx, &records).unwrap();
    let before = ctx
        .store
        .record::<serde_json::Value>("pm-decisions", "state")
        .unwrap();
    let pending = inbox(&ctx, None, &[], "corral").unwrap().pop().unwrap();
    assert!(pending.pending);
    assert_eq!(pending.key, "decision-exact");
    assert_eq!(pending.scope, "workspace");
    assert!(
        pending
            .response
            .unwrap()
            .contains("pm resolve 'decision-exact'")
    );
    assert_eq!(
        ctx.store
            .record::<serde_json::Value>("pm-decisions", "state")
            .unwrap(),
        before
    );
    assert!(
        inbox(&ctx, Some("other"), &[], "corral")
            .unwrap()
            .is_empty()
    );
    records.records.get_mut("decision-exact").unwrap().answer = Some("Approved scope only".into());
    save(&ctx, &records).unwrap();
    let resolved = inbox(&ctx, None, &[], "corral").unwrap().pop().unwrap();
    assert_eq!(resolved.state, "resolved");
    assert!(!resolved.pending);
    assert!(resolved.response.is_none());
    assert_ne!(resolved.revision, pending.revision);
    assert!(
        resolved
            .detail
            .iter()
            .any(|line| line.contains("Approved scope only"))
    );
    ctx.store
        .save_record("pm-decisions", "state", &"not a decision document")
        .unwrap();
    assert!(
        inbox(&ctx, None, &[], "corral")
            .unwrap_err()
            .to_string()
            .contains("decoding record pm-decisions/state")
    );
}
