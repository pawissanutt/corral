use super::*;

#[test]
fn full_member_receipts_keep_selection_freshness() {
    let fx = fixture(true);
    let (task, tracker) = fx.member();
    let id = task.id;
    let (task, run) = fx.current(&id);
    let run = corral_core::RunHead::of(&run).unwrap();
    let requirement = select(&fx.ctx, &task, &run, GatePhase::Review)
        .unwrap()
        .requirement;
    let full_binding = requirement.full_binding.clone().unwrap();
    assert!(satisfies(Some(&full_binding), &requirement));
    let mut other = full_binding.clone();
    other.selection_sha256 = "different".into();
    assert!(!satisfies(Some(&other), &requirement));
    let mut full = fx.receipt(&id, GatePhase::Review, "full");
    full.identity =
        Some(verification::capture_identity_with(&fx.ctx, &task, &run, "full", None).unwrap());
    assert!(verification::gate_fresh_for(&fx.ctx, &task, &run, &full, &requirement).unwrap());
    fx.amend(&tracker, &id, 1, "quick");
    let (task, run) = fx.current(&id);
    let run = corral_core::RunHead::of(&run).unwrap();
    let amended = select(&fx.ctx, &task, &run, GatePhase::Review)
        .unwrap()
        .requirement;
    assert!(!satisfies(Some(&full_binding), &amended));
    assert!(!verification::gate_fresh_for(&fx.ctx, &task, &run, &full, &amended).unwrap());
}
