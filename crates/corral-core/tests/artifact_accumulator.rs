//! Independent byte-level recomputation versus incremental publication/replacement.
mod store_support;
use corral_core::backend::fake::FakeBackend;
use corral_core::backend::{BackendConfig, FaultInjector, ManualClock};
use corral_core::work::{self, Counter, HotScope};
use corral_core::{Artifact, ArtifactKind, ArtifactTier, Status, Store};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use store_support::{at, coding_run, store};

fn artifact(run: &str, n: usize) -> Artifact {
    Artifact {
        run: run.into(),
        kind: ArtifactKind::Log,
        title: format!("sample-{n:04}"),
        path: format!("artifacts/T-001/{run}/{n:04}.jsonl"),
        source: format!("/source/{n:04}"),
        sha256: format!("{n:064x}"),
        bytes: 3,
        added: at(0),
        tier_override: None,
        compressed_bytes: None,
        purged_at: None,
    }
}

fn model(store: &Store, id: &str) -> String {
    let task = store.get(id).unwrap();
    let run = &task.current_run().unwrap().id;
    let artifacts: Vec<_> = task.artifacts.iter().filter(|a| &a.run == run).collect();
    // Independent binary XOR, not the production hex-string implementation or fold methods.
    let mut expected: [u8; 32] = Sha256::digest(b"corral.accepted-proof.v2").into();
    for (n, artifact) in artifacts.iter().enumerate() {
        let mut digest = Sha256::new();
        digest.update(b"corral.accepted-proof.v2/registration");
        digest.update(u64::try_from(n + 1).unwrap().to_be_bytes());
        digest.update(format!("{artifact:?}").as_bytes());
        for (sum, byte) in expected.iter_mut().zip(digest.finalize()) {
            *sum ^= byte;
        }
    }
    let token = format!(
        "v2:{}",
        expected
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    let head = store.head(id).unwrap();
    let proof = &head.summary.current_artifacts.accepted_proof;
    assert_eq!(proof.revision, token);
    assert_eq!(proof.count, u64::try_from(artifacts.len()).unwrap());
    assert_eq!(head.summary.current_artifacts.count, proof.count);
    let first = artifacts
        .iter()
        .enumerate()
        .find(|(_, a)| a.purged_at.is_some());
    assert_eq!(
        proof
            .first_purged
            .as_ref()
            .map(|f| (f.ordinal, f.path.as_str())),
        first.map(|(n, a)| (u64::try_from(n + 1).unwrap(), a.path.as_str()))
    );
    assert_eq!(
        head.summary.current_artifacts.latest_diff,
        artifacts
            .iter()
            .rev()
            .find(|a| a.kind == ArtifactKind::Diff)
            .map(|a| a.title.clone())
    );
    token
}

#[test]
fn independent_model_covers_every_field_duplicates_reordering_empty_runs_and_revival_on_both_backends()
 {
    for fake in [false, true] {
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
            .create("Accumulator", None, Status::Running, "")
            .unwrap();
        task.runs.push(coding_run(&task.id, "R1", 0, 0, 0, false));
        store.fixture_put_task(&task).unwrap();
        let empty = model(&store, &task.id);
        for n in 0..3 {
            store
                .update_head(&task.id, |h| h.add_artifact(artifact("R1", n)))
                .unwrap();
            model(&store, &task.id);
        }
        let before_duplicate = model(&store, &task.id);
        store
            .update_head(&task.id, |h| h.add_artifact(artifact("R1", 1)))
            .unwrap();
        assert_ne!(
            model(&store, &task.id),
            before_duplicate,
            "duplicate multiplicity has its own ordinal"
        );
        for i in 0..4 {
            store
                .update_head(&task.id, |h| {
                    h.edit_artifact(i, |a| {
                        a.kind = ArtifactKind::Diff;
                        a.title = format!("Final diff +{} −1", i + 1);
                        a.source = format!("/changed/{i}");
                        a.bytes += 7;
                        a.added = at(10);
                        a.tier_override = Some(ArtifactTier::Transient);
                        a.compressed_bytes = Some(2);
                        a.purged_at = Some(at(11));
                        Ok(())
                    })
                })
                .unwrap();
            model(&store, &task.id);
        }
        for i in 0..4 {
            store
                .update_head(&task.id, |h| {
                    h.edit_artifact(i, |a| {
                        a.purged_at = None;
                        a.compressed_bytes = None;
                        Ok(())
                    })
                })
                .unwrap();
            model(&store, &task.id);
        }
        let original = model(&store, &task.id);
        store
            .update(&task.id, |t| {
                t.artifacts.swap(0, 3);
                Ok(())
            })
            .unwrap();
        assert_ne!(
            model(&store, &task.id),
            original,
            "registration order is included"
        );
        store
            .update(&task.id, |t| {
                t.artifacts[0].sha256 = "changed-content".into();
                t.artifacts[0].path.push_str("-renamed");
                t.artifacts[0].run = "R9".into();
                Ok(())
            })
            .unwrap();
        model(&store, &task.id);
        store
            .update(&task.id, |t| {
                t.runs.push(coding_run(&t.id, "R2", 0, 0, 0, false));
                Ok(())
            })
            .unwrap();
        assert_eq!(
            model(&store, &task.id),
            empty,
            "empty runs use the versioned empty accumulator"
        );
        store
            .update_head(&task.id, |h| h.add_artifact(artifact("R1", 10)))
            .unwrap();
        assert_eq!(
            model(&store, &task.id),
            empty,
            "historical publication cannot alter the current run"
        );
        store
            .update(&task.id, |t| {
                t.runs.swap(0, 1);
                Ok(())
            })
            .unwrap();
        model(&store, &task.id);
    }
}

#[test]
fn current_and_historical_append_and_current_revival_have_equal_work_at_one_and_tenfold_history() {
    for fake in [false, true] {
        let mut results = Vec::new();
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
            let mut task = store.create("Bounded", None, Status::Running, "").unwrap();
            task.runs = vec![
                coding_run(&task.id, "R0", 0, 0, 0, true),
                coding_run(&task.id, "R1", 0, 0, 0, false),
            ];
            let mut first = artifact("R1", 0);
            first.purged_at = Some(at(1));
            task.artifacts.push(first);
            for n in 1..scale * 10 {
                task.artifacts.push(artifact("R1", n));
                task.artifacts.push(artifact("R0", n));
            }
            store.fixture_put_task(&task).unwrap();
            let hot = HotScope::enter();
            let before = work::snapshot();
            store
                .update_head(&task.id, |h| {
                    h.edit_artifact(0, |a| {
                        a.purged_at = None;
                        Ok(())
                    })?;
                    h.add_artifact(artifact("R1", 1000))?;
                    h.add_artifact(artifact("R0", 1000))?;
                    h.add_artifact(artifact("R0", 1001))
                })
                .unwrap();
            let delta = work::snapshot().since(&before);
            drop(hot);
            delta.check_identity().unwrap();
            assert_eq!(delta.unattributed_hydrations(), 0);
            results.push(Counter::ALL.map(|c| delta.get(c)));
            model(&store, &task.id);
            let full = store.get(&task.id).unwrap();
            let raw = store
                .record::<String>("artifact-run-counts", &format!("{}/R0", task.id))
                .unwrap()
                .unwrap();
            assert_eq!(
                raw.parse::<usize>().unwrap(),
                full.artifacts.iter().filter(|a| a.run == "R0").count()
            );
        }
        assert_eq!(results[0], results[1]);
    }
}
