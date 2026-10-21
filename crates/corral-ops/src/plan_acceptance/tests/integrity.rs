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
    let snapshot = f.ctx.store.dir().join(&fresh.path);
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
    std::fs::write(f.ctx.store.dir().join(&report.path), "rewritten\n").unwrap();
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
