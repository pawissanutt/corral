use corral_core::{FULL_TIER, GateRecord, GateRules, Origin, Run, Verdict};

use super::*;
use crate::gate_tier::tests::{Fx, fixture};
use crate::gate_tier::{GatePhase, select};
use crate::verification;

const FULL: &str = "full-gate";
const QUICK: &str = "quick-gate";
const SUMMARY: &str = "Done.\nVerified: gate\nNot verified: none\n";

fn owner(fx: &Fx) -> OwnerEntry {
    OwnerEntry {
        agent: false,
        pm: false,
        cwd: fx.repo.parent().unwrap().to_path_buf(),
    }
}

fn amend_as(
    fx: &Fx,
    member: &str,
    tracker: Option<&str>,
    expected_revision: u64,
    yaml: &str,
    entry: &OwnerEntry,
) -> Result<MemberGateOutcome> {
    let file = fx.repo.parent().unwrap().join("gate.yaml");
    std::fs::write(&file, yaml).unwrap();
    plan_member_gate_from(
        &fx.ctx,
        &MemberGateRequest {
            member,
            tracker,
            file: &file,
            expected_revision,
            reason: "owner decision",
        },
        entry,
    )
}

fn amend(fx: &Fx, member: &str, expected_revision: u64, yaml: &str) -> Result<MemberGateOutcome> {
    amend_as(fx, member, None, expected_revision, yaml, &owner(fx))
}

fn amended_events(fx: &Fx) -> Vec<serde_json::Value> {
    let events = fx.ctx.store.events().unwrap();
    let amended = events
        .into_iter()
        .filter(|e| e.kind == "plan_member_gate_amended");
    amended.map(|e| e.detail).collect()
}

/// An approved review-phase receipt for `id`, as autopilot would leave it.
fn approved(fx: &Fx, id: &str, attempt: &str) -> GateRecord {
    let gate = fx.receipt(id, GatePhase::Review, attempt);
    fx.review(id, Verdict::Approve, gate.identity.as_ref().unwrap());
    gate
}

fn fresh_receipts(fx: &Fx, id: &str, gate: &GateRecord) -> (bool, bool) {
    let (task, run) = fx.current(id);
    let review = verification::review_fresh(&fx.ctx, &task, &run).unwrap();
    (fx.fresh(id, gate), review)
}

/// The receipt identity re-derived under current inputs with the sibling's own binding.
fn rederived(fx: &Fx, id: &str, gate: &GateRecord) -> Vec<u8> {
    let (task, run) = fx.current(id);
    let attempt = &gate.identity.as_ref().unwrap().attempt;
    let binding = gate.identity.as_ref().unwrap().gate_tier.clone();
    let identity =
        verification::capture_identity_with(&fx.ctx, &task, &run, attempt, binding).unwrap();
    serde_json::to_vec(&identity).unwrap()
}

fn target_valid(fx: &Fx, id: &str) -> bool {
    let (task, run) = fx.current(id);
    crate::plan_target::validate_run(&fx.ctx.store, &task, &run)
        .unwrap()
        .is_some()
}

#[test]
fn an_owner_amendment_records_a_revision_and_reconciles_only_the_amended_member() {
    let fx = fixture(true);
    let (members, tracker) = fx.members(2);
    let (a, b) = (members[0].id.clone(), members[1].id.clone());
    // Given both members running and the sibling holding an approved receipt.
    let sibling = approved(&fx, &b, "b1");
    let sibling_run = serde_json::to_vec(&fx.current(&b).1).unwrap();
    assert!(target_valid(&fx, &b));
    // When the owner amends the first member from quick to full at revision 0.
    let outcome = amend(&fx, &a, 0, "tier: full\n").unwrap();
    // Then the tracker records revision 1 and one event names the reconciled run.
    let source = format!("member-gate:{tracker}@1");
    assert_eq!(
        (outcome.record.revision, outcome.source.as_str()),
        (1, source.as_str())
    );
    assert_eq!(outcome.reconciled_run.as_deref(), Some("R1"));
    let saved = fx.ctx.store.get(&tracker).unwrap();
    assert_eq!(saved.member_gates[&a], outcome.record);
    assert!(outcome.record.history.is_empty());
    let events = amended_events(&fx);
    assert_eq!(events.len(), 1, "{events:?}");
    let event = &events[0];
    assert_eq!(
        (&event["tracker"], &event["member"], &event["actor"]),
        (&tracker.clone().into(), &a.clone().into(), &"owner".into())
    );
    assert_eq!(
        (&event["from_revision"], &event["revision"]),
        (&0.into(), &1.into())
    );
    assert_eq!(
        (&event["gate"]["tier"], &event["reconciled_run"]),
        (&FULL_TIER.into(), &"R1".into())
    );
    // And the member's next gate binds the new source; its snapshot names the new commands.
    let (task, run) = fx.current(&a);
    let contract = select(&fx.ctx, &task, &run, GatePhase::Review).unwrap();
    assert_eq!(
        (contract.tier.as_str(), contract.source.as_str()),
        (FULL_TIER, source.as_str())
    );
    assert_eq!(contract.binding().unwrap().source, source);
    assert_eq!(run.completion.required_checks, Some(vec![FULL.to_owned()]));
    // The sibling's run, frozen target binding and receipt identity are untouched.
    assert_eq!(serde_json::to_vec(&fx.current(&b).1).unwrap(), sibling_run);
    assert!(target_valid(&fx, &b));
    assert_eq!(
        rederived(&fx, &b, &sibling),
        serde_json::to_vec(sibling.identity.as_ref().unwrap()).unwrap()
    );
    assert_eq!(fresh_receipts(&fx, &b, &sibling), (true, true));
    // A second amendment appends the superseded revision to the history.
    let second = amend(&fx, &a, 1, "tier: quick\ntests: [\"package(app)\"]\n").unwrap();
    assert_eq!(second.record.revision, 2);
    assert_eq!(second.record.history, [outcome.record]);
    assert_eq!(second.source, format!("member-gate:{tracker}@2"));
    let (_, run) = fx.current(&a);
    assert_eq!(run.completion.required_checks, Some(vec![QUICK.to_owned()]));
}

#[test]
fn every_amendment_stales_the_members_receipts_of_every_tier_and_nothing_else() {
    let fx = fixture(true);
    let (members, _) = fx.members(2);
    let (a, b) = (members[0].id.clone(), members[1].id.clone());
    let sibling = approved(&fx, &b, "b1");
    let failure = "expected_failures: [{test: \"app::suite slow\", reason: restored by b}]";
    // The member starts on quick through tier_policy.plan_member.
    for (step, (yaml, tier)) in [
        ("tier: full\n".to_owned(), "quick"),
        ("tier: quick\n".to_owned(), FULL_TIER),
        (
            "tier: quick\ntests: [\"package(app)\"]\n".to_owned(),
            "quick",
        ),
        (
            format!("tier: quick\ntests: [\"package(app)\"]\n{failure}\n"),
            "quick",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let receipt = approved(&fx, &a, &format!("m{step}"));
        let binding = receipt
            .identity
            .as_ref()
            .unwrap()
            .gate_tier
            .clone()
            .unwrap();
        assert_eq!(binding.tier, tier, "step {step}");
        assert_eq!(
            fresh_receipts(&fx, &a, &receipt),
            (true, true),
            "step {step}"
        );
        amend(&fx, &a, step as u64, &yaml).unwrap();
        // Then the member's gate and review receipts are stale, whatever their tier.
        assert_eq!(
            fresh_receipts(&fx, &a, &receipt),
            (false, false),
            "step {step}"
        );
        assert_eq!(
            fresh_receipts(&fx, &b, &sibling),
            (true, true),
            "step {step}"
        );
    }
}

#[test]
fn a_legacy_member_run_keeps_its_full_receipt_and_snapshot() {
    let fx = fixture(true);
    let (members, _) = fx.members(1);
    let id = members[0].id.clone();
    fx.ctx
        .store
        .update(&id, |task| {
            let run = task.current_run_mut().unwrap();
            run.gate_rules = GateRules::Legacy;
            run.completion.required_checks = Some(vec![FULL.to_owned()]);
            Ok(())
        })
        .unwrap();
    let receipt = approved(&fx, &id, "l1");
    assert_eq!(receipt.identity.as_ref().unwrap().gate_tier, None);
    let outcome = amend(&fx, &id, 0, "tier: quick\ntests: [\"package(app)\"]\n").unwrap();
    assert_eq!(outcome.reconciled_run, None);
    assert_eq!(fresh_receipts(&fx, &id, &receipt), (true, true));
    let (_, run) = fx.current(&id);
    assert_eq!(run.completion.required_checks, Some(vec![FULL.to_owned()]));
}

fn audited(fx: &Fx, id: &str) {
    let task = fx.ctx.store.get(id).unwrap();
    let manifest: corral_core::CriteriaManifest = serde_json::from_value(serde_json::json!({
        "version": 1, "spec_digest": crate::completion_spec_digest(&task).unwrap(),
        "reviewed_by": "owner", "audited": true, "require_gate": true,
        "criteria": [{"id": "C1", "required": true, "live": false,
            "scenario": "s", "expected": "e"}],
        "owner_decisions": [],
    }))
    .unwrap();
    crate::register_criteria(&fx.ctx, id, &manifest).unwrap();
}

fn policy_changed(fx: &Fx, id: &str) -> bool {
    let task = fx.ctx.store.get(id).unwrap();
    let audit = crate::completion_check_task(&fx.ctx, &task, SUMMARY).unwrap();
    audit
        .failures
        .iter()
        .any(|f| f == "completion_policy_changed")
}

#[test]
fn a_no_op_is_refused_and_a_retry_after_a_crash_before_the_tracker_save_completes_once() {
    let fx = fixture(true);
    let (members, tracker) = fx.members(1);
    let id = members[0].id.clone();
    audited(&fx, &id);
    // A gate equal to the member's current one is refused.
    let error = format!("{:#}", amend(&fx, &id, 0, "tier: quick\n").unwrap_err());
    assert!(error.contains("no-op member-gate amendment"), "{error}");
    assert!(amended_events(&fx).is_empty());
    assert!(!policy_changed(&fx, &id));
    // Given a crash after the member snapshot step and before the tracker save.
    let full = ScopeGate {
        tier: FULL_TIER.into(),
        tests: Vec::new(),
        expected_failures: Vec::new(),
    };
    fx.ctx
        .store
        .update(&id, |task| {
            reconcile_snapshot(&fx.ctx.store, task, &full)?;
            Ok(())
        })
        .unwrap();
    // Then done refuses until the tracker save lands.
    assert!(policy_changed(&fx, &id));
    assert!(fx.ctx.store.get(&tracker).unwrap().member_gates.is_empty());
    // When the owner retries with the same expected revision, it completes once.
    let outcome = amend(&fx, &id, 0, "tier: full\n").unwrap();
    assert_eq!(
        (outcome.record.revision, outcome.reconciled_run.as_deref()),
        (1, Some("R1"))
    );
    assert!(!policy_changed(&fx, &id));
    let error = format!("{:#}", amend(&fx, &id, 0, "tier: full\n").unwrap_err());
    assert!(error.contains("stale member-gate amendment"), "{error}");
    assert_eq!(amended_events(&fx).len(), 1);
}

#[test]
fn pm_can_amend_member_gate() {
    let fx = fixture(true);
    let (members, _) = fx.members(1);
    let entry = OwnerEntry {
        pm: true,
        agent: true,
        ..owner(&fx)
    };
    let outcome = amend_as(&fx, &members[0].id, None, 0, "tier: full\n", &entry).unwrap();
    assert_eq!(outcome.record.by, "pm");
    assert_eq!(outcome.record.gate.tier, "full");
}

#[test]
fn non_owner_stale_main_delivery_foreign_and_generic_amendments_write_nothing() {
    let fx = fixture(true);
    let (members, tracker) = fx.members(1);
    let id = members[0].id.clone();
    let other = fx.repo.parent().unwrap().join("main-plan.md");
    std::fs::write(&other, "---\ntitle: Main Plan\nproject: app\napproved_scope:\n  goal: Ship\n  acceptance: [{scenario: s, expected: e}]\ntasks:\n  - {key: one, title: One}\n  - {key: two, title: Two}\n---\n").unwrap();
    let main_plan = crate::import_plan(&fx.ctx, &other, false, Origin::Agent).unwrap();
    let before = serde_json::to_vec(&fx.current(&id).1).unwrap();
    let worktree: Run = fx.current(&id).1;
    let entry = |agent, pm| OwnerEntry {
        agent,
        pm,
        cwd: owner(&fx).cwd,
    };
    let yaml = "tier: full\n";
    for (name, result, reason) in [
        (
            "agent",
            amend_as(&fx, &id, None, 0, yaml, &entry(true, false)),
            "PM or agent",
        ),
        (
            "run worktree",
            amend_as(
                &fx,
                &id,
                None,
                0,
                yaml,
                &OwnerEntry {
                    cwd: worktree.worktree.clone(),
                    ..entry(false, false)
                },
            ),
            "agent run worktree",
        ),
        (
            "stale",
            amend(&fx, &id, 3, yaml),
            "stale member-gate amendment",
        ),
        (
            "main delivery",
            amend(&fx, &main_plan.tasks[0].id, 0, yaml),
            "member gates apply only to integration Plans",
        ),
        (
            "other tracker",
            amend_as(&fx, &id, Some(&main_plan.tracker.id), 0, yaml, &owner(&fx)),
            "is a member of",
        ),
    ] {
        let error = format!("{:#}", result.unwrap_err());
        assert!(error.contains(reason), "{name}: {error}");
    }
    // A generic edit of member_gates is refused at the store boundary.
    let record = corral_core::MemberGateRecord {
        revision: 1,
        gate: ScopeGate {
            tier: FULL_TIER.into(),
            tests: Vec::new(),
            expected_failures: Vec::new(),
        },
        reason: "bypass".into(),
        by: "owner".into(),
        at: corral_core::clock::now(),
        history: Vec::new(),
    };
    let error = fx
        .ctx
        .store
        .update(&tracker, |task| {
            task.member_gates.insert(id.clone(), record.clone());
            Ok(())
        })
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("corral plan member-gate"),
        "{error:#}"
    );
    // Nothing was written.
    for plan in [&tracker, &main_plan.tracker.id] {
        assert!(fx.ctx.store.get(plan).unwrap().member_gates.is_empty());
    }
    assert!(amended_events(&fx).is_empty());
    assert_eq!(serde_json::to_vec(&fx.current(&id).1).unwrap(), before);
}

#[test]
fn main_landing_and_aggregate_gates_stay_full_after_expected_failures_are_added() {
    let fx = fixture(true);
    let (members, tracker) = fx.members(1);
    let id = members[0].id.clone();
    let yaml =
        "tier: quick\nexpected_failures: [{test: \"app::suite slow\", reason: restored by b}]\n";
    amend(&fx, &id, 0, yaml).unwrap();
    let (task, run) = fx.current(&id);
    let member = select(&fx.ctx, &task, &run, GatePhase::Review).unwrap();
    assert_eq!(member.expected_failures.len(), 1);
    // A main-bound landing and the aggregate are full with an empty expected-failure set.
    let main = fx.main_bound();
    let landing = select(
        &fx.ctx,
        &main,
        main.current_run().unwrap(),
        GatePhase::Landing,
    )
    .unwrap();
    let store = fx.ctx.store.reopen().unwrap();
    let tracker = store.get(&tracker).unwrap();
    let policy = store.config.task_review_policy(&tracker);
    let aggregate = crate::gate_tier::aggregate(&store.config, &tracker, &policy);
    for contract in [landing, aggregate] {
        assert_eq!(contract.tier, FULL_TIER);
        assert_eq!(contract.commands, [FULL]);
        assert!(contract.expected_failures.is_empty() && contract.tests.is_empty());
        assert_eq!(contract.requirement, crate::GateRequirement::full());
    }
}
