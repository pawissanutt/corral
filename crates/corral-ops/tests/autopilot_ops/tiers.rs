//! Gate tiers on main: review rounds reuse the worker's quick receipt, a landing on main runs one
//! full gate, and promotion carries the quick approval and its evidence onto that receipt.
use super::*;
use std::collections::BTreeMap;

use corral_core::{
    ArtifactKind, CandidateIdentity, CriteriaManifest, EvidenceObservation, GatePromotion,
    GateRecord, GateRules, ReviewOverride, TierPolicy,
};
use serde_json::{Value, json};

const TEMPLATE: &str =
    "{{id}} {{title}}\n{{body}}\nRun the gate below once after the final rebase.\n{{gate}}\n";
const SUMMARY: &str = "Implemented the widget.\nVerified: corral gate\nNot verified: none\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    /// No `gate_tiers`: today's project.
    Untiered,
    /// Dispatched before tiers were configured, then configured with quick review rounds.
    Legacy,
    /// Tiered, but `tier_policy.review_rounds` is unset.
    Unset,
    /// Tiered with `review_rounds = quick`.
    Quick,
}

fn quick_command(out: &Path) -> String {
    let out = out.display();
    format!("echo quick >> {out}/runs && test ! -e {out}/quick-fails")
}

fn full_command(out: &Path) -> String {
    let out = out.display();
    format!("echo full >> {out}/runs && test ! -e {out}/full-fails")
}

fn configure(fx: &mut Fx, mode: Mode, out: &Path) {
    let review = &mut fx.ctx.store.config.review;
    review.gate = Some(vec![full_command(out)]);
    if mode != Mode::Untiered {
        review.gate_tiers = Some(BTreeMap::from([(
            "quick".to_owned(),
            vec![quick_command(out)],
        )]));
        review.tier_policy = (mode != Mode::Unset).then(|| TierPolicy {
            plan_member: None,
            review_rounds: Some("quick".into()),
        });
    }
    fx.ctx.store.save_config().unwrap();
}

/// The base fixture's T-001 is discarded; T-002 is dispatched main-bound under `mode`, with
/// reviewer `rev` unless `policy` says otherwise. Gate commands append their tier to `out/runs`.
pub(super) fn tiered(
    mode: Mode,
    audited: bool,
    policy: impl FnOnce(&mut ReviewOverride),
) -> (Fx, PathBuf) {
    // Tier/candidate identity tests need their declared profiles, not starter catalogs.
    let mut fx = Fx::with_config(
        |r| {
            r.reviewer = Some("rev".into());
            r.remove_worktree = Some(false);
            policy(r);
        },
        Some(
            "[projects.app]\npath = \"app\"\n[pm]\nagent = \"fake\"\n[agents.fake]\ncommand = \"true\"\n",
        ),
    );
    corral_ops::discard(&fx.ctx, &fx.id).unwrap();
    let out = fx._tmp.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::create_dir_all(fx.ctx.store.prompts_dir()).unwrap();
    std::fs::write(fx.ctx.store.prompts_dir().join("run.md"), TEMPLATE).unwrap();
    let dispatched = if mode == Mode::Legacy {
        Mode::Untiered
    } else {
        mode
    };
    configure(&mut fx, dispatched, &out);
    let task = fx
        .ctx
        .store
        .create(
            "Ship the widget",
            Some("app"),
            Status::Ready,
            "## Goal\nShip it.\n",
        )
        .unwrap();
    fx.id = task.id.clone();
    if audited {
        let manifest: CriteriaManifest = serde_json::from_value(json!({
            "version": 1, "spec_digest": corral_ops::completion_spec_digest(&task).unwrap(),
            "reviewed_by": "owner", "audited": true, "require_gate": true,
            "criteria": [{"id": "C1", "required": true, "live": false,
                "scenario": "the widget spins", "expected": "pass"}],
            "owner_decisions": [],
        }))
        .unwrap();
        corral_ops::register_criteria(&fx.ctx, &fx.id, &manifest).unwrap();
    }
    let opts = DispatchOpts {
        profile: Some("fake".into()),
        ..DispatchOpts::default()
    };
    let started = corral_ops::dispatch(&fx.ctx, &fx.id, &opts).unwrap();
    assert!(matches!(started, DispatchOutcome::Started { .. }));
    let rules = fx.run().gate_rules;
    let tiered = matches!(mode, Mode::Unset | Mode::Quick);
    assert_eq!(rules == GateRules::Tiered, tiered, "{mode:?}");
    if mode == Mode::Legacy {
        configure(&mut fx, Mode::Quick, &out);
    }
    fx.grant(std::slice::from_ref(&fx.id.clone()));
    (fx, out)
}

pub(super) fn runs(out: &Path) -> Vec<String> {
    std::fs::read_to_string(out.join("runs"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// This task's events of `kind`, in order.
pub(super) fn events_of(fx: &Fx, kind: &str) -> Vec<Value> {
    fx.ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.task.as_deref() == Some(fx.id.as_str()) && e.kind == kind)
        .map(|e| e.detail)
        .collect()
}

/// Position of this task's first event of `kind` after `after`, in the whole log.
pub(super) fn position(fx: &Fx, kind: &str, after: usize) -> usize {
    let events = fx.ctx.store.events().unwrap();
    after
        + events[after..]
            .iter()
            .position(|e| e.task.as_deref() == Some(fx.id.as_str()) && e.kind == kind)
            .unwrap_or_else(|| panic!("no {kind} event after {after}"))
}

pub(super) fn observe(fx: &Fx, out: &Path, attempt: &str, round: usize) {
    let proof = out.join(format!("proof-{round}.txt"));
    std::fs::write(&proof, format!("pass {round}\n")).unwrap();
    let artifact = corral_ops::add_artifact(
        &fx.ctx,
        &fx.id,
        &proof,
        Some(ArtifactKind::Evidence),
        Some("Widget scenario proof"),
    )
    .unwrap();
    let observation: EvidenceObservation = serde_json::from_value(json!({
        "version": 1, "key": format!("k{round}"), "criterion": "C1", "run": fx.run().id,
        "attempt": attempt, "status": "pass", "action": "spin the widget",
        "environment": "fixture", "expected": "pass", "observed": "pass",
        "artifacts": [artifact.path], "captured": corral_core::clock::now(),
        "producer": "test", "live": false,
    }))
    .unwrap();
    corral_ops::record_evidence(&fx.ctx, &fx.id, &observation).unwrap();
}

/// The worker's sequence after its final rebase: commit, the one receipted `corral gate --wait`,
/// evidence for that attempt when audited, then `done`.
pub(super) fn worker_round(fx: &Fx, out: &Path, round: usize) -> GateRecord {
    let file = format!("round-{round}.txt");
    fx.change(&file, "w\n");
    corral_git::commit_all(&fx.run().worktree, &file).unwrap();
    corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
    let gate = corral_ops::gate_wait(&fx.ctx, &fx.id).unwrap();
    assert_eq!(gate.result, GateResult::Pass, "{:?}", gate.failed_cmd);
    if fx.task().completion.is_some() {
        observe(fx, out, &gate.identity.as_ref().unwrap().attempt, round);
    }
    fx.done_with(SUMMARY);
    gate
}

/// Autopilot reuses the worker's receipt for the review, which approves it: approval A.
pub(super) fn approved(fx: &Fx, out: &Path) -> (CandidateIdentity, GatePromotion) {
    let receipt = worker_round(fx, out, 1);
    fx.sync();
    assert_eq!(fx.stage(), Some(AutoStage::Reviewing), "{}", fx.reason());
    fx.verdict(Verdict::Approve, "Ship it.");
    let identity = fx.run().review.unwrap().identity.unwrap();
    assert_eq!(Some(&identity), receipt.identity.as_ref());
    let origin = GatePromotion {
        identity: identity.clone(),
        gate_at: receipt.at,
    };
    (identity, origin)
}

pub(super) fn ledger(fx: &Fx) -> Vec<u8> {
    serde_json::to_vec(&fx.run().completion.evidence).unwrap()
}

/// The landed approval was promoted by `method` from A onto the current full receipt, which
/// copied the origin and replayed A's environment; the evidence relation and ledger agree.
pub(super) fn assert_promoted(fx: &Fx, method: &str, origin: &GatePromotion, ledger: &[u8]) {
    assert_eq!(fx.task().status, Status::Done, "{}", fx.reason());
    let run = fx.run();
    let gate = run.gate.clone().unwrap();
    let full = gate.identity.clone().unwrap();
    let tier = gate.tier.clone().unwrap();
    assert_eq!(
        (tier.tier.as_str(), tier.promoted_from.as_ref()),
        ("full", Some(origin))
    );
    assert_eq!(full.gate_tier, None);
    assert_eq!(full.environment, origin.identity.environment);
    let carried = events_of(fx, "verdict_carried");
    assert_eq!(carried.len(), 1, "{carried:?}");
    let detail = &carried[0];
    assert_eq!(
        (&detail["method"], &detail["from_tier"], &detail["to_tier"]),
        (&json!(method), &json!("quick"), &json!("full"))
    );
    assert_eq!(detail["old_identity"], json!(origin.identity));
    assert_eq!(detail["new_identity"], json!(full));
    assert_eq!(detail["promotion_origin"], json!(origin));
    let relation = &detail["evidence_carry"];
    assert_eq!(relation["from"], json!(origin.identity));
    assert_eq!(relation["window_start"], json!(origin.gate_at));
    assert_eq!(relation["to"], json!(full));
    assert_eq!(json!(run.completion.evidence_carry), *relation);
    assert_eq!(
        serde_json::to_vec(&run.completion.evidence).unwrap(),
        ledger
    );
    assert_eq!(run.promotion_origin, None);
    let review = run.review.unwrap();
    assert_eq!(review.identity.as_ref(), Some(&full));
    assert_eq!(review.sha, full.head);
    assert_eq!(git(&fx.app, &["rev-parse", "main"]), full.head);
    let promoted = position(fx, "verdict_carried", 0);
    assert!(position(fx, "approved", promoted) > promoted);
}

#[test]
fn audited_review_rounds_reuse_the_worker_quick_gate_and_one_full_gate_promotes_the_approval() {
    let (fx, out) = tiered(Mode::Quick, true, |r| r.accept = Some(Accept::Auto));
    // Given two change rounds, each on the worker's own quick receipt.
    for round in 1..=2 {
        worker_round(&fx, &out, round);
        fx.sync();
        assert_eq!(fx.stage(), Some(AutoStage::Reviewing), "{}", fx.reason());
        fx.verdict(Verdict::Changes, &format!("round {round}: tighten it"));
        fx.sync();
        assert_eq!(fx.task().status, Status::Running, "{}", fx.reason());
    }
    let receipt = worker_round(&fx, &out, 3);
    fx.sync();
    fx.verdict(Verdict::Approve, "Ship it.");
    let approved = fx.run().review.unwrap().identity.unwrap();
    assert_eq!(receipt.identity.as_ref(), Some(&approved));
    let origin = GatePromotion {
        identity: approved,
        gate_at: receipt.at,
    };
    let before = ledger(&fx);
    // When the approval lands while main stays put.
    fx.sync();
    // Then each round ran one quick gate and the landing one full gate, with no new review.
    assert_eq!(runs(&out), ["quick", "quick", "quick", "full"]);
    assert_eq!(events_of(&fx, "gate_started").len(), 4);
    assert_eq!(events_of(&fx, "review_started").len(), 3);
    assert_eq!(events_of(&fx, "evidence_recorded").len(), 3);
    assert_promoted(&fx, "tier_promotion", &origin, &before);
    let landed = events_of(&fx, "approved").pop().unwrap();
    assert_eq!(landed["acceptance_mode"], "autonomous");
    assert_eq!(landed["identity"], json!(fx.run().gate.unwrap().identity));
}

#[test]
fn main_moving_after_approval_replays_once_then_one_full_gate_on_the_replayed_head_promotes() {
    let (fx, out) = tiered(Mode::Quick, true, |r| r.accept = Some(Accept::Auto));
    let (approved, origin) = approved(&fx, &out);
    let before = ledger(&fx);
    fx.commit_on_main("other.txt", "main moved\n", "main advanced");
    fx.sync();
    assert_eq!(events_of(&fx, "candidate_rebased").len(), 1);
    assert_eq!(runs(&out), ["quick", "full"]);
    assert_eq!(events_of(&fx, "review_started").len(), 1);
    // The replay came before the landing gate, which ran on the replayed head.
    let rebased = position(&fx, "candidate_rebased", 0);
    let landing = position(&fx, "gate_started", rebased);
    let head = fx.run().gate.unwrap().sha;
    assert_ne!(head, approved.head);
    let started = &fx.ctx.store.events().unwrap()[landing];
    assert_eq!(started.detail["sha"], head);
    assert_promoted(&fx, "clean_replay+tier_promotion", &origin, &before);
}

#[test]
fn replaced_landing_gates_keep_the_origin_and_the_passing_replacement_promotes() {
    // Main advances during landing gate B, and corral restarts before replacement C.
    let (mut fx, out) = tiered(Mode::Quick, true, |r| r.accept = Some(Accept::Auto));
    let (_, origin) = approved(&fx, &out);
    let before = ledger(&fx);
    corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
    assert_eq!(fx.run().promotion_origin.as_deref(), Some(&origin));
    fx.commit_on_main("other.txt", "main moved\n", "main advanced");
    fx.ctx.store = Store::open(fx.ctx.store.root()).unwrap();
    fx.sync();
    let collected = events_of(&fx, "gate");
    let stale = collected[collected.len() - 2]["failed_cmd"]
        .as_str()
        .unwrap();
    assert!(stale.starts_with("stale_result"), "{stale}");
    assert_eq!(runs(&out), ["quick", "full", "full"]);
    assert_eq!(events_of(&fx, "review_started").len(), 1);
    assert_promoted(&fx, "clean_replay+tier_promotion", &origin, &before);

    // B's pane is lost and C passes on the same head; the owner approves.
    let (fx, out) = tiered(Mode::Quick, true, |r| r.accept = Some(Accept::Owner));
    let (_, origin) = approved(&fx, &out);
    let before = ledger(&fx);
    corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
    std::fs::remove_file(corral_ops::gate_paths(&fx.ctx, &fx.id, &fx.run().id).result).unwrap();
    fx.ctx
        .store
        .update(&fx.id, |task| {
            let gate = task.current_run_mut().unwrap().gate.as_mut().unwrap();
            gate.pane = None;
            gate.process_lost = true;
            Ok(())
        })
        .unwrap();
    corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
    assert_eq!(
        corral_ops::gate_wait(&fx.ctx, &fx.id).unwrap().result,
        GateResult::Pass
    );
    corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, false, true).unwrap();
    assert_eq!(runs(&out), ["quick", "full", "full"]);
    assert_eq!(events_of(&fx, "review_started").len(), 1);
    assert_promoted(&fx, "tier_promotion", &origin, &before);
}

#[test]
fn main_landings_run_or_require_a_full_gate_while_only_a_quick_receipt_exists() {
    // Without a reviewer no approval exists, yet the landing still runs the full tier.
    let (fx, out) = tiered(Mode::Quick, false, |r| {
        r.reviewer = Some("none".into());
        r.accept = Some(Accept::Owner);
    });
    worker_round(&fx, &out, 1);
    fx.sync();
    assert_eq!(fx.stage(), Some(AutoStage::Reviewed), "{}", fx.reason());
    assert_eq!(runs(&out), ["quick"]);
    corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, false, true).unwrap();
    assert_eq!(runs(&out), ["quick", "full"]);
    let gate = fx.run().gate.unwrap();
    assert_eq!(gate.tier.unwrap().tier, "full");
    assert_eq!(gate.identity.unwrap().gate_tier, None);
    assert_eq!(git(&fx.app, &["rev-parse", "main"]), gate.sha);
    // `corral approve` with either merge strategy runs the full gate before main moves.
    for strategy in [ApproveStrategy::FastForward, ApproveStrategy::Rebase] {
        let (fx, out) = tiered(Mode::Quick, false, |r| r.accept = Some(Accept::Owner));
        approved(&fx, &out);
        let main = git(&fx.app, &["rev-parse", "main"]);
        corral_ops::approve(&fx.ctx, &fx.id, strategy, false, true).unwrap();
        assert_eq!(runs(&out), ["quick", "full"], "{strategy:?}");
        let full = position(&fx, "gate_started", 0);
        let full = position(&fx, "gate_started", full + 1);
        assert!(position(&fx, "approved", full) > full);
        assert_eq!(fx.run().gate.unwrap().identity.unwrap().gate_tier, None);
        assert_ne!(git(&fx.app, &["rev-parse", "main"]), main);
    }
    // Keep-branch moves no ref and needs no landing tier.
    let (fx, out) = tiered(Mode::Quick, false, |r| r.accept = Some(Accept::Owner));
    approved(&fx, &out);
    let main = git(&fx.app, &["rev-parse", "main"]);
    corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::KeepBranch, false, true).unwrap();
    assert_eq!(fx.task().status, Status::Done);
    assert_eq!(runs(&out), ["quick"]);
    assert_eq!(git(&fx.app, &["rev-parse", "main"]), main);
}

#[test]
fn a_candidate_behind_its_destination_is_replayed_before_its_gate_starts() {
    // Given the worker's receipt, after which main moves before autopilot gates the candidate.
    let (fx, out) = tiered(Mode::Quick, false, |r| r.accept = Some(Accept::Owner));
    worker_round(&fx, &out, 1);
    fx.commit_on_main("other.txt", "main moved\n", "main advanced");
    fx.sync();
    // Then corral replays first and gates the replayed head (a review round: quick).
    let rebased = position(&fx, "candidate_rebased", 0);
    let started = position(&fx, "gate_started", rebased);
    let head = git(&fx.run().worktree, &["rev-parse", "HEAD"]);
    assert_eq!(fx.ctx.store.events().unwrap()[started].detail["sha"], head);
    assert_eq!(runs(&out), ["quick", "quick"]);
    assert_eq!(fx.stage(), Some(AutoStage::Reviewing), "{}", fx.reason());
    // A conflicting replay sends changes as today, without a gate.
    let (fx, out) = tiered(Mode::Quick, false, |r| r.accept = Some(Accept::Owner));
    worker_round(&fx, &out, 1);
    fx.commit_on_main("round-1.txt", "conflict\n", "main conflicts");
    fx.sync();
    assert_eq!(runs(&out), ["quick"]);
    assert_eq!(events_of(&fx, "gate_started").len(), 1);
    assert_eq!(events_of(&fx, "integration_request").len(), 1);
    assert_eq!(fx.task().status, Status::Running, "{}", fx.reason());
}

#[test]
fn pm_observation_asks_for_the_landing_gate_then_landing_instead_of_a_new_review() {
    let (fx, out) = tiered(Mode::Quick, true, |r| r.accept = Some(Accept::Owner));
    approved(&fx, &out);
    // The approved quick receipt is trusted; it still needs the full landing gate.
    let card = corral_ops::pm_next(&fx.ctx).unwrap();
    assert_eq!(card.task.as_deref(), Some(fx.id.as_str()));
    assert_eq!(card.kind, corral_ops::PmActionKind::Gate, "{card:?}");
    // Once that gate passes, the promotable approval lands without another review.
    corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
    assert_eq!(
        corral_ops::gate_wait(&fx.ctx, &fx.id).unwrap().result,
        GateResult::Pass
    );
    let card = corral_ops::pm_next(&fx.ctx).unwrap();
    assert_eq!(card.kind, corral_ops::PmActionKind::Land, "{card:?}");
    corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, false, true).unwrap();
    assert_eq!(
        events_of(&fx, "verdict_carried")[0]["method"],
        "tier_promotion"
    );
    assert_eq!(events_of(&fx, "review_started").len(), 1);
}

#[test]
fn a_failed_quick_gate_and_a_stale_review_send_back_the_tier_and_untiered_keeps_todays_text() {
    for mode in [Mode::Quick, Mode::Untiered] {
        // Given a submitted round whose gate tier fails.
        let (fx, out) = tiered(mode, false, |r| r.accept = Some(Accept::Owner));
        std::fs::write(out.join("quick-fails"), "").unwrap();
        std::fs::write(out.join("full-fails"), "").unwrap();
        fx.change("round-1.txt", "w\n");
        corral_git::commit_all(&fx.run().worktree, "round 1").unwrap();
        fx.done_with(SUMMARY);

        // When autopilot gates it.
        fx.sync();
        fx.sync();

        // Then the change request names the tier and the one receipted gate on a quick run only.
        assert_eq!(
            fx.stage(),
            Some(AutoStage::Changes),
            "{mode:?}: {}",
            fx.reason()
        );
        let request = fx.artifact_text("Autopilot changes (round 1)");
        let gate = format!("gate {} --wait`", fx.id);
        let tiered = [
            "the gate (quick tier) failed at",
            gate.as_str(),
            "it runs the quick tier",
            "expected failures, under `Not verified:`",
            "Do not start, or attach evidence of, a full-tier gate",
        ];
        for needle in tiered {
            assert_eq!(
                request.contains(needle),
                mode == Mode::Quick,
                "{mode:?} {needle:?}:\n{request}"
            );
        }
        assert_eq!(
            request.contains("the gate failed at"),
            mode == Mode::Untiered,
            "{request}"
        );

        // When the candidate's review is bound to an older head.
        fx.ctx
            .store
            .update(&fx.id, |t| {
                t.status = Status::Review;
                t.current_run_mut().unwrap().review = Some(
                    serde_json::from_value(json!({
                        "reviewer":"rev","sha":"old","at":"2026-10-15T12:00:00Z","verdict":"approve"
                    }))
                    .unwrap(),
                );
                Ok(())
            })
            .unwrap();
        let card = corral_ops::pm_next(&fx.ctx).unwrap();

        let repair = card.evidence.join("\n");

        // Then the PM send-back names the tier and `corral gate <id> --wait` only on a quick run.
        assert_eq!(
            card.kind,
            corral_ops::PmActionKind::StaleVerdict,
            "{card:?}"
        );
        let regate = format!("corral gate {} --wait", fx.id);
        assert_eq!(repair.contains(&regate), mode == Mode::Quick, "{card:?}");
        assert_eq!(
            repair.contains("it runs the quick tier"),
            mode == Mode::Quick
        );
        assert_eq!(
            repair.contains("run the full configured gate"),
            mode == Mode::Untiered,
            "{card:?}"
        );
        assert_eq!(
            card.verify.contains("full gate"),
            mode == Mode::Untiered,
            "{card:?}"
        );
    }
}

/// Gate, review, replay and landing event kinds of this task, with autopilot target stages.
fn sequence(fx: &Fx) -> Vec<String> {
    let kinds = [
        "gate_started",
        "gate",
        "review_started",
        "candidate_rebased",
        "verdict_carried",
        "evidence_recorded",
        "approved",
        "autopilot",
    ];
    fx.ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.task.as_deref() == Some(fx.id.as_str()) && kinds.contains(&e.kind.as_str()))
        .map(|e| match e.kind.as_str() {
            "autopilot" => format!("autopilot:{}", e.detail["to"].as_str().unwrap_or("?")),
            kind => kind.to_owned(),
        })
        .collect()
}

#[test]
fn untiered_legacy_and_unset_review_rounds_keep_todays_sequence_and_evidence_escalation() {
    let mut sequences = Vec::new();
    for mode in [Mode::Untiered, Mode::Legacy, Mode::Unset] {
        // An audited approval, then main moves: a same-tier replay of today's flow.
        let (fx, out) = tiered(mode, true, |r| r.accept = Some(Accept::Auto));
        approved(&fx, &out);
        fx.commit_on_main("other.txt", "main moved\n", "main advanced");
        fx.sync();
        assert_eq!(runs(&out), ["full", "full"], "{mode:?}");
        assert_eq!(fx.stage(), Some(AutoStage::Escalated), "{mode:?}");
        assert!(
            fx.reason().contains("required evidence missing or stale"),
            "{mode:?}: {}",
            fx.reason()
        );
        let run = fx.run();
        assert_eq!(run.completion.evidence_carry, None, "{mode:?}");
        assert_eq!(run.promotion_origin, None, "{mode:?}");
        assert!(events_of(&fx, "verdict_carried").is_empty(), "{mode:?}");
        sequences.push(sequence(&fx));
    }
    assert_eq!(sequences[0], sequences[1]);
    assert_eq!(sequences[0], sequences[2]);
}
