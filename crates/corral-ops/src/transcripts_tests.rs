use super::*;
use corral_core::{ArtifactKind, ArtifactTier, RetentionAction, Status, Store};
use serde_json::json;

struct Fx {
    _tmp: tempfile::TempDir,
    ctx: Ctx,
    task: String,
}

fn fx(pi: bool) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.sync.omo_agent_dir = Some(tmp.path().join("provider"));
    let ctx = crate::resources::FixedResourceProbe::ctx(store);
    let task = ctx
        .store
        .create("Transcripts", None, Status::Running, "")
        .unwrap()
        .id;
    ctx.store
        .update(&task, |t| {
            let run: Run = serde_json::from_value(json!({
                "id":"R1", "agent":"omo", "harness": if pi {"pi-native"} else {"omo"},
                "worktree":"/fixture/project", "branch":"test", "base":"main", "base_sha":"abc",
                "started":"2026-09-01T00:00:00Z", "ended":"2026-11-01T00:00:00Z"
            }))
            .unwrap();
            let mut child = run.clone();
            child.id = "R1-review1".into();
            t.runs.push(run);
            t.runs[0].review_runs.push(child);
            Ok(())
        })
        .unwrap();
    Fx {
        _tmp: tmp,
        ctx,
        task,
    }
}

fn owner(f: &Fx, review: bool) -> ContainerRef {
    if review {
        ContainerRef::Reviewer {
            task: f.task.clone(),
            run: "R1".into(),
            review: "R1-review1".into(),
        }
    } else {
        ContainerRef::Worker {
            task: f.task.clone(),
            run: "R1".into(),
        }
    }
}

fn pi_source(f: &Fx, review: bool, bytes: &[u8]) -> std::path::PathBuf {
    let dir = corral_core::agent::paths::container_sessions_dir(&f.ctx.store, &owner(f, review));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("unremarkable.jsonl");
    std::fs::write(&path, bytes).unwrap();
    path
}

fn finish(f: &Fx, status: Status, days: i64) {
    f.ctx
        .store
        .update(&f.task, |t| {
            t.status = status;
            Ok(())
        })
        .unwrap();
    let at = jiff::Timestamp::now()
        .checked_sub(jiff::SignedDuration::from_hours(days * 24))
        .unwrap();
    f.ctx
        .store
        .fixture_replace_events(&format!(
            "{}\n",
            json!({
                "seq":0, "ts":at, "kind":"moved", "task":f.task,
                "detail":{"to":status.label()}
            })
        ))
        .unwrap();
}

fn live(f: &Fx) -> Vec<Artifact> {
    f.ctx
        .store
        .get(&f.task)
        .unwrap()
        .artifacts
        .into_iter()
        .filter(|a| a.purged_at.is_none())
        .collect()
}

#[test]
fn pi_run_end_promotes_worker_and_child_without_removing_sessions() {
    let f = fx(true);
    let worker = pi_source(&f, false, b"worker session\n");
    let review = pi_source(&f, true, b"review session\n");
    finish(&f, Status::Dropped, 31);
    let task = f.ctx.store.get(&f.task).unwrap();
    crate::agent::hooks::cleanup::run_end_in(&f.ctx, &task, f._tmp.path()).unwrap();
    let artifacts = live(&f);
    assert_eq!(artifacts.len(), 2);
    for (path, review) in [(worker, false), (review, true)] {
        let bytes = std::fs::read(path).unwrap();
        let a = artifacts
            .iter()
            .find(|a| a.sha256 == corral_core::agent::paths::sha256_hex(&bytes))
            .unwrap();
        assert_eq!(owners(&f.ctx, &f.task, a).unwrap(), vec![owner(&f, review)]);
        assert_eq!(a.run, "R1");
        assert!(
            f.ctx
                .store
                .blob_row(&corral_core::blob_ref::BlobHash::parse(&a.sha256).unwrap())
                .unwrap()
                .is_some()
        );
        assert_eq!(crate::artifact_read(&f.ctx, a).unwrap(), bytes);
    }
}

#[test]
fn an_absent_provider_session_is_not_a_failed_capture() {
    let f = fx(false);
    finish(&f, Status::Dropped, 31);
    run_end(&f.ctx, &f.ctx.store.get(&f.task).unwrap()).unwrap();
    assert!(live(&f).is_empty());
}

#[test]
fn raw_worker_capture_uses_provider_window_and_never_removes_provider_files() {
    let mut f = fx(false);
    let dir = f
        .ctx
        .store
        .config
        .sync
        .omo_agent_dir
        .as_ref()
        .unwrap()
        .join("sessions")
        .join(crate::omo::encode_cwd(Path::new("/fixture/project")));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("actual.jsonl");
    let body = concat!(
        "{\"type\":\"session\",\"cwd\":\"/fixture/project\",\"timestamp\":\"2026-09-01T00:00:00Z\"}\n",
        "{\"type\":\"message\",\"timestamp\":\"2026-09-02T00:00:00Z\",\"message\":{\"role\":\"assistant\"}}\n",
        "{\"type\":\"message\",\"timestamp\":\"2026-11-02T00:00:00Z\",\"message\":{\"role\":\"assistant\"}}\n"
    );
    std::fs::write(&path, body).unwrap();
    finish(&f, Status::Done, 31);
    run_end(&f.ctx, &f.ctx.store.get(&f.task).unwrap()).unwrap();
    let artifacts = live(&f);
    assert_eq!(artifacts.len(), 1);
    let a = &artifacts[0];
    assert_eq!(owners(&f.ctx, &f.task, a).unwrap(), vec![owner(&f, false)]);
    let captured = String::from_utf8(crate::artifact_read(&f.ctx, a).unwrap()).unwrap();
    assert!(captured.contains("2026-09-02"));
    assert!(!captured.contains("2026-11-02"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), body);
    crate::artifact_gc(&f.ctx, false).unwrap();
    assert_eq!(live(&f).len(), 1, "no retention configured");
    f.ctx.store.config.storage.transcript_retention_days = Some(30);
    finish(&f, Status::Done, 29);
    crate::artifact_gc(&f.ctx, false).unwrap();
    assert_eq!(live(&f).len(), 1, "before retention horizon");
    finish(&f, Status::Done, 31);
    crate::artifact_gc(&f.ctx, false).unwrap();
    assert!(live(&f).is_empty());
    run_end(&f.ctx, &f.ctx.store.get(&f.task).unwrap()).unwrap();
    assert!(
        live(&f).is_empty(),
        "cleanup must not resurrect a retired raw transcript"
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), body);
}

#[test]
fn retention_matrix_uses_real_pi_lifecycle_and_storage_gc() {
    for (status, days, setting, purge) in [
        (Status::Done, 31, None, false),
        (Status::Done, 29, Some(30), false),
        (Status::Done, 31, Some(30), true),
        (Status::Dropped, 31, Some(30), true),
        (Status::Running, 90, Some(30), false),
        (Status::Review, 90, Some(30), false),
    ] {
        let mut f = fx(true);
        f.ctx.store.config.storage.transcript_retention_days = setting;
        pi_source(&f, false, b"raw session content\n");
        finish(&f, Status::Done, days);
        run_end(&f.ctx, &f.ctx.store.get(&f.task).unwrap()).unwrap();
        // A previously terminal task reopened: an old terminal event must not age it out.
        f.ctx
            .store
            .update(&f.task, |t| {
                t.status = status;
                Ok(())
            })
            .unwrap();
        if status == Status::Dropped {
            finish(&f, status, days);
        }
        let dry = crate::storage_gc::storage_gc(&f.ctx, false).unwrap();
        assert_eq!(
            dry.entries.iter().any(|e| e.class == "transcript"),
            purge,
            "{status:?} {days} {setting:?}"
        );
        assert_eq!(live(&f).len(), 1, "dry run is read only");
        crate::storage_gc::storage_gc(&f.ctx, true).unwrap();
        assert_eq!(live(&f).is_empty(), purge, "{status:?} {days} {setting:?}");
    }
}

#[test]
fn pins_and_same_content_keep_tier_references_protect_transcripts() {
    for pin in [false, true] {
        let mut f = fx(true);
        f.ctx.store.config.storage.transcript_retention_days = Some(30);
        let source = pi_source(&f, false, b"protected session\n");
        finish(&f, Status::Done, 31);
        run_end(&f.ctx, &f.ctx.store.get(&f.task).unwrap()).unwrap();
        let a = live(&f).remove(0);
        if pin {
            f.ctx
                .store
                .pin(
                    &corral_core::PinRef::of_artifact(&f.task, &a).to_string(),
                    None,
                    false,
                )
                .unwrap();
        } else {
            let other = f
                .ctx
                .store
                .create("keep reference", None, Status::Ready, "")
                .unwrap();
            let kept =
                crate::add_artifact(&f.ctx, &other.id, &source, Some(ArtifactKind::Report), None)
                    .unwrap();
            assert_eq!(kept.tier(), ArtifactTier::Keep);
        }
        assert!(
            crate::artifact_gc(&f.ctx, true)
                .unwrap()
                .actions
                .iter()
                .all(|a| a.action != "transcript")
        );
        crate::artifact_gc(&f.ctx, false).unwrap();
        assert_eq!(live(&f).len(), 1);
    }
}

#[test]
fn generic_purge_cannot_purge_owned_content_but_names_have_no_authority() {
    let mut f = fx(true);
    f.ctx.store.config.artifacts.retention.action = RetentionAction::Purge;
    f.ctx.store.config.artifacts.retention.larger_than = Some("1B".into());
    pi_source(&f, false, b"real raw transcript\n");
    finish(&f, Status::Done, 90);
    run_end(&f.ctx, &f.ctx.store.get(&f.task).unwrap()).unwrap();
    let path = f._tmp.path().join("fake.transcript.log");
    std::fs::write(&path, b"just a generic log\n").unwrap();
    crate::add_artifact(&f.ctx, &f.task, &path, Some(ArtifactKind::Log), None).unwrap();
    crate::artifact_gc(&f.ctx, false).unwrap();
    assert_eq!(live(&f).len(), 1);
    assert!(!owners(&f.ctx, &f.task, &live(&f)[0]).unwrap().is_empty());
}

#[test]
fn an_open_tasks_closed_child_is_promoted_but_never_aged_out() {
    let mut f = fx(true);
    f.ctx.store.config.storage.transcript_retention_days = Some(30);
    let source = pi_source(&f, true, b"closed review of an open task\n");
    let task = f
        .ctx
        .store
        .update(&f.task, |t| {
            t.runs[0].review_runs[0].outcome = Some(corral_core::RunOutcome::Superseded);
            Ok(())
        })
        .unwrap();
    assert_eq!(task.status, Status::Running);
    run_end(&f.ctx, &task).unwrap();
    let a = live(&f).remove(0);
    assert_eq!(owners(&f.ctx, &f.task, &a).unwrap(), vec![owner(&f, true)]);
    assert!(source.exists());
    crate::artifact_gc(&f.ctx, false).unwrap();
    assert_eq!(live(&f).len(), 1);
}

#[test]
fn same_content_retains_each_child_owner() {
    let f = fx(true);
    let source = pi_source(&f, false, b"same bytes\n");
    let a = promote(&f.ctx, &f.task, "R1", owner(&f, false), &source).unwrap();
    let b = promote(&f.ctx, &f.task, "R1", owner(&f, true), &source).unwrap();
    assert_eq!(a.path, b.path);
    assert_eq!(
        owners(&f.ctx, &f.task, &a).unwrap(),
        vec![owner(&f, false), owner(&f, true)]
    );
}
