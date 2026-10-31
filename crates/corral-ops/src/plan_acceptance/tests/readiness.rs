use super::*;
use corral_core::work::{self, Counter, HotScope};
use corral_core::{ArtifactKind, Task};

fn snapshot_path(f: &Fixture, artifact: &corral_core::Artifact) -> std::path::PathBuf {
    let hash = corral_core::blob_ref::BlobHash::parse(&artifact.sha256).unwrap();
    f.ctx.store.blob_store().unwrap().path_of(&hash)
}

fn one_member(delivery: Delivery) -> Fixture {
    let mut f = Fixture::new(delivery, &["true"]);
    f.ctx.resource_probe = std::sync::Arc::new(crate::resources::PinnedReadingsProbe::admitting());
    let mut tracker = f.plan();
    tracker.children.truncate(1);
    f.ctx.store.fixture_remove_task(&f.members[1]).unwrap();
    f.ctx.store.fixture_put_task(&tracker).unwrap();
    f
}

fn add(f: &Fixture, range: std::ops::Range<usize>) {
    for n in range {
        let source = f._dir.path().join(format!("proof-{n:04}.txt"));
        std::fs::write(&source, format!("accepted proof {n:04}\n")).unwrap();
        crate::add_artifact(
            &f.ctx,
            &f.members[0],
            &source,
            Some(ArtifactKind::Evidence),
            Some("Accepted proof"),
        )
        .unwrap();
    }
    let mut child = f.ctx.store.get(&f.members[0]).unwrap();
    child.updated = "2026-10-19T00:00:00Z".parse().unwrap();
    for artifact in &mut child.artifacts {
        artifact.added = child.updated;
    }
    f.ctx.store.fixture_put_task(&child).unwrap();
}

/// The full capture remains the integrity-boundary oracle, not the readiness shortcut.
fn full_blockers(f: &Fixture, tracker: &Task) -> Vec<String> {
    let store = f.ctx.store.read_only_snapshot().unwrap();
    match super::super::capture::capture(&f.ctx, &store, tracker, "readiness", None) {
        Ok(_) => Vec::new(),
        Err(error) => match error.downcast::<super::super::capture::Unmet>() {
            Ok(unmet) => unmet.0,
            Err(error) => vec![format!("{error:#}")],
        },
    }
}

fn measured(f: &Fixture) -> (serde_json::Value, work::WorkSnapshot) {
    // Warm the production projection at this size, just as in the review reproduction.
    crate::plan_progress(&f.ctx, &f.plan).unwrap();
    let before = work::snapshot();
    let progress = {
        let _hot = HotScope::enter();
        crate::plan_progress(&f.ctx, &f.plan).unwrap()
    };
    let report = work::snapshot().since(&before);
    assert_eq!(report.unattributed_hydrations(), 0);
    assert_eq!(report.get(Counter::HistoryRowDecodes), 0);
    report.check_identity().unwrap();
    let aggregate = progress.aggregate.as_ref().unwrap();
    assert_eq!(aggregate.blockers, full_blockers(f, &f.plan()));
    assert_eq!(aggregate.ready, aggregate.blockers.is_empty());
    let value = serde_json::to_value(progress).unwrap();
    assert_eq!(value, full_progress(f));
    (value, report)
}

/// db316644's full-task projection for this one-member, dependency-free fixture.
fn full_progress(f: &Fixture) -> serde_json::Value {
    let tracker = f.plan();
    let child = f.ctx.store.get(&f.members[0]).unwrap();
    let acceptance = tracker.approved_scope.as_ref().unwrap().acceptance.clone();
    let blockers = full_blockers(f, &tracker);
    json!({
        "task": tracker.id, "status": tracker.status, "approved_scope": tracker.approved_scope,
        "scope_revision": tracker.approved_scope_revision,
        "counts": {"accepted":1, "running":0, "review":0, "waiting":0, "missing":0},
        "work": [{
            "task":child.id, "title":child.title, "status":child.status, "state":"accepted",
            "run":child.current_run().map(|run| &run.id), "reason":null, "prerequisites":[]
        }],
        "coverage":acceptance.iter().map(|criterion| json!({
            "acceptance":criterion, "tasks":[]
        })).collect::<Vec<_>>(),
        "unmapped_criteria":acceptance,
        "aggregate":{
            "ready":blockers.is_empty(), "blockers":blockers,
            "attempt":tracker.aggregate_attempts.last().map(|attempt| &attempt.id),
            "accepted":false, "acceptance":null
        }
    })
}

fn growth(delivery: Delivery, missing_git: bool) {
    // Given an approved Plan with criteria and one completed, accepted R1 member.
    let mut f = one_member(delivery);
    if missing_git {
        f.ctx.store.config.projects.get_mut("app").unwrap().path =
            f.ctx.store.root().to_string_lossy().into_owned();
        f.ctx.store.save_config().unwrap();
    }
    f.ctx
        .store
        .log_event(
            Some(&f.members[0]),
            "approved",
            json!({"run":"R1", "sha":f.main(), "strategy":"rebase", "base":"main"}),
        )
        .unwrap();
    add(&f, 0..4);
    let (base, small) = measured(&f);

    // When same-run accepted artifacts grow tenfold, with a warmup at the new size.
    add(&f, 4..40);
    let (grown, large) = measured(&f);

    // Then every output and every unattributed cell stays identical.
    assert_eq!(base, grown);
    for counter in Counter::ALL {
        assert_eq!(small.get(counter), large.get(counter), "{counter:?}");
    }
}

#[test]
fn accepted_code_readiness_has_constant_warm_artifact_work() {
    growth(Delivery::Main, false);
}

#[test]
fn accepted_no_code_readiness_has_constant_warm_artifact_work() {
    growth(Delivery::NoCode, false);
}

#[test]
fn accepted_readiness_before_missing_git_has_constant_artifact_work() {
    growth(Delivery::Main, true);
}

#[test]
fn no_code_artifact_purge_invalidates_warm_readiness_with_full_output_parity() {
    // Given a ready, warmed Plan with intact accepted delivery.
    let f = one_member(Delivery::NoCode);
    add(&f, 0..4);
    assert!(measured(&f).0["aggregate"]["ready"].as_bool().unwrap());

    // When a corral-mediated edit purges the first accepted artifact.
    f.ctx
        .store
        .update(&f.members[0], |child| {
            child.artifacts[0].purged_at = Some("2026-10-20T00:00:00Z".parse().unwrap());
            Ok(())
        })
        .unwrap();
    let (base, small) = measured(&f);
    add(&f, 4..40);
    let (grown, large) = measured(&f);

    // Then readiness immediately matches the full blocker, without a history-sized read.
    assert!(!base["aggregate"]["ready"].as_bool().unwrap());
    assert_eq!(base, grown);
    for counter in Counter::ALL {
        assert_eq!(small.get(counter), large.get(counter), "{counter:?}");
    }
}

#[test]
fn code_member_purged_artifacts_keep_the_full_readiness_rule() {
    // Given code delivery whose accepted proof is its integrated commit.
    let f = one_member(Delivery::Main);
    add(&f, 0..4);
    measured(&f);
    // When the member's snapshots are purged.
    f.ctx
        .store
        .update(&f.members[0], |child| {
            child.artifacts[0].purged_at = Some("2026-10-20T00:00:00Z".parse().unwrap());
            Ok(())
        })
        .unwrap();
    // Then the full path and read-only readiness both continue to accept that member.
    assert!(measured(&f).0["aggregate"]["ready"].as_bool().unwrap());
}

#[test]
fn verification_publishes_integrity_failure_and_recovery_to_readiness() {
    // Given an intact accepted no-code snapshot and a pinned ready evaluation.
    let f = one_member(Delivery::NoCode);
    add(&f, 0..1);
    let original = crate::plan_progress(&f.ctx, &f.plan).unwrap();
    let pinned = Ctx {
        store: f.ctx.store.read_only_snapshot().unwrap(),
        ..f.ctx.clone()
    };
    let artifact = f.ctx.store.get(&f.members[0]).unwrap().artifacts[0].clone();
    let path = snapshot_path(&f, &artifact);
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, "changed outside corral\n").unwrap();
    // Readback deliberately does not discover filesystem corruption before a check.
    assert!(
        crate::plan_progress(&f.ctx, &f.plan)
            .unwrap()
            .aggregate
            .unwrap()
            .ready
    );

    // When the real verification boundary checks the accepted bytes.
    let failure = message(plan_verify(&f.ctx, &f.plan));
    let progress = measured(&f).0;

    // Then the failure is durable and exactly matches the full path, without changing old views.
    assert!(!progress["aggregate"]["ready"].as_bool().unwrap());
    assert!(failure.contains(&artifact.path), "{failure}");
    assert_eq!(
        serde_json::to_value(crate::plan_progress(&pinned, &f.plan).unwrap()).unwrap(),
        serde_json::to_value(original).unwrap()
    );
    let fresh_ctx = Ctx {
        store: Store::open(f.ctx.store.root()).unwrap(),
        ..f.ctx.clone()
    };
    assert_eq!(
        serde_json::to_value(crate::plan_progress(&fresh_ctx, &f.plan).unwrap()).unwrap(),
        progress
    );

    // A subsequent real check also publishes recovery of the same metadata revision.
    std::fs::write(&path, bytes).unwrap();
    plan_verify(&f.ctx, &f.plan).unwrap();
    assert!(measured(&f).0["aggregate"]["ready"].as_bool().unwrap());
}

#[test]
fn acceptance_rechecks_no_code_bytes_and_publishes_the_exact_blocker() {
    // Given an independently reviewed aggregate attempt with an accepted no-code snapshot.
    let f = one_member(Delivery::NoCode);
    add(&f, 0..1);
    let attempt = f.reviewed();
    let artifact = f.ctx.store.get(&f.members[0]).unwrap().artifacts[0].clone();
    std::fs::write(snapshot_path(&f, &artifact), "changed after review\n").unwrap();

    // When acceptance rechecks the real proof, rather than trusting readiness metadata.
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, false));

    // Then acceptance refuses, and readback reports the full path's unchanged blocker.
    assert!(error.contains(&artifact.path), "{error}");
    assert!(!measured(&f).0["aggregate"]["ready"].as_bool().unwrap());
    assert_ne!(f.plan().status, Status::Done);
}

#[test]
fn refused_acceptance_prepare_keeps_its_integrity_observation() {
    // Given no-code bytes that change after a successful integrity boundary.
    let f = one_member(Delivery::NoCode);
    add(&f, 0..1);
    let attempt = plan_verify(&f.ctx, &f.plan).unwrap();
    let artifact = f.ctx.store.get(&f.members[0]).unwrap().artifacts[0].clone();
    std::fs::write(snapshot_path(&f, &artifact), "changed during prepare\n").unwrap();
    let before = f.plan();

    // When the full acceptance check refuses inside the task's buffered update unit.
    let error = message(f.ctx.store.update(&f.plan, |tracker| {
        super::super::capture::capture(
            &f.ctx,
            &f.ctx.store,
            tracker,
            &attempt.id,
            Some(&attempt.identity.environment),
        )
        .map(|_| ())
    }));

    // Then the task write is refused but the exact integrity blocker survives durably.
    assert!(error.contains(&artifact.path), "{error}");
    assert_eq!(f.plan(), before);
    assert!(!measured(&f).0["aggregate"]["ready"].as_bool().unwrap());
}

#[test]
fn readiness_keeps_registration_order_between_purge_and_integrity_blockers() {
    for (changed, purged) in [(0, 2), (2, 0)] {
        // Given a purged registration and a different corrupt snapshot in either order.
        let f = one_member(Delivery::NoCode);
        add(&f, 0..4);
        f.ctx
            .store
            .update(&f.members[0], |child| {
                child.artifacts[purged].purged_at = Some("2026-10-20T00:00:00Z".parse().unwrap());
                Ok(())
            })
            .unwrap();
        let child = f.ctx.store.get(&f.members[0]).unwrap();
        std::fs::write(snapshot_path(&f, &child.artifacts[changed]), "corrupt\n").unwrap();

        // When a real boundary observes the first failure in registration order.
        assert!(plan_verify(&f.ctx, &f.plan).is_err());

        // Then readiness's maintained purge and integrity selectors preserve that order.
        let value = measured(&f).0;
        assert_eq!(
            value["aggregate"]["blockers"],
            json!([corral_core::accepted_proof::ProofFailure {
                ordinal: u64::try_from(changed.min(purged) + 1).unwrap(),
                path: child.artifacts[changed.min(purged)].path.clone(),
                kind: if changed < purged {
                    corral_core::accepted_proof::ProofFailureKind::MissingOrChanged
                } else {
                    corral_core::accepted_proof::ProofFailureKind::Purged
                }
            }
            .blocker(&f.members[0])])
        );
    }
}

#[test]
fn warm_whole_plan_progress_includes_preparation_and_integrity_observations() {
    // Given real boundary observations, not test-only readiness preparation.
    let f = one_member(Delivery::NoCode);
    let mut reports = Vec::new();
    let mut outputs = Vec::new();
    for range in [0..4, 4..40] {
        add(&f, range);
        let child = f.ctx.store.get(&f.members[0]).unwrap();
        let artifact = &child.artifacts[0];
        std::fs::write(snapshot_path(&f, artifact), "corrupt snapshot\n").unwrap();
        assert!(plan_verify(&f.ctx, &f.plan).is_err());
        let observed = corral_core::records::RecordArea::<
            corral_core::accepted_proof::IntegrityCheck,
        >::new(corral_core::accepted_proof::INTEGRITY_CHECKS)
        .load(&f.ctx.store, &format!("{}/R1", child.id))
        .unwrap()
        .unwrap();
        assert_eq!(observed.failure.as_ref().unwrap().path, artifact.path);
        assert!(f.plan().aggregate_attempts.is_empty());
        let first = crate::plan_progress(&f.ctx, &f.plan).unwrap();
        let before = work::snapshot();

        // When the entire second production call is measured, including store opening,
        // migration preparation, snapshot creation and the keyed integrity observation.
        let second = {
            let _whole_call = HotScope::enter();
            crate::plan_progress(&f.ctx, &f.plan).unwrap()
        };
        let report = work::snapshot().since(&before);

        // Then no preparation is hidden outside the guard, and output equals the full oracle.
        let output = serde_json::to_value(second).unwrap();
        assert_eq!(output, serde_json::to_value(first).unwrap());
        assert_eq!(output, full_progress(&f));
        assert_eq!(report.get(Counter::HistoryRowDecodes), 0);
        assert!(report.get(Counter::RecordDecodes) > 0);
        assert_eq!(report.unattributed_hydrations(), 0);
        report.check_identity().unwrap();
        reports.push(report);
        outputs.push(output);
    }
    assert_eq!(outputs[0], outputs[1]);
    for counter in Counter::ALL {
        assert_eq!(
            reports[0].get(counter),
            reports[1].get(counter),
            "{counter:?}"
        );
    }
}
