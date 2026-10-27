mod store_support;

use std::sync::Arc;

use corral_core::accepted_proof::{
    INTEGRITY_CHECKS, IntegrityCheck, ProofFailure, ProofFailureKind,
};
use corral_core::backend::fake::FakeBackend;
use corral_core::backend::{BackendConfig, FaultInjector, ManualClock};
use corral_core::{Artifact, ArtifactTier, Status};
use store_support::{at, coding_run, store};

#[test]
fn accepted_proof_facts_and_observations_follow_artifact_changes_on_both_backends() {
    for fake in [false, true] {
        // Given a current run with an accepted registration and a published integrity failure.
        let (_dir, base) = store();
        let store = if fake {
            base.with_backend(Box::new(FakeBackend::new(
                BackendConfig::default(),
                Arc::new(ManualClock::new(at(0))),
                Arc::new(FaultInjector::default()),
            )))
        } else {
            base
        };
        let mut task = store.create("Accepted", None, Status::Done, "").unwrap();
        task.runs.push(coding_run(&task.id, "R1", 0, 0, 0, false));
        store.fixture_put_task(&task).unwrap();
        let artifact: Artifact = serde_json::from_value(serde_json::json!({
            "run":"R1", "kind":"evidence", "title":"Proof", "path":"artifacts/proof.txt",
            "source":"/tmp/proof.txt", "sha256":"0".repeat(64), "bytes":1, "added":at(0),
        }))
        .unwrap();
        store
            .update_head(&task.id, |head| head.add_artifact(artifact.clone()))
            .unwrap();
        let key = format!("{}/R1", task.id);
        let proof = store
            .head(&task.id)
            .unwrap()
            .summary
            .current_artifacts
            .accepted_proof;
        assert_eq!(proof.count, 1);
        let checked = IntegrityCheck {
            revision: proof.revision.clone(),
            failure: Some(ProofFailure {
                ordinal: 1,
                path: artifact.path.clone(),
                kind: ProofFailureKind::MissingOrChanged,
            }),
        };
        store.save_record(INTEGRITY_CHECKS, &key, &checked).unwrap();
        let frozen = store.read_only_snapshot().unwrap();

        // When unrelated metadata changes, the observation remains; artifact edits retire it.
        store
            .update(&task.id, |t| {
                t.title = "Renamed".into();
                Ok(())
            })
            .unwrap();
        assert_eq!(
            store
                .record::<IntegrityCheck>(INTEGRITY_CHECKS, &key)
                .unwrap(),
            Some(checked.clone())
        );
        for change in 0..5 {
            let before = store
                .head(&task.id)
                .unwrap()
                .summary
                .current_artifacts
                .accepted_proof;
            store
                .save_record(
                    INTEGRITY_CHECKS,
                    &key,
                    &IntegrityCheck {
                        revision: before.revision.clone(),
                        ..checked.clone()
                    },
                )
                .unwrap();
            store
                .update(&task.id, |t| {
                    match change {
                        0 => t.artifacts[0].tier_override = Some(ArtifactTier::Transient),
                        1 => t.artifacts[0].purged_at = Some(at(2)),
                        2 => t.artifacts[0].purged_at = None,
                        3 => t.artifacts[0].compressed_bytes = Some(10),
                        4 => t.artifacts[0].sha256 = "1".repeat(64),
                        _ => unreachable!(),
                    }
                    Ok(())
                })
                .unwrap();
            let after = store
                .head(&task.id)
                .unwrap()
                .summary
                .current_artifacts
                .accepted_proof;
            assert_ne!(before.revision, after.revision);
            assert_eq!(after.first_purged.is_some(), change == 1);
            assert!(
                store
                    .record::<IntegrityCheck>(INTEGRITY_CHECKS, &key)
                    .unwrap()
                    .is_none()
            );
        }
        // Then a pinned read still sees exactly its original facts and observation.
        assert_eq!(
            frozen
                .head(&task.id)
                .unwrap()
                .summary
                .current_artifacts
                .accepted_proof,
            proof
        );
        assert_eq!(
            frozen
                .record::<IntegrityCheck>(INTEGRITY_CHECKS, &key)
                .unwrap(),
            Some(checked.clone())
        );
        let next = format!("{}/R2", task.id);
        store
            .save_record(INTEGRITY_CHECKS, &next, &checked)
            .unwrap();
        store
            .update_head(&task.id, |head| {
                head.start_run(coding_run(&task.id, "R2", 1, 0, 0, false))
            })
            .unwrap();
        assert!(
            store
                .record::<IntegrityCheck>(INTEGRITY_CHECKS, &next)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store
                .head(&task.id)
                .unwrap()
                .summary
                .current_artifacts
                .accepted_proof
                .count,
            0
        );
    }
}
