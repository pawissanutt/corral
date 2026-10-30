//! `corral plan member-gate` against dispatched members and real gates.
use corral_core::GateResult;
use corral_ops::{MemberGateRequest, OwnerEntry};

use super::{Fx, full_command, quick_command};

fn amend(
    fx: &Fx,
    member: &str,
    expected_revision: u64,
    yaml: &str,
) -> corral_ops::MemberGateOutcome {
    let file = fx.out.join("gate.yaml");
    std::fs::write(&file, yaml).unwrap();
    let entry = OwnerEntry {
        agent: false,
        pm: false,
        cwd: fx.ctx.store.root().to_path_buf(),
    };
    let request = MemberGateRequest {
        member,
        tracker: None,
        file: &file,
        expected_revision,
        reason: "owner decision",
    };
    corral_ops::plan_member_gate_from(&fx.ctx, &request, &entry).unwrap()
}

#[test]
fn amended_member_regates_on_its_new_binding_while_the_running_sibling_stays_fresh() {
    let fx = Fx::new(true, |_| {});
    let [a, b] = fx.plan();
    for id in [&a, &b] {
        fx.dispatch(id).unwrap();
        fx.commit(id, "work.txt");
    }
    // Given the sibling's passing member gate.
    let sibling = fx.gate(&b);
    assert_eq!(sibling.result, GateResult::Pass, "{:?}", sibling.failed_cmd);
    let ready = corral_ops::review_info(&fx.ctx, &b).unwrap().main_ready;
    // When the owner amends the first member to full.
    let first = amend(&fx, &a, 0, "tier: full\n");
    let tracker = first.tracker.clone();
    assert_eq!(
        fx.run(&a).completion.required_checks,
        Some(vec![full_command(&fx.out)])
    );
    // And amends it back to quick while its full gate is running.
    let running = fx.start(&a);
    assert_eq!(running.result, GateResult::Running);
    let full = running.identity.unwrap().gate_tier.unwrap();
    assert_eq!(
        (full.tier.as_str(), full.source),
        ("full", format!("member-gate:{tracker}@1"))
    );
    amend(&fx, &a, 1, "tier: quick\ntests: [\"package(app)\"]\n");
    // Then the running gate is collected as stale.
    let collected = corral_ops::gate_collect(&fx.ctx, &a).unwrap().unwrap();
    assert_eq!(collected.result, GateResult::Stale);
    let reason = collected.failed_cmd.unwrap_or_default();
    assert!(reason.starts_with("stale_result"), "{reason}");
    // And the member's next gate runs the quick commands on the new binding.
    let next = fx.gate(&a);
    assert_eq!(next.result, GateResult::Pass, "{:?}", next.failed_cmd);
    let binding = next.identity.unwrap().gate_tier.unwrap();
    assert_eq!(
        (binding.tier.as_str(), binding.source),
        ("quick", format!("member-gate:{tracker}@2"))
    );
    assert_eq!(
        fx.run(&a).completion.required_checks,
        Some(vec![quick_command(&fx.out)])
    );
    // The sibling's receipt still collects as a pass with a byte-equal identity.
    let again = corral_ops::gate_collect(&fx.ctx, &b).unwrap().unwrap();
    assert_eq!(again.result, GateResult::Pass, "{:?}", again.failed_cmd);
    assert_eq!(
        serde_json::to_vec(&again.identity).unwrap(),
        serde_json::to_vec(&sibling.identity).unwrap()
    );
    assert_eq!(
        corral_ops::review_info(&fx.ctx, &b).unwrap().main_ready,
        ready
    );
    assert_eq!(fx.events(&tracker, "plan_member_gate_amended").len(), 2);
}
