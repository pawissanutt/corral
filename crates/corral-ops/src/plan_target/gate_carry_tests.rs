use super::*;
use corral_core::{Status, Store};

#[test]
fn a_run_without_an_owned_plan_target_cannot_reach_the_gate_carry() {
    // Given a tiered, approved, main-bound run whose passing receipt carries a reduced binding.
    let dir = tempfile::tempdir().unwrap();
    let ctx = crate::resources::FixedResourceProbe::ctx(Store::init(dir.path()).unwrap());
    let task = ctx
        .store
        .create("Main-bound", Some("app"), Status::Review, "")
        .unwrap();
    let identity = json!({"version": 2, "task": task.id, "run": "R1", "attempt": "a1",
        "head": "h1", "tree": "t1", "integration_base": "b1", "dispatch_base": "b0",
        "spec_digest": "s", "policy_digest": "p", "inputs_digest": "i", "environment": [],
        "gate_tier": {"tier": "quick", "selection_sha256": "q", "source": "policy:review_rounds"}});
    let run: corral_core::Run = serde_json::from_value(json!({
        "id": "R1", "agent": "fixture", "worktree": dir.path().join("missing"),
        "branch": "corral/T-001", "base": "main", "base_sha": "b0", "gate_rules": "tiered",
        "started": "2026-10-19T00:00:00Z", "outcome": "done",
        "gate": {"identity": identity, "result": "pass", "at": "2026-10-19T00:01:00Z",
            "sha": "h1", "tier": {"tier": "quick", "context_sha256": "c"}},
        "review": {"reviewer": "rev", "sha": "h1", "verdict": "approve", "identity": identity,
            "at": "2026-10-19T00:02:00Z"},
    }))
    .unwrap();
    let task = ctx
        .store
        .update(&task.id, |task| {
            task.runs.push(run);
            Ok(())
        })
        .unwrap();
    let before = serde_json::to_vec(&task).unwrap();
    // When the carry path is reached, as after corral's replay onto a moved main.
    let carried = gate_carry(&ctx, &task.id).unwrap();
    // Then it refuses before reading Git, policy or evidence: nothing is rebound or logged.
    assert!(!carried);
    let after = ctx.store.get(&task.id).unwrap();
    assert_eq!(serde_json::to_vec(&after).unwrap(), before);
    assert!(
        ctx.store
            .events()
            .unwrap()
            .iter()
            .all(|event| event.kind != "gate_carried")
    );
}
