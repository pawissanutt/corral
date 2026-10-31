//! First-registration dedup uses canonical rows, never filename, time or live-state preference.
mod store_support;
use corral_core::backend::fake::FakeBackend;
use corral_core::backend::{BackendConfig, FaultInjector, ManualClock};
use corral_core::work::{self, Counter, HotScope};
use corral_core::{Artifact, ArtifactKind, Status};
use std::sync::Arc;
use store_support::{at, store};

fn artifact(run: &str, sha: &str, file: &str, purged: bool) -> Artifact {
    Artifact {
        run: run.into(),
        kind: ArtifactKind::Log,
        title: file.into(),
        path: format!("artifacts/T-001/{run}/{file}"),
        source: format!("/source/{file}"),
        sha256: sha.into(),
        bytes: 3,
        added: at(0),
        tier_override: None,
        compressed_bytes: purged.then_some(0),
        purged_at: purged.then_some(at(1)),
    }
}

#[test]
fn digest_lookup_matches_full_first_position_after_append_edit_and_rollback_on_both_backends() {
    for fake in [false, true] {
        let mut measures = Vec::new();
        for scale in [1, 10] {
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
            store.fixture_pin_now(at(0)).unwrap();
            let mut task = store
                .create("Digest lookup", None, Status::Review, "")
                .unwrap();
            task.artifacts = vec![
                artifact("R0", "same", "earlier-other-run", false),
                artifact("R1", "same", "old-purged-name", true),
                artifact("R1", "same", "new-live-name", false),
            ];
            for n in 0..scale * 10 {
                task.artifacts
                    .push(artifact("R1", &format!("other-{n}"), "unrelated", false));
            }
            store.fixture_put_task(&task).unwrap();
            // Expected identity and ordinal are derived independently from the unchanged full API.
            let full = store.get(&task.id).unwrap();
            let expected = full
                .artifacts
                .iter()
                .enumerate()
                .find(|(_, a)| a.run == "R1" && a.sha256 == "same")
                .map(|(i, a)| (i, a.clone()));
            let head = store.head(&task.id).unwrap();
            let hot = HotScope::enter();
            let before = work::snapshot();
            assert_eq!(
                head.artifact_registration_by_sha("R1", "same").unwrap(),
                expected
            );
            assert!(
                head.artifact_registration_by_sha("R1", "absent")
                    .unwrap()
                    .is_none()
            );
            let delta = work::snapshot().since(&before);
            drop(hot);
            delta.check_identity().unwrap();
            assert_eq!(delta.get(Counter::HistoryRowDecodes), 1);
            assert_eq!(delta.unattributed_hydrations(), 0);
            measures.push(Counter::ALL.map(|c| delta.get(c)));
            // Full edits remove the old token; the next matching ordinal becomes first.
            store
                .update(&task.id, |t| {
                    t.artifacts[1].sha256 = "changed".into();
                    Ok(())
                })
                .unwrap();
            assert_eq!(
                store
                    .head(&task.id)
                    .unwrap()
                    .artifact_registration_by_sha("R1", "same")
                    .unwrap(),
                Some((2, full.artifacts[2].clone()))
            );
            let refused = store.try_update_head(&task.id, |t| {
                t.add_artifact(artifact("R2", "same", "not-published", false))?;
                anyhow::bail!("refused")
            });
            assert!(refused.is_err());
            assert!(
                store
                    .head(&task.id)
                    .unwrap()
                    .artifact_registration_by_sha("R2", "same")
                    .unwrap()
                    .is_none()
            );
            store
                .update_head(&task.id, |t| {
                    t.add_artifact(artifact("R2", "same", "published", false))
                })
                .unwrap();
            assert_eq!(
                store
                    .head(&task.id)
                    .unwrap()
                    .artifact_registration_by_sha("R2", "same")
                    .unwrap()
                    .unwrap()
                    .0,
                full.artifacts.len()
            );
            // Delimiters in legacy identities cannot alias another run/content pair.
            store
                .update_head(&task.id, |t| {
                    t.add_artifact(artifact("R:2", "x", "delimited", false))
                })
                .unwrap();
            assert!(
                store
                    .head(&task.id)
                    .unwrap()
                    .artifact_registration_by_sha("R", "2:x")
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(measures[0], measures[1]);
    }
}
