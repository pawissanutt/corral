use super::*;

#[test]
fn a_changed_member_selection_stales_its_receipts_of_every_tier() {
    let fx = fixture(true);
    let (task, tracker) = fx.member();
    let id = task.id;
    let quick = fx.receipt(&id, GatePhase::Review, "m1");
    assert!(fx.fresh(&id, &quick));
    fx.review(&id, Verdict::Approve, quick.identity.as_ref().unwrap());
    let (task, run) = fx.current(&id);
    assert!(verification::review_fresh(&fx.ctx, &task, &run_head(&run)).unwrap());
    // When the owner's record changes the member's selection, the quick receipt and its
    // review go stale.
    fx.amend(&tracker, &id, 1, FULL_TIER);
    assert!(!fx.fresh(&id, &quick));
    let (task, run) = fx.current(&id);
    assert!(!verification::review_fresh(&fx.ctx, &task, &run_head(&run)).unwrap());
    // A full member receipt is bound too, and the next change stales it despite its tier.
    let full = fx.receipt(&id, GatePhase::Review, "m2");
    let binding = full.identity.as_ref().unwrap().gate_tier.clone().unwrap();
    assert_eq!(
        (binding.tier.as_str(), binding.source),
        (FULL_TIER, format!("member-gate:{tracker}@1"))
    );
    assert!(fx.fresh(&id, &full));
    fx.amend(&tracker, &id, 2, "quick");
    assert!(!fx.fresh(&id, &full));
}

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
