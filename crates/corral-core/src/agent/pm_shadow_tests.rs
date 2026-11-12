use super::*;

fn decision() -> PmShadowDecision {
    PmShadowDecision {
        id: String::new(),
        at: crate::clock::now(),
        generation: 1,
        project: Some("p".into()),
        task: Some("T-1".into()),
        kind: "wait".into(),
        digest: "a".repeat(64),
        policy_revision: "policy".into(),
        model: "test/cheap".into(),
        choice: "wait".into(),
        requested_escalation: false,
        reason: None,
        code_recommendation: Some("wait".into()),
        agrees: Some(true),
        stale: false,
        baseline_required: false,
        baseline_reasons: Vec::new(),
        escalation_required: false,
        escalation_reasons: Vec::new(),
    }
}

#[test]
fn missing_shadow_area_loads_empty() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let shadow = load(&store).unwrap();
    assert_eq!(shadow.counters, PmShadowCounters::default());
    assert!(shadow.decisions.is_empty());
}

#[test]
fn ring_drops_oldest_without_resetting_cumulative_counters() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let first = record(&store, |_| Ok(decision())).unwrap();
    assert_eq!(first.id, "shadow-1");
    let mut shadow = load(&store).unwrap();
    for _ in 0..MAX_DECISIONS + 1 {
        shadow.push(decision()).unwrap();
    }
    AREA.save(&store, SCOPE, &shadow).unwrap();
    let saved = load(&store).unwrap();
    assert_eq!(saved.decisions.len(), MAX_DECISIONS);
    assert_eq!(saved.decisions[0].id, "shadow-3");
    assert_eq!(saved.counters.total, MAX_DECISIONS as u64 + 2);
    assert_eq!(saved.counters.agree, saved.counters.total);
}

#[test]
fn missed_and_needless_escalations_are_separate_and_stale_is_unscored() {
    let mut shadow = PmShadow::default();
    let mut missed = decision();
    missed.baseline_required = true;
    missed.escalation_required = true;
    shadow.push(missed).unwrap();
    let mut needless = decision();
    needless.choice = "escalate".into();
    needless.agrees = Some(false);
    needless.escalation_required = true;
    shadow.push(needless).unwrap();
    let mut requested = decision();
    requested.requested_escalation = true;
    shadow.push(requested).unwrap();
    let before = shadow.counters.clone();
    let mut stale = decision();
    stale.stale = true;
    stale.baseline_required = true;
    let stored = shadow.push(stale).unwrap();
    assert_eq!(stored.agrees, None);
    assert_eq!(
        shadow.counters,
        PmShadowCounters {
            total: 4,
            stale: 1,
            ..before
        }
    );
    assert_eq!(shadow.counters.missed_escalation, 1);
    assert_eq!(shadow.counters.needless_escalation, 2);
    assert_eq!(shadow.counters.escalate_requested, 2);
    assert_eq!(shadow.counters.escalation_required, 1);
    assert_eq!(shadow.repeats(&"a".repeat(64), 1), 3);
    assert_eq!(shadow.repeats(&"a".repeat(64), 2), 0);
}
