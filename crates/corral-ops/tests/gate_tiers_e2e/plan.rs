//! G8, G5 and G3: audited quick members of an owned integration-target Plan gate once per round,
//! reach independent review, carry onto a sibling's landing and land on the target; plan verify
//! and plan accept run the full tier, so only a full PASS candidate reaches main.
use super::*;
use corral_core::{
    ExpectedFailure, MemberGateAmendment, ObservationStatus, Origin, ScopeGate, Status,
};
use corral_ops::{AggregateVerdict, DispatchOpts, DispatchOutcome};

const TARGET: &str = "feature/plan";
const EXPECTED: &str = "core cache";
const OUT_OF_SCOPE: &str = "other legacy";

pub(super) struct PlanFx {
    pub(super) fx: Fx,
    pub(super) tracker: String,
    pub(super) a: String,
    pub(super) b: String,
    fixer: Option<String>,
}

/// The owner's first, revision-guarded declaration of `member`'s gate on `tracker`.
fn declare_gate(fx: &Fx, tracker: &str, member: &str, tests: &[&str], expected: &[&str]) {
    let amendment = MemberGateAmendment {
        member: member.to_owned(),
        expected_revision: fx
            .ctx
            .store
            .get(tracker)
            .unwrap()
            .member_gates
            .get(member)
            .map_or(0, |record| record.revision),
        gate: ScopeGate {
            tier: "quick".into(),
            tests: tests.iter().map(|t| (*t).to_owned()).collect(),
            expected_failures: expected
                .iter()
                .map(|test| ExpectedFailure {
                    test: (*test).to_owned(),
                    reason: "fails until the cache member restores it".into(),
                })
                .collect(),
        },
        reason: "declared by the planner at decomposition, approved with the plan".into(),
        by: "owner".into(),
    };
    fx.ctx
        .store
        .amend_member_gate(tracker, &amendment, |_, _| Ok(()), |_, _| Ok(None))
        .unwrap();
}

/// Dispatches `id` unless autopilot already did under the grant.
pub(super) fn dispatch(fx: &Fx, id: &str) {
    if fx.task(id).runs.is_empty() {
        let started = corral_ops::dispatch(&fx.ctx, id, &DispatchOpts::default()).unwrap();
        assert!(matches!(started, DispatchOutcome::Started { .. }));
    }
    let run = fx.run(id);
    assert_eq!(run.gate_rules, corral_core::GateRules::Tiered);
    assert_eq!(run.plan_target.unwrap().branch, TARGET);
}

/// A Plan whose audited members A and B declare quick gates over the `core` tests with the
/// expected failure, so `other legacy` is outside their selection. With `fixer`, member C's quick
/// gate selects both binaries and declares no expected failure. Nothing is dispatched yet.
pub(super) fn imported(fixer: bool) -> PlanFx {
    let mut fx = Fx::new(Suite::Broken, |_| {});
    let c = if fixer {
        "\n  - {key: c, title: Restore the cache and legacy tests, profile: fake, depends: [a, b]}"
    } else {
        ""
    };
    let source = fx.tmp.path().join("plan.md");
    std::fs::write(
        &source,
        format!(
            "---\ntitle: Integration Plan\nproject: app\napproved_scope:\n  goal: Ship A and B together\n  acceptance: [{{criterion: P1, scenario: Combine changes, expected: All included}}]\n  delivery_target: {{kind: integration, branch: {TARGET}}}\ntasks:\n  - {{key: a, title: A, profile: fake}}\n  - {{key: b, title: B, profile: fake}}{c}\n---\n"
        ),
    )
    .unwrap();
    let imported = corral_ops::import_plan(&fx.ctx, &source, false, Origin::Owner).unwrap();
    let ids: Vec<String> = imported.tasks.iter().map(|t| t.id.clone()).collect();
    let tracker = &imported.tracker.id;
    for id in &ids[..2] {
        declare_gate(&fx, tracker, id, &["core"], &[EXPECTED]);
    }
    if let Some(c) = ids.get(2) {
        declare_gate(&fx, tracker, c, &["core", "other"], &[]);
    }
    for id in &ids[..2] {
        fx.audit(id);
    }
    fx.grant(&[&imported.tracker.id]);
    PlanFx {
        tracker: imported.tracker.id.clone(),
        a: ids[0].clone(),
        b: ids[1].clone(),
        fixer: ids.get(2).cloned(),
        fx,
    }
}

fn plan(fixer: bool) -> PlanFx {
    let p = imported(fixer);
    dispatch(&p.fx, &p.a);
    dispatch(&p.fx, &p.b);
    p
}

pub(super) fn summary(change: &str) -> String {
    format!(
        "{change}\nVerified: corral's receipted quick gate passed\nNot verified: `{OUT_OF_SCOPE}` fails outside this member's declared tier selection; reported, not fixed\n"
    )
}

fn statuses(fx: &Fx, attempt: &str) -> BTreeMap<String, String> {
    fx.suite()
        .into_iter()
        .filter(|(_, a, _, _)| a == attempt)
        .map(|(_, _, test, status)| (test, status))
        .collect()
}

fn sync_until_done(fx: &Fx, ids: &[&str]) {
    for _ in 0..6 {
        if ids.iter().all(|id| fx.task(id).status == Status::Done) {
            return;
        }
        fx.sync();
    }
    for id in ids {
        let run = fx.run(id);
        assert_eq!(
            fx.task(id).status,
            Status::Done,
            "{id}: {:?}",
            run.autopilot
        );
    }
}

/// G8 and G5: A takes one changes round, B is approved on its first receipt, then both land on
/// the target; B through the clean-replay carry. Returns A's and B's receipts in order.
fn land_members(p: &PlanFx) -> Vec<GateRecord> {
    let fx = &p.fx;
    let (a, b) = (p.a.as_str(), p.b.as_str());
    let a1 = summary("Added the A widget.");
    let a1 = work(
        fx,
        a,
        "quick",
        &Round {
            files: &[("a.txt", "a1\n")],
            summary: &a1,
        },
    );
    let b1 = summary("Added the B widget.");
    let b1 = work(
        fx,
        b,
        "quick",
        &Round {
            files: &[("b.txt", "b\n")],
            summary: &b1,
        },
    );
    fx.sync();
    for id in [a, b] {
        assert!(
            fx.run(id).review.is_some(),
            "{id}: {:?} {:?} {:?}",
            fx.run(id).autopilot,
            corral_ops::automation_blocker(&fx.ctx, &fx.task(id)),
            fx.task(id).status
        );
    }
    review(
        fx,
        a,
        "quick",
        Verdict::Changes,
        "Round 1: name the A widget explicitly.",
    );
    fx.sync();
    assert_eq!(
        fx.task(a).status,
        Status::Running,
        "{:?}",
        fx.run(a).autopilot
    );
    assert_eq!(fx.task(a).rounds, 1);
    let a2 = summary("Named the A widget explicitly.");
    let a2 = work(
        fx,
        a,
        "quick",
        &Round {
            files: &[("a.txt", "a2\n")],
            summary: &a2,
        },
    );
    fx.sync();
    let approved = review(fx, a, "quick", Verdict::Approve, "A is correct.");
    review(fx, b, "quick", Verdict::Approve, "B is correct.");
    sync_until_done(fx, &[a, b]);

    // Each round ran exactly the worker's receipted quick gate, which autopilot reused.
    for (id, receipts) in [(a, vec![&a1, &a2]), (b, vec![&b1])] {
        let started = fx.events(id, "gate_started");
        assert_eq!(started.len(), receipts.len(), "{id}: {started:?}");
        for (event, receipt) in started.iter().zip(&receipts) {
            assert_eq!(event["tier"], "quick", "{id}");
            assert_eq!(event["identity"], json!(receipt.identity), "{id}");
        }
        assert_eq!(fx.events(id, "evidence_recorded").len(), receipts.len());
        assert_eq!(fx.events(id, "review_started").len(), receipts.len());
        let gate = fx.run(id).gate.unwrap();
        assert_eq!(
            gate.tier.unwrap().tier,
            "quick",
            "{id}: no full-tier receipt"
        );
        assert!(gate.identity.unwrap().gate_tier.is_some());
    }
    assert_eq!(
        fx.tiers_run(),
        ["quick", "quick", "quick"],
        "{:?}",
        fx.suite()
    );
    for receipt in [&a1, &b1, &a2] {
        let attempt = &receipt.identity.as_ref().unwrap().attempt;
        let tests = statuses(fx, attempt);
        assert_eq!(tests["core widget"], "pass");
        assert_eq!(
            tests[EXPECTED], "excluded",
            "expected failure excluded by exact name"
        );
        assert_eq!(
            tests[OUT_OF_SCOPE], "unselected",
            "out-of-scope test never ran"
        );
    }
    // The out-of-scope failure stayed under `Not verified:` and became a non-blocking note.
    let notes = approved.notes.unwrap();
    assert!(
        notes.contains(&format!(
            "Non-blocking note: `{OUT_OF_SCOPE}` fails outside"
        )),
        "{notes}"
    );
    assert!(
        fx.run(a)
            .summary
            .unwrap()
            .contains("Not verified: `other legacy`")
    );

    // A landed first; B was replayed onto it and carried with no new gate or review (G5).
    let landed = |id: &str| fx.events(id, "approved").pop().unwrap();
    for id in [a, b] {
        assert_eq!(landed(id)["base"], TARGET, "{id}");
        assert_eq!(fx.task(id).rounds, u32::from(id == a), "{id}");
    }
    assert!(fx.events(a, "gate_carried").is_empty());
    let carried = fx.events(b, "gate_carried");
    assert_eq!(carried.len(), 1, "{carried:?}");
    let relation = &carried[0]["evidence_carry"];
    assert_eq!(relation["from"], json!(b1.identity));
    assert_eq!(relation["window_start"], json!(b1.at));
    let verdict = fx.events(b, "verdict_carried").pop().unwrap();
    assert_eq!(verdict["method"], "clean_replay");
    assert_eq!(verdict["evidence_carry"], *relation);
    assert_eq!(landed(b)["evidence_carry"], *relation);
    let run = fx.run(b);
    assert_eq!(run.review.unwrap().identity, run.gate.unwrap().identity);
    let target = git(&fx.app, &["rev-parse", TARGET]);
    assert_eq!(json!(target), landed(b)["sha"]);
    assert_eq!(git(&fx.app, &["show", &format!("{target}:a.txt")]), "a2");
    assert_eq!(git(&fx.app, &["show", &format!("{target}:b.txt")]), "b");
    vec![a2, b1]
}

fn verify(p: &PlanFx) -> corral_core::AggregateAttempt {
    let attempt = corral_ops::plan_verify(&p.fx.ctx, &p.tracker).unwrap();
    assert_eq!(p.fx.tiers_run().last().map(String::as_str), Some("full"));
    let gate = attempt.gate.clone().unwrap();
    assert_eq!(gate.tier.unwrap().tier, "full");
    attempt
}

#[test]
fn audited_quick_members_gate_once_per_round_carry_and_plan_verify_fails_on_the_full_tier() {
    // Given a Plan of two audited quick members, one declared expected failure and one failing
    // test outside their selection.
    let p = plan(false);
    let main = git(&p.fx.app, &["rev-parse", "main"]);
    land_members(&p);
    assert_eq!(git(&p.fx.app, &["rev-parse", "main"]), main);

    // When plan verify runs while the expected failure persists.
    let attempt = verify(&p);

    // Then the full tier ran every test with no exclusion and failed.
    let gate = attempt.gate.clone().unwrap();
    assert_eq!(gate.result, GateResult::Fail);
    assert_eq!(gate.failed_cmd.as_deref(), Some(p.fx.full().as_str()));
    let tests = statuses(&p.fx, &attempt.identity.attempt);
    assert_eq!(tests[EXPECTED], "fail");
    assert_eq!(tests[OUT_OF_SCOPE], "fail");
    // And plan accept refuses it, so main never moves.
    let error = corral_ops::plan_accept(&p.fx.ctx, &p.tracker, &attempt.id, false).unwrap_err();
    assert!(
        format!("{error:#}").contains("aggregate gate has not passed"),
        "{error:#}"
    );
    assert_eq!(git(&p.fx.app, &["rev-parse", "main"]), main);
    assert_ne!(p.fx.task(&p.tracker).status, Status::Done);
}

/// Lands the fixing member `c`, whose quick gate selects both failing tests, on the target.
fn land_fixer(p: &PlanFx, c: &str) {
    dispatch(&p.fx, c);
    let fixed = "Restored the cache and legacy tests.\nVerified: corral's receipted quick gate passed\nNot verified: none\n";
    let files = [
        ("suite/core/cache", "pass\n"),
        ("suite/other/legacy", "pass\n"),
    ];
    work(
        &p.fx,
        c,
        "quick",
        &Round {
            files: &files,
            summary: fixed,
        },
    );
    p.fx.sync();
    review(&p.fx, c, "quick", Verdict::Approve, "The fix is correct.");
    sync_until_done(&p.fx, &[c]);
    assert_eq!(p.fx.events(c, "gate_started").len(), 1);
}

/// The full tier passed `attempt`; it is observed, reviewed and accepted, and main moves exactly
/// from `main` to the full PASS candidate.
fn accept_passing(p: &PlanFx, attempt: &corral_core::AggregateAttempt, main: &str) {
    let gate = attempt.gate.clone().unwrap();
    assert_eq!(gate.result, GateResult::Pass, "{:?}", gate.failed_cmd);
    let tests = statuses(&p.fx, &attempt.identity.attempt);
    assert!(tests.values().all(|status| status == "pass"), "{tests:?}");
    assert_eq!(git(&p.fx.app, &["rev-parse", TARGET]), gate.sha);
    let observation: corral_core::EvidenceObservation = serde_json::from_value(json!({
        "version": 1, "key": "P1-pass", "criterion": "P1", "run": "aggregate",
        "attempt": attempt.id, "status": ObservationStatus::Pass, "action": "run the combined scenario",
        "environment": "fixture", "expected": "All included", "observed": "All included",
        "artifacts": [], "captured": corral_core::clock::now(), "producer": "test", "live": false,
    }))
    .unwrap();
    corral_ops::plan_evidence(&p.fx.ctx, &p.tracker, &observation).unwrap();
    let review = corral_ops::plan_aggregate_review(&p.fx.ctx, &p.tracker, &attempt.id).unwrap();
    let verdict = AggregateVerdict {
        attempt: &attempt.id,
        review: &review.id,
        verdict: Verdict::Approve,
        notes: "The combined candidate passed the full tier.",
    };
    corral_ops::plan_aggregate_verdict(&p.fx.ctx, &p.tracker, &verdict).unwrap();
    let accepted = corral_ops::plan_accept(&p.fx.ctx, &p.tracker, &attempt.id, false).unwrap();

    assert_eq!(accepted.before.as_deref(), Some(main));
    assert_eq!(accepted.after.as_deref(), Some(gate.sha.as_str()));
    assert_eq!(git(&p.fx.app, &["rev-parse", "main"]), gate.sha);
    assert_eq!(p.fx.task(&p.tracker).status, Status::Done);
}

#[test]
fn a_member_fixing_the_expected_failure_lets_plan_verify_pass_and_main_receive_that_candidate() {
    // Given the same Plan with a third quick member whose selection covers both failing tests.
    let p = plan(true);
    land_members(&p);
    land_fixer(&p, p.fixer.as_deref().unwrap());
    let main = git(&p.fx.app, &["rev-parse", "main"]);

    // When plan verify runs after the fix landed on the target.
    let attempt = verify(&p);

    // Then the full tier passes and acceptance moves main exactly to that candidate.
    accept_passing(&p, &attempt, &main);
}

#[test]
fn one_plan_recovers_from_a_failed_full_verify_by_adding_and_landing_a_fixing_member() {
    // Given a Plan whose full verification failed, so acceptance is refused and main stays put.
    let mut p = plan(false);
    land_members(&p);
    let main = git(&p.fx.app, &["rev-parse", "main"]);
    let failed = verify(&p);
    assert_eq!(failed.gate.clone().unwrap().result, GateResult::Fail);
    assert!(corral_ops::plan_accept(&p.fx.ctx, &p.tracker, &failed.id, false).is_err());
    assert_eq!(git(&p.fx.app, &["rev-parse", "main"]), main);

    // When the owner adds a fixing member to the same Plan, declares its gate, re-confirms the
    // Plan's grant for the changed membership, and the member lands on the target.
    let store = &p.fx.ctx.store;
    let c = store
        .create(
            "Restore the cache and legacy tests",
            Some("app"),
            Status::Backlog,
            "",
        )
        .unwrap()
        .id;
    store
        .update(&c, |task| {
            task.profile = Some("fake".into());
            Ok(())
        })
        .unwrap();
    let added = corral_ops::plan_add_member(&p.fx.ctx, &p.tracker, &c).unwrap();
    assert_eq!(added.children, [p.a.clone(), p.b.clone(), c.clone()]);
    declare_gate(&p.fx, &p.tracker, &c, &["core", "other"], &[]);
    let tracker = p.tracker.clone();
    p.fx.grant(&[&tracker]);
    land_fixer(&p, &c);
    assert_eq!(git(&p.fx.app, &["rev-parse", "main"]), main);

    // Then the failed attempt stays refused, a new full verify of the same Plan passes, and
    // acceptance moves main exactly to the full-gated head.
    assert!(corral_ops::plan_accept(&p.fx.ctx, &p.tracker, &failed.id, false).is_err());
    let attempt = verify(&p);
    assert_ne!(attempt.id, failed.id);
    assert_eq!(p.fx.tiers_run().iter().filter(|t| *t == "full").count(), 2);
    accept_passing(&p, &attempt, &main);
}
