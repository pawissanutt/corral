//! Plan-target gate carry: an approved, audited quick member replayed cleanly onto a sibling's
//! landing keeps its receipt, approval and criterion evidence, with no new gate or review.
use super::tiers::{events_of, runs, worker_round};
use super::*;
use std::collections::BTreeMap;

use corral_core::{CandidateIdentity, CriteriaManifest, GateRecord, GateRules, TierPolicy};
use serde_json::json;

const TARGET: &str = "feature/plan";
const TEMPLATE: &str =
    "{{id}} {{title}}\n{{body}}\nRun the gate below once after the final rebase.\n{{gate}}\n";

fn quick_command(out: &Path, variant: &str) -> String {
    format!("echo quick{variant} >> {}/runs", out.display())
}

fn configure(fx: &mut Fx, out: &Path, variant: &str) {
    fx.ctx.store.config.limits.running = 8;
    fx.ctx.store.config.limits.review = 3;
    let review = &mut fx.ctx.store.config.review;
    review.gate = Some(vec![format!("echo full >> {}/runs", out.display())]);
    review.gate_tiers = Some(BTreeMap::from([(
        "quick".to_owned(),
        vec![quick_command(out, variant)],
    )]));
    review.tier_policy = Some(TierPolicy {
        plan_member: Some("quick".into()),
        review_rounds: None,
    });
    fx.ctx.store.save_config().unwrap();
}

/// An integration Plan of three independent, audited quick members, each dispatched tiered.
fn plan(accept: Accept) -> (Fx, PathBuf, [String; 3]) {
    let mut fx = Fx::new(|r| {
        r.reviewer = Some("rev".into());
        r.accept = Some(accept);
        r.remove_worktree = Some(false);
    });
    fx.done();
    let out = fx._tmp.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::create_dir_all(fx.ctx.store.prompts_dir()).unwrap();
    std::fs::write(fx.ctx.store.prompts_dir().join("run.md"), TEMPLATE).unwrap();
    configure(&mut fx, &out, "");
    let source = fx.ctx.store.root().join("plan.md");
    std::fs::write(
        &source,
        r#"---
title: Integration Plan
project: app
approved_scope:
  goal: Ship A, B and C together
  acceptance: [{scenario: Combine changes, expected: All included}]
  delivery_target: {kind: integration, branch: feature/plan}
tasks:
  - {key: a, title: A, profile: fake}
  - {key: b, title: B, profile: fake}
  - {key: c, title: C, profile: fake}
---
"#,
    )
    .unwrap();
    let imported =
        corral_ops::import_plan(&fx.ctx, &source, false, corral_core::Origin::Owner).unwrap();
    let ids: [String; 3] = std::array::from_fn(|i| imported.tasks[i].id.clone());
    for id in &ids {
        let task = fx.ctx.store.get(id).unwrap();
        let manifest: CriteriaManifest = serde_json::from_value(json!({
            "version": 1, "spec_digest": corral_ops::completion_spec_digest(&task).unwrap(),
            "reviewed_by": "owner", "audited": true, "require_gate": true,
            "criteria": [{"id": "C1", "required": true, "live": false,
                "scenario": "the member works", "expected": "pass"}],
            "owner_decisions": [],
        }))
        .unwrap();
        corral_ops::register_criteria(&fx.ctx, id, &manifest).unwrap();
    }
    fx.grant(std::slice::from_ref(&imported.tracker.id));
    for id in &ids {
        fx.id = id.clone();
        assert!(matches!(
            corral_ops::dispatch(&fx.ctx, id, &DispatchOpts::default()).unwrap(),
            DispatchOutcome::Started { .. }
        ));
        assert_eq!(fx.run().gate_rules, GateRules::Tiered);
        assert!(fx.run().plan_target.is_some());
    }
    (fx, out, ids)
}

/// The worker's quick gate and evidence, then an independent approving review of that receipt.
fn approved(fx: &mut Fx, out: &Path, id: &str, round: usize) -> GateRecord {
    fx.id = id.into();
    let gate = worker_round(fx, out, round);
    let tier = gate.tier.as_ref().unwrap();
    assert_eq!(tier.tier, "quick");
    corral_ops::review_run(&fx.ctx, id, None).unwrap();
    fx.verdict(Verdict::Approve, "Member approved.");
    assert_eq!(fx.run().review.unwrap().identity, gate.identity);
    gate
}

fn ledger(fx: &Fx) -> Vec<u8> {
    serde_json::to_vec(&fx.run().completion.evidence).unwrap()
}

fn land(fx: &Fx, id: &str, strategy: ApproveStrategy) {
    corral_ops::approve(&fx.ctx, id, strategy, true, false).unwrap();
    assert_eq!(fx.ctx.store.get(id).unwrap().status, Status::Done);
}

/// One `gate_carried` from `old` with an evidence relation over the original source window,
/// then a `clean_replay` verdict carry, and a landing receipt with that provenance.
fn assert_carried(fx: &Fx, source: &GateRecord, carries: usize, ledger: &[u8]) {
    let origin = source.identity.clone().unwrap();
    let run = fx.run();
    let gate = run.gate.clone().unwrap();
    let new = gate.identity.clone().unwrap();
    assert_eq!(
        gate.at, source.at,
        "the carried receipt keeps its result time"
    );
    assert_eq!(new.attempt, origin.attempt);
    assert_eq!(new.gate_tier, origin.gate_tier);
    assert_ne!(new.head, origin.head);
    let carried = events_of(fx, "gate_carried");
    assert_eq!(carried.len(), carries, "{carried:?}");
    let last = carried.last().unwrap();
    assert_eq!(last["new_identity"], json!(new));
    assert_eq!(last["tier"], "quick");
    let relation = &last["evidence_carry"];
    assert_eq!(relation["method"], "clean_replay");
    assert_eq!(relation["from"], json!(origin));
    assert_eq!(relation["window_start"], json!(source.at));
    assert_eq!(relation["to"], json!(new));
    let tier = gate.tier.clone().unwrap();
    assert_eq!(json!(tier.carried), last["gate_carry"]);
    assert_eq!(json!(run.completion.evidence_carry), *relation);
    assert_eq!(
        serde_json::to_vec(&run.completion.evidence).unwrap(),
        ledger
    );
    let verdicts = events_of(fx, "verdict_carried");
    let verdict = verdicts.last().unwrap();
    assert_eq!(verdict["method"], "clean_replay");
    assert_eq!(verdict["new_identity"], json!(new));
    assert_eq!(verdict["evidence_carry"], *relation);
    assert_eq!(run.review.unwrap().identity.as_ref(), Some(&new));
    assert_eq!(events_of(fx, "gate_started").len(), 1);
    assert_eq!(events_of(fx, "review_started").len(), 1);
    let approved = events_of(fx, "approved");
    assert_eq!(approved.len(), 1);
    let landed = &approved[0];
    assert_eq!(landed["base"], TARGET);
    assert_eq!(landed["gate_tier"], json!(tier));
    assert_eq!(landed["evidence_carry"], *relation);
    assert_eq!(git(&fx.app, &["rev-parse", TARGET]), new.head);
}

fn head(fx: &Fx) -> String {
    git(&fx.run().worktree, &["rev-parse", "HEAD"])
}

#[test]
fn approved_member_replayed_onto_a_sibling_landing_carries_gate_approval_and_evidence() {
    // Given two audited members approved on their own quick receipts.
    let (mut fx, out, [a, b, _]) = plan(Accept::Owner);
    approved(&mut fx, &out, &a, 1);
    let source = approved(&mut fx, &out, &b, 2);
    let before = ledger(&fx);
    // When A lands on the Plan target and the owner approves B, which corral replays cleanly.
    land(&fx, &a, ApproveStrategy::FastForward);
    fx.id = b.clone();
    land(&fx, &b, ApproveStrategy::Rebase);
    // Then B's gate, approval and evidence are carried, with no new gate or review.
    assert_eq!(runs(&out), ["quick", "quick"]);
    assert_eq!(events_of(&fx, "candidate_rebased").len(), 1);
    assert_carried(&fx, &source, 1, &before);
    let main = git(&fx.app, &["rev-parse", "main"]);
    assert_ne!(main, git(&fx.app, &["rev-parse", TARGET]));
}

#[test]
fn autopilot_carries_the_replayed_member_and_lands_it_without_a_new_gate_or_review() {
    // Given two approved audited members and automatic acceptance.
    let (mut fx, out, [a, b, _]) = plan(Accept::Auto);
    approved(&mut fx, &out, &a, 1);
    let source = approved(&mut fx, &out, &b, 2);
    fx.id = b.clone();
    let before = ledger(&fx);
    land(&fx, &a, ApproveStrategy::FastForward);
    // When autopilot sees B behind the target it lands next.
    for _ in 0..8 {
        if fx.sync().status == Status::Done {
            break;
        }
    }
    // Then B lands on the target through the carry, with no second gate or review.
    assert_eq!(fx.task().status, Status::Done, "{}", fx.reason());
    assert_eq!(runs(&out), ["quick", "quick"]);
    assert_carried(&fx, &source, 1, &before);
}

#[test]
fn a_second_sibling_landing_chains_the_carry_from_the_original_source() {
    // Given three approved members; A then B land before C.
    let (mut fx, out, [a, b, c]) = plan(Accept::Owner);
    approved(&mut fx, &out, &a, 1);
    approved(&mut fx, &out, &b, 2);
    let source = approved(&mut fx, &out, &c, 3);
    fx.id = c.clone();
    let before = ledger(&fx);
    land(&fx, &a, ApproveStrategy::FastForward);
    // When C is replayed onto the target once per sibling landing: A, then B.
    fx.id = c.clone();
    for _ in 0..4 {
        if !events_of(&fx, "gate_carried").is_empty() {
            break;
        }
        fx.sync();
    }
    assert_eq!(events_of(&fx, "gate_carried").len(), 1);
    let first = fx.run().completion.evidence_carry.unwrap();
    land(&fx, &b, ApproveStrategy::Rebase);
    land(&fx, &c, ApproveStrategy::Rebase);
    // Then the second carry starts from the carried receipt and keeps the original window.
    assert_eq!(runs(&out), ["quick", "quick", "quick"]);
    let carried = events_of(&fx, "gate_carried");
    assert_eq!(carried[1]["old_identity"], json!(first.to));
    assert_eq!(json!(first.from), json!(source.identity));
    assert_carried(&fx, &source, 2, &before);
}

#[test]
fn a_restart_after_the_carry_event_rebinds_once_with_the_same_relation() {
    // Given B carried by autopilot after A landed.
    let (mut fx, out, [a, b, _]) = plan(Accept::Owner);
    approved(&mut fx, &out, &a, 1);
    approved(&mut fx, &out, &b, 2);
    land(&fx, &a, ApproveStrategy::FastForward);
    fx.id = b.clone();
    fx.sync();
    let event = events_of(&fx, "gate_carried").pop().expect("carry");
    assert_eq!(events_of(&fx, "candidate_rebased").len(), 1);
    // When corral restarts as if the save after that event was lost.
    let old: CandidateIdentity = serde_json::from_value(event["old_identity"].clone()).unwrap();
    fx.ctx
        .store
        .update(&b, |task| {
            let run = task.current_run_mut().unwrap();
            let gate = run.gate.as_mut().unwrap();
            gate.sha = old.head.clone();
            gate.identity = Some(old.clone());
            gate.tier.as_mut().unwrap().carried = None;
            run.completion.evidence_carry = None;
            let review = run.review.as_mut().unwrap();
            review.sha = old.head.clone();
            review.identity = Some(old.clone());
            Ok(())
        })
        .unwrap();
    // Then the unsaved receipt authorizes nothing: it no longer matches the replayed head.
    let unsaved = fx.run().gate.unwrap();
    assert_ne!(unsaved.sha, head(&fx));
    assert!(fx.run().completion.evidence_carry.is_none());
    fx.sync();
    // And the retry rebinds once, to the event's identity, carry and relation.
    assert_eq!(events_of(&fx, "gate_carried").len(), 1);
    let run = fx.run();
    let gate = run.gate.unwrap();
    assert_eq!(json!(gate.identity), event["new_identity"]);
    assert_eq!(json!(gate.tier.unwrap().carried), event["gate_carry"]);
    assert_eq!(
        json!(run.completion.evidence_carry),
        event["evidence_carry"]
    );
    assert_eq!(runs(&out), ["quick", "quick"]);
}

/// After A lands, B is replayed but must not be carried: a new member-tier gate runs on the
/// replayed head and no `gate_carried` event or relation exists.
fn assert_regated(fx: &Fx, out: &Path, quick: &str) {
    assert!(events_of(fx, "gate_carried").is_empty());
    assert!(fx.run().completion.evidence_carry.is_none());
    let gates = events_of(fx, "gate_started");
    assert_eq!(gates.len(), 2, "{gates:?}");
    assert_eq!(gates[1]["sha"], json!(head(fx)));
    assert_eq!(runs(out).last().map(String::as_str), Some(quick));
    assert!(!runs(out).iter().any(|run| run == "full"));
}

fn sync_until_regated(fx: &Fx) {
    for _ in 0..6 {
        fx.sync();
        if events_of(fx, "gate_started").len() > 1
            && fx.run().gate.unwrap().result != GateResult::Running
        {
            return;
        }
    }
}

#[test]
fn a_changed_patch_after_the_replay_runs_a_new_member_gate() {
    // Given B's approved patch includes a commit that A lands on the target.
    let (mut fx, out, [a, b, _]) = plan(Accept::Owner);
    fx.id = a.clone();
    fx.change("shared.txt", "shared\n");
    approved(&mut fx, &out, &a, 1);
    fx.id = b.clone();
    fx.change("shared.txt", "shared\n");
    corral_git::commit_all(&fx.run().worktree, "shared change").unwrap();
    approved(&mut fx, &out, &b, 2);
    land(&fx, &a, ApproveStrategy::FastForward);
    // When corral replays B, dropping the integrated commit.
    fx.id = b.clone();
    sync_until_regated(&fx);
    // Then nothing is carried and a new quick gate runs on the replayed head.
    assert_regated(&fx, &out, "quick");
    assert!(events_of(&fx, "verdict_carried").is_empty());
}

#[test]
fn a_changed_member_tier_after_approval_runs_a_new_member_gate() {
    let (mut fx, out, [a, b, _]) = plan(Accept::Owner);
    approved(&mut fx, &out, &a, 1);
    approved(&mut fx, &out, &b, 2);
    land(&fx, &a, ApproveStrategy::FastForward);
    // Given the quick tier's commands change after B's approval.
    configure(&mut fx, &out, "-v2");
    fx.id = b.clone();
    sync_until_regated(&fx);
    // Then the old member receipt cannot be carried under the new selection.
    assert_regated(&fx, &out, "quick-v2");
}

#[test]
fn carried_evidence_that_fails_refuses_the_carry_and_reports_the_failure() {
    let (mut fx, out, [a, b, _]) = plan(Accept::Owner);
    approved(&mut fx, &out, &a, 1);
    approved(&mut fx, &out, &b, 2);
    land(&fx, &a, ApproveStrategy::FastForward);
    // Given B's evidence artifact bytes edited after the receipt.
    fx.id = b.clone();
    let task = fx.task();
    let artifact = &task
        .artifacts
        .iter()
        .find(|artifact| artifact.title == "Widget scenario proof")
        .unwrap()
        .path;
    std::fs::write(fx.ctx.store.dir().join(artifact), "edited\n").unwrap();
    fx.id = b.clone();
    sync_until_regated(&fx);
    // Then no carry happens, a new member gate runs, and the failure is reported.
    assert_regated(&fx, &out, "quick");
    fx.sync();
    let failures = corral_ops::evidence_failures(&fx.ctx, &fx.task(), &fx.run()).unwrap();
    assert!(
        failures.iter().any(|f| f.starts_with("evidence_")),
        "{failures:?}"
    );
}

#[test]
fn a_main_bound_replay_never_reaches_the_gate_carry() {
    // Given a tiered main-bound approval replayed by corral after main moved.
    let (fx, out) = super::tiers::tiered(super::tiers::Mode::Quick, true, |r| {
        r.accept = Some(Accept::Auto)
    });
    let receipt = worker_round(&fx, &out, 1);
    fx.sync();
    fx.verdict(Verdict::Approve, "Ship it.");
    fx.commit_on_main("other.txt", "main moved\n", "main advanced");
    fx.sync();
    // Then the quick receipt is never rebound: the full landing gate runs on the replayed head.
    assert_eq!(events_of(&fx, "candidate_rebased").len(), 1);
    assert!(events_of(&fx, "gate_carried").is_empty());
    assert_eq!(runs(&out), ["quick", "full"]);
    let gate = fx.run().gate.unwrap();
    assert!(gate.tier.unwrap().carried.is_none());
    assert_ne!(gate.identity, receipt.identity);
}
