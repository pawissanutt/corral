use super::*;
use corral_core::{Artifact, ArtifactKind};

const GATE: [&str; 2] = ["test -f first.txt", "test -f second.txt"];

fn verify_checkout(f: &Fixture) -> PathBuf {
    f.repo.join(".worktrees").join(format!("{}-verify", f.plan))
}

fn capture(f: &Fixture, id: &str, name: &str) -> Artifact {
    let path = f._dir.path().join(name);
    std::fs::write(&path, format!("{name}: combined scenario output\n")).unwrap();
    crate::add_artifact(&f.ctx, id, &path, Some(ArtifactKind::Evidence), Some(name)).unwrap()
}

fn observe_with(f: &Fixture, attempt: &str, key: &str, artifacts: &[&Artifact]) -> Result<()> {
    let observation: EvidenceObservation = serde_json::from_value(json!({
        "version":1, "key":key, "criterion":"C2", "run":"aggregate", "attempt":attempt,
        "status":"pass", "action":"run the combined scenario", "environment":"fixture",
        "expected":"pass together", "observed":"observed",
        "artifacts":artifacts.iter().map(|artifact| &artifact.path).collect::<Vec<_>>(),
        "captured":corral_core::clock::now(), "producer":"test", "live":true,
    }))
    .unwrap();
    plan_evidence(&f.ctx, &f.plan, &observation).map(|_| ())
}

/// The attempt's gate row ended with exactly the outcome its saved receipt records.
fn assert_row_matches(f: &Fixture, attempt: &AggregateAttempt) {
    let gate = attempt.gate.as_ref().unwrap();
    let row = f
        .ctx
        .store
        .gate_attempt(&f.plan, super::super::gate::AGGREGATE_SCOPE, &attempt.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.result.as_deref(), row.failed_cmd.as_deref()),
        (Some(gate.result.as_str()), gate.failed_cmd.as_deref())
    );
    assert!(row.ended.is_some() && row.log.is_some());
}

fn edit_reviewer(f: &Fixture, command: &str) {
    let mut store = f.ctx.store.reopen().unwrap();
    store.config.agents.get_mut("rev").unwrap().command = command.into();
    store.save_config().unwrap();
}

#[test]
fn gate_that_changes_checkout_files_cannot_prove_the_candidate() {
    // Given a gate that repairs a file in its checkout and then tests the repaired copy.
    let f = Fixture::new(
        Delivery::Main,
        &["echo repaired > first.txt", "grep -q repaired first.txt"],
    );
    let main = f.main();
    // When the aggregate gate runs.
    let attempt = plan_verify(&f.ctx, &f.plan).unwrap();
    // Then its PASS describes other bytes, so the receipt is a failure and nothing lands.
    let gate = attempt.gate.as_ref().unwrap();
    assert_eq!(gate.result, GateResult::Fail);
    assert!(
        gate.failed_cmd
            .as_deref()
            .is_some_and(|reason| reason.contains("verification_checkout_changed")),
        "{gate:?}"
    );
    assert_row_matches(&f, &attempt);
    for criterion in ["C1", "C2"] {
        f.observe(&attempt.id, criterion, ObservationStatus::Pass)
            .unwrap();
    }
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, false));
    assert!(error.contains("aggregate gate has not passed"), "{error}");
    assert_eq!(
        std::fs::read_to_string(f.repo.join("first.txt")).unwrap(),
        "accepted work\n"
    );
    f.assert_not_completed(&main);
}

#[test]
fn gate_that_moves_checkout_head_cannot_prove_the_candidate() {
    // Given a gate that leaves a clean checkout at a different commit.
    let f = Fixture::new(
        Delivery::Main,
        &[
            "git -c user.name=t -c user.email=t@example.invalid -c commit.gpgsign=false commit -q --allow-empty -m moved",
        ],
    );
    let main = f.main();
    // When the aggregate gate runs.
    let attempt = plan_verify(&f.ctx, &f.plan).unwrap();
    // Then the clean but different HEAD cannot authorize the candidate.
    let gate = attempt.gate.as_ref().unwrap();
    assert_eq!(gate.result, GateResult::Fail);
    assert!(
        gate.failed_cmd
            .as_deref()
            .is_some_and(|reason| reason.contains("HEAD is")),
        "{gate:?}"
    );
    assert_row_matches(&f, &attempt);
    f.assert_not_completed(&main);
}

#[test]
fn passing_gate_is_rechecked_against_its_checkout_at_acceptance() {
    // Given a passing aggregate attempt whose checkout is later modified.
    let f = Fixture::new(Delivery::Main, &GATE);
    let main = f.main();
    let attempt = f.passing();
    assert_eq!(attempt.gate.as_ref().unwrap().result, GateResult::Pass);
    std::fs::write(verify_checkout(&f).join("first.txt"), "changed later\n").unwrap();
    // When the owner accepts it.
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, false));
    // Then the untested checkout state cannot be accepted.
    assert!(error.contains("aggregate gate has not passed"), "{error}");
    f.assert_not_completed(&main);
}

#[test]
fn reviewer_definition_change_invalidates_aggregate_proof() {
    // Given an independently approved attempt under an aggregate acceptance grant.
    let f = Fixture::new(Delivery::Integration, &GATE);
    f.grant(OperatingAuthority {
        aggregate_accept: true,
        ..Default::default()
    });
    let main = f.main();
    let attempt = f.passing();
    f.review(&attempt.id, Verdict::Approve).unwrap();
    // When the selected reviewer's definition changes but its name stays the same.
    edit_reviewer(&f, "false");
    // Then evidence, review and both acceptance origins refuse the stale proof.
    for error in [
        message(f.observe(&attempt.id, "C1", ObservationStatus::Pass)),
        message(f.review(&attempt.id, Verdict::Approve)),
        message(plan_accept(&f.ctx, &f.plan, &attempt.id, true)),
        message(plan_accept(&f.ctx, &f.plan, &attempt.id, false)),
    ] {
        assert!(error.contains("stale Plan proof"), "{error}");
        assert!(error.contains("inputs_digest"), "{error}");
    }
    f.assert_not_completed(&main);
}

#[test]
fn tampered_missing_or_stale_evidence_snapshots_invalidate_proof() {
    // Given an aggregate grant, and a capture attached before the attempt existed.
    let f = Fixture::new(Delivery::Integration, &GATE);
    f.grant(OperatingAuthority {
        aggregate_accept: true,
        ..Default::default()
    });
    let main = f.main();
    let early = capture(&f, &f.plan, "early.txt");
    let attempt = f.passing();
    // Then a capture from before the attempt is refused as stale.
    let error = message(observe_with(&f, &attempt.id, "C2-early", &[&early]));
    assert!(error.contains("artifact_before_attempt"), "{error}");
    // Given an approved observation bound to a fresh capture.
    let fresh = capture(&f, &f.plan, "fresh.txt");
    observe_with(&f, &attempt.id, "C2-fresh", &[&fresh]).unwrap();
    f.review(&attempt.id, Verdict::Approve).unwrap();
    let hash = corral_core::blob_ref::BlobHash::parse(&fresh.sha256).unwrap();
    let snapshot = f.ctx.store.blob_store().unwrap().path_of(&hash);
    let original = std::fs::read(&snapshot).unwrap();
    // When the stored snapshot bytes change after the approving review.
    std::fs::write(&snapshot, "replaced bytes\n").unwrap();
    // Then automatic acceptance and a new review refuse the corrupt evidence.
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, true));
    assert!(error.contains("artifact_tampered"), "{error}");
    let error = message(f.review(&attempt.id, Verdict::Approve));
    assert!(error.contains("artifact_tampered"), "{error}");
    // When the snapshot is missing instead.
    std::fs::write(&snapshot, &original).unwrap();
    std::fs::remove_file(&snapshot).unwrap();
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, false));
    assert!(error.contains("missing_artifact"), "{error}");
    f.assert_not_completed(&main);
    // And the intact snapshot remains acceptable proof.
    std::fs::write(&snapshot, &original).unwrap();
    plan_accept(&f.ctx, &f.plan, &attempt.id, true).unwrap();
    assert_eq!(f.plan().status, Status::Done);
}

#[test]
fn no_code_plan_refuses_changed_accepted_artifact_versions() {
    // Given a no-code Plan whose member delivered an accepted artifact.
    let f = Fixture::new(Delivery::NoCode, &GATE);
    let report = capture(&f, &f.members[0], "report.md");
    let main = f.main();
    // When the accepted snapshot is changed before verification.
    let hash = corral_core::blob_ref::BlobHash::parse(&report.sha256).unwrap();
    std::fs::write(
        f.ctx.store.blob_store().unwrap().path_of(&hash),
        "rewritten\n",
    )
    .unwrap();
    // Then verification and readiness name the changed delivery instead of binding it.
    let error = message(plan_verify(&f.ctx, &f.plan));
    assert!(error.contains("is missing or changed"), "{error}");
    let progress = crate::plan_progress(&f.ctx, &f.plan).unwrap();
    let aggregate = progress.aggregate.unwrap();
    assert!(!aggregate.ready);
    assert!(
        aggregate
            .blockers
            .iter()
            .any(|blocker| blocker.contains(&report.path)),
        "{:?}",
        aggregate.blockers
    );
    f.assert_not_completed(&main);
}

#[test]
fn interrupted_aggregate_attempt_ends_lost_with_its_partial_log_before_the_next() {
    // Given a verification that committed its Running attempt and gate row, then ended before
    // its runner finished, leaving part of a log.
    let f = Fixture::new(Delivery::Main, &GATE);
    let main = f.main();
    crate::gate::evidence::FAILPOINT.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(|point: &str| {
            assert_ne!(point, "plan_verify_running", "verification ended");
        }));
    });
    let ended = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        plan_verify(&f.ctx, &f.plan)
    }))
    .is_err();
    crate::gate::evidence::FAILPOINT.with(|hook| *hook.borrow_mut() = None);
    assert!(ended);
    let scratch = |attempt: &str| {
        let id = format!("gate-plan-{}-{attempt}", f.plan);
        f.ctx.store.scratch_row(&id).unwrap().unwrap().state
    };
    assert_eq!(scratch("A1"), corral_core::backend::ScratchState::Open);
    let log = super::super::gate::paths(&f.ctx, &f.plan, "A1").log;
    let partial = b"\n$ test -f first.txt\n".to_vec();
    std::fs::write(&log, &partial).unwrap();

    // When verification runs again.
    let next = plan_verify(&f.ctx, &f.plan).unwrap();

    // Then A1 ended lost in its marker and row alike, with the log it left and never a pass,
    // its directory was closed and then reclaimed, and A2 verified normally.
    assert_eq!(next.id, "A2");
    assert_eq!(next.gate.as_ref().unwrap().result, GateResult::Pass);
    let tracker = f.ctx.store.get(&f.plan).unwrap();
    let lost = tracker.aggregate_attempts[0].gate.clone().unwrap();
    assert_eq!(lost.result, GateResult::Fail);
    assert!(lost.process_lost);
    let row = f
        .ctx
        .store
        .gate_attempt(&f.plan, super::super::gate::AGGREGATE_SCOPE, "A1")
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.result.as_deref(), row.failed_cmd.as_deref()),
        (Some("fail"), lost.failed_cmd.as_deref())
    );
    let stored = f
        .ctx
        .store
        .blob_bytes(row.log.as_ref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(stored, partial);
    assert_eq!(scratch("A1"), corral_core::backend::ScratchState::Reclaimed);
    f.assert_not_completed(&main);
}
