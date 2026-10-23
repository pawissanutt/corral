//! G6: an audited main-bound tiered task with `review_rounds = quick` runs the worker's quick gate
//! each review round and one full gate per landing head, and the tier-aware promotion carries the
//! approval and the evidence so it lands on main without a new review.
use super::*;
use corral_core::{GateRules, Status};
use corral_ops::{DispatchOpts, DispatchOutcome};

fn landed_after_one_changes_round(main_moves: bool) {
    // Given an audited main-bound task dispatched into the tiered project.
    let mut fx = Fx::new(Suite::Green, |_| {});
    let id = fx
        .ctx
        .store
        .create(
            "Ship the widget",
            Some("app"),
            Status::Ready,
            "## Goal\nShip it.\n",
        )
        .unwrap()
        .id;
    fx.audit(&id);
    let opts = DispatchOpts {
        profile: Some("fake".into()),
        ..DispatchOpts::default()
    };
    let started = corral_ops::dispatch(&fx.ctx, &id, &opts).unwrap();
    assert!(matches!(started, DispatchOutcome::Started { .. }));
    assert_eq!(fx.run(&id).gate_rules, GateRules::Tiered);
    fx.grant(&[&id]);

    // When it goes through one changes round and an approval.
    let summary =
        "Shipped the widget.\nVerified: corral's receipted quick gate passed\nNot verified: none\n";
    let first = [("widget.txt", "v1\n")];
    let second = [("widget.txt", "v2\n")];
    let round = |files| Round { files, summary };
    work(&fx, &id, "quick", &round(&first));
    fx.sync();
    review(
        &fx,
        &id,
        "quick",
        Verdict::Changes,
        "Round 1: handle the empty widget.",
    );
    fx.sync();
    assert_eq!(
        fx.task(&id).status,
        Status::Running,
        "{:?}",
        fx.run(&id).autopilot
    );
    let approved = work(&fx, &id, "quick", &round(&second));
    fx.sync();
    review(&fx, &id, "quick", Verdict::Approve, "Ship it.");
    let ledger = serde_json::to_vec(&fx.run(&id).completion.evidence).unwrap();
    let before = git(&fx.app, &["rev-parse", "main"]);
    if main_moves {
        fx.commit_on_main("other.txt", "main moved\n");
    }
    for _ in 0..3 {
        if fx.task(&id).status != Status::Done {
            fx.sync();
        }
    }

    // Then the rounds ran quick, the landing head one full gate, and no review was added.
    assert_eq!(
        fx.task(&id).status,
        Status::Done,
        "{:?}",
        fx.run(&id).autopilot
    );
    assert_eq!(fx.tiers_run(), ["quick", "quick", "full"]);
    let tiers: Vec<Value> = fx
        .events(&id, "gate_started")
        .iter()
        .map(|e| e["tier"].clone())
        .collect();
    assert_eq!(tiers, [json!("quick"), json!("quick"), json!("full")]);
    assert_eq!(fx.events(&id, "review_started").len(), 2);
    assert_eq!(fx.events(&id, "evidence_recorded").len(), 2);
    assert_eq!(
        fx.events(&id, "candidate_rebased").len(),
        usize::from(main_moves)
    );
    let run = fx.run(&id);
    let gate = run.gate.clone().unwrap();
    let full = gate.identity.clone().unwrap();
    assert_eq!(gate.tier.unwrap().tier, "full");
    assert_eq!(full.gate_tier, None);
    let main = git(&fx.app, &["rev-parse", "main"]);
    assert_eq!(main, gate.sha, "main received the full PASS head");
    assert_eq!(fx.events(&id, "gate_started")[2]["sha"], json!(main));
    assert_ne!(main, before);

    // And the tier-aware promotion carried the approval and evidence onto that receipt.
    let method = if main_moves {
        "clean_replay+tier_promotion"
    } else {
        "tier_promotion"
    };
    let carried = fx.events(&id, "verdict_carried");
    assert_eq!(carried.len(), 1, "{carried:?}");
    assert_eq!(
        (
            &carried[0]["method"],
            &carried[0]["from_tier"],
            &carried[0]["to_tier"]
        ),
        (&json!(method), &json!("quick"), &json!("full"))
    );
    assert_eq!(carried[0]["old_identity"], json!(approved.identity));
    let relation = &carried[0]["evidence_carry"];
    assert_eq!(relation["from"], json!(approved.identity));
    assert_eq!(relation["window_start"], json!(approved.at));
    assert_eq!(relation["to"], json!(full));
    assert_eq!(
        serde_json::to_vec(&run.completion.evidence).unwrap(),
        ledger
    );
    assert_eq!(run.review.unwrap().identity, Some(full));
    assert_eq!(
        fx.events(&id, "approved").pop().unwrap()["acceptance_mode"],
        "autonomous"
    );
}

#[test]
fn quick_review_rounds_then_one_full_landing_gate_promote_the_approval_on_a_still_main() {
    landed_after_one_changes_round(false);
}

#[test]
fn after_main_moves_one_replay_and_one_full_gate_promote_the_approval() {
    landed_after_one_changes_round(true);
}
