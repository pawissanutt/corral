//! Re-reviewing an approved quick candidate builds its packet in the review gate phase, so the
//! packet's gate evidence and gaps agree and the reviewer's verdict is not stale on its digest.
use corral_core::{GateResult, Status, Verdict};
use serde_json::{Value, json};

use super::Fx;

fn packet(fx: &Fx, id: &str) -> Value {
    let run = fx.run(id);
    let attempt = &run.review_runs.last().unwrap().id;
    let path = fx
        .ctx
        .store
        .prompts_dir()
        .join(format!("{id}-{}-{attempt}-review-packet.json", run.id));
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

/// Given a passing quick receipt approved by an earlier review, when a new review launches and
/// its reviewer approves, the packet reports the receipt fresh with no gate gap and the verdict
/// is recorded.
fn re_review_after_quick_approval(fx: &Fx, id: &str) {
    fx.commit(id, "a.txt");
    let gate = fx.gate(id);
    assert_eq!(gate.result, GateResult::Pass, "{:?}", gate.failed_cmd);
    assert_eq!(gate.tier.as_ref().unwrap().tier, "quick");
    fx.review(id, "approve", gate.identity.as_ref().unwrap());
    fx.ctx
        .store
        .update(id, |task| {
            task.status = Status::Review;
            Ok(())
        })
        .unwrap();

    corral_ops::review_run(&fx.ctx, id, Some("fake")).unwrap();

    let packet = packet(fx, id);
    let bound = &packet["bound"];
    assert_eq!(bound["evidence"]["gate"]["fresh"], true, "{bound:#}");
    assert_eq!(bound["evidence"]["reusable"], json!(["gate"]), "{bound:#}");
    assert_eq!(bound["gaps"], json!([]), "{bound:#}");
    let receipts = bound["gate_contract"]["receipts"].as_array().unwrap();
    let required: Vec<_> = receipts.iter().filter(|r| r["required"] == true).collect();
    assert_eq!(required.len(), 1, "{bound:#}");
    assert_eq!(required[0]["tier"], "quick", "{bound:#}");
    assert_eq!(required[0]["fresh"], true, "{bound:#}");
    let record = corral_ops::verdict(&fx.ctx, id, Verdict::Approve, "re-reviewed").unwrap();
    assert_eq!(record.verdict, Some(Verdict::Approve), "{:?}", record.notes);
}

#[test]
fn re_review_of_an_approved_main_bound_quick_candidate_uses_the_review_phase() {
    let fx = Fx::new(true, |_| {});
    let id = fx.main_bound();
    re_review_after_quick_approval(&fx, &id);
}

#[test]
fn re_review_of_an_approved_quick_plan_member_is_unchanged() {
    let fx = Fx::new(true, |_| {});
    let [member, _] = fx.plan();
    fx.dispatch(&member).unwrap();
    re_review_after_quick_approval(&fx, &member);
}
