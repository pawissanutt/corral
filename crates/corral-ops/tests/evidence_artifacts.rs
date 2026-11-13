//! Blob-backed artifacts (plan T-253 sections 7-10, child evidence-artifacts): content stored
//! once as a blob referenced per task, purges that leave stubs and free bytes only through
//! blob GC after the horizon, transcript retention, decoded copies under the cache root and
//! keyed artifact lookups whose work does not grow with the rest of the board.
use std::fs;
use std::time::Duration;

use corral_core::backend::{BlobState, ManualClock};
use corral_core::blob::GcRun;
use corral_core::blob_ref::BlobHash;
use corral_core::work::{self, Counter, Work};
use corral_core::{Artifact, ArtifactKind, ArtifactTier, Event, RetentionAction, Status, Store};
use corral_ops::{
    Ctx, add_artifact, artifact_gc, artifact_path, artifact_read, resolve_artifact,
    set_artifact_tier,
};
use serde_json::json;

struct Fixture {
    tmp: tempfile::TempDir,
    ctx: Ctx,
}

impl Fixture {
    fn new(configure: impl FnOnce(&mut Store)) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let mut store = Store::init(tmp.path()).unwrap();
        configure(&mut store);
        Self {
            tmp,
            ctx: corral_ops::resources::FixedResourceProbe::ctx(store),
        }
    }

    fn task(&self, status: Status) -> String {
        self.ctx
            .store
            .create("Evidence fixture", None, status, "")
            .unwrap()
            .id
    }

    fn add(&self, task: &str, name: &str, bytes: &[u8], kind: ArtifactKind) -> Artifact {
        let path = self.tmp.path().join("sources").join(task).join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        add_artifact(&self.ctx, task, &path, Some(kind), None).unwrap()
    }

    fn session(&self, task: &str, name: &str, bytes: &[u8]) -> Artifact {
        self.ctx
            .store
            .update(task, |t| {
                if t.runs.is_empty() {
                    t.runs.push(
                        serde_json::from_value(json!({
                            "id":"R1", "agent":"corral-pi", "harness":"corral-pi",
                            "worktree":"/fixture", "branch":"test", "base":"main", "base_sha":"abc",
                            "started":"2026-09-01T00:00:00Z"
                        }))
                        .unwrap(),
                    );
                }
                Ok(())
            })
            .unwrap();
        let dir = self
            .ctx
            .store
            .dir()
            .join("agent/sessions")
            .join(task)
            .join("R1");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(name), bytes).unwrap();
        // Use the lifecycle's closed-container view; the caller may then model reopening.
        let mut closed = self.ctx.store.get(task).unwrap();
        closed.status = Status::Done;
        corral_ops::agent::hooks::cleanup::run_end(&self.ctx, &closed).unwrap();
        let sha = corral_core::agent::paths::sha256_hex(bytes);
        self.ctx
            .store
            .get(task)
            .unwrap()
            .artifacts
            .into_iter()
            .find(|a| a.sha256 == sha)
            .unwrap()
    }

    fn current(&self, task: &str, a: &Artifact) -> Artifact {
        let task = self.ctx.store.get(task).unwrap();
        task.artifacts
            .into_iter()
            .find(|o| o.path == a.path)
            .unwrap()
    }

    fn blob(&self, a: &Artifact) -> corral_core::backend::BlobRow {
        let hash = BlobHash::parse(&a.sha256).unwrap();
        self.ctx.store.blob_row(&hash).unwrap().expect("blob row")
    }

    fn blob_file(&self, a: &Artifact) -> std::path::PathBuf {
        let hash = BlobHash::parse(&a.sha256).unwrap();
        self.ctx.store.blob_store().unwrap().path_of(&hash)
    }

    fn finish_days_ago(&self, task: &str, days: i64) {
        let at = jiff::Timestamp::now() - jiff::SignedDuration::from_hours(24 * days);
        let event = Event {
            seq: 0,
            task: Some(task.into()),
            ts: at,
            kind: "moved".into(),
            actor: None,
            via: None,
            detail: json!({"to": "done"}),
        };
        self.ctx
            .store
            .fixture_append_events(&format!("{}\n", serde_json::to_string(&event).unwrap()))
            .unwrap();
    }

    fn reference(task: &str, a: &Artifact) -> String {
        corral_core::PinRef::of_artifact(task, a).to_string()
    }
}

#[test]
fn identical_bytes_on_two_tasks_are_one_blob_with_two_rows_and_events() {
    // Given two tasks.
    let f = Fixture::new(|_| {});
    let (first, second) = (f.task(Status::Ready), f.task(Status::Ready));
    // When identical bytes are attached to both.
    let bytes = b"identical evidence bytes\n";
    let a = f.add(&first, "one.txt", bytes, ArtifactKind::Evidence);
    let b = f.add(&second, "two.txt", bytes, ArtifactKind::Evidence);
    // Then one live blob holds the content, referenced by both tasks.
    assert_eq!(a.sha256, b.sha256);
    let row = f.blob(&a);
    assert_eq!((row.state, row.refs), (BlobState::Live, 2));
    let stored = f.ctx.store.blob_store().unwrap();
    let files = corral_core::backend::BlobStore::list(&stored, None, 16).unwrap();
    assert_eq!(files.len(), 1);
    // Two artifact rows and two artifact events.
    for (task, artifact) in [(&first, &a), (&second, &b)] {
        let rows = f.ctx.store.get(task).unwrap().artifacts;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, artifact.path);
    }
    let events = f.ctx.store.events_of(None, &["artifact"]).unwrap();
    assert_eq!(events.len(), 2);
    // New publication never creates a legacy copy; reads use the canonical blob.
    assert!(!f.ctx.store.dir().join(&a.path).exists());
    assert_eq!(artifact_read(&f.ctx, &a).unwrap(), bytes);
    assert_eq!(artifact_read(&f.ctx, &b).unwrap(), bytes);
}

#[test]
fn gc_purges_rows_to_stubs_and_blob_gc_frees_bytes_only_after_the_horizon() {
    // Given keep, normal, transient and pinned artifacts on a finished task, under a purge rule.
    let f = Fixture::new(|store| {
        store.config.artifacts.retention.action = RetentionAction::Purge;
        store.config.artifacts.retention.larger_than = Some("1B".into());
    });
    let task = f.task(Status::Done);
    let keep = f.add(&task, "report.md", b"keep report", ArtifactKind::Report);
    let normal = f.add(&task, "notes.txt", b"normal notes", ArtifactKind::Evidence);
    let transient = f.add(&task, "run.log", b"transient log", ArtifactKind::Log);
    let pinned = f.add(&task, "pinned.log", b"pinned log", ArtifactKind::Log);
    f.ctx
        .store
        .pin(&Fixture::reference(&task, &pinned), None, false)
        .unwrap();
    assert_eq!(normal.tier(), ArtifactTier::Normal);
    assert_eq!(transient.tier(), ArtifactTier::Transient);
    // When a dry run previews retention.
    let preview = artifact_gc(&f.ctx, true).unwrap();
    let mut planned: Vec<_> = preview.actions.iter().map(|a| a.path.clone()).collect();
    planned.sort();
    let mut expected = vec![normal.path.clone(), transient.path.clone()];
    expected.sort();
    // Then exactly the unprotected normal and transient rows are selected, and nothing changes.
    assert_eq!(planned, expected);
    assert!(preview.actions.iter().all(|a| a.action == "purge"));
    assert_eq!(f.current(&task, &normal).purged_at, None);
    assert_eq!(f.blob(&normal).refs, 1);
    // When retention applies.
    let applied = artifact_gc(&f.ctx, false).unwrap();
    // Then those rows are stubs, the others are untouched, and no bytes were freed yet.
    assert_eq!(applied.actions.len(), 2);
    assert_eq!(applied.blobs_deleted, 0);
    for a in [&normal, &transient] {
        let stub = f.current(&task, a);
        assert!(stub.purged_at.is_some());
        let text = String::from_utf8(artifact_read(&f.ctx, &stub).unwrap()).unwrap();
        assert!(text.starts_with("Artifact content purged"), "{text}");
        let row = f.blob(a);
        assert_eq!((row.state, row.refs), (BlobState::Live, 0));
        assert!(row.unreferenced_since.is_some());
        assert!(f.blob_file(a).is_file());
    }
    for a in [&keep, &pinned] {
        assert_eq!(f.current(&task, a).purged_at, None);
        assert_eq!(f.blob(a).refs, 1);
    }
    // When blob GC runs one second before and one second after the grace horizon.
    let grace = f.ctx.store.config.storage.blob_grace();
    let since = f.blob(&normal).unreferenced_since.unwrap();
    let latest = f.blob(&transient).unreferenced_since.unwrap().max(since);
    let early = ManualClock::new(since + grace - Duration::from_secs(1));
    let GcRun::Done(kept) = f.ctx.store.blob_gc(&early).unwrap() else {
        panic!("claim held");
    };
    // Then the bytes survive until the horizon passes, then go; kept blobs stay.
    assert!(kept.deleted.is_empty());
    assert!(f.blob_file(&normal).is_file());
    let late = ManualClock::new(latest + grace + Duration::from_secs(1));
    let GcRun::Done(freed) = f.ctx.store.blob_gc(&late).unwrap() else {
        panic!("claim held");
    };
    assert_eq!(freed.deleted.len(), 2);
    for a in [&normal, &transient] {
        assert!(!f.blob_file(a).exists());
        assert!(
            f.ctx
                .store
                .blob_row(&BlobHash::parse(&a.sha256).unwrap())
                .unwrap()
                .is_none()
        );
    }
    assert_eq!(artifact_read(&f.ctx, &keep).unwrap(), b"keep report");
    assert_eq!(artifact_read(&f.ctx, &pinned).unwrap(), b"pinned log");
}

fn transcripts(f: &Fixture) -> (String, String, [Artifact; 3], Artifact) {
    let done = f.task(Status::Done);
    let plain = f.session(&done, "ordinary.jsonl", b"raw Pi transcript");
    let pinned = f.session(&done, "pinned.jsonl", b"pinned agent transcript");
    f.ctx
        .store
        .pin(&Fixture::reference(&done, &pinned), None, false)
        .unwrap();
    let shared = b"pi session transcript kept by a report";
    let kept = f.session(&done, "shared.jsonl", shared);
    // A keep-tier report elsewhere holds the same blob.
    let reporter = f.task(Status::Ready);
    f.add(&reporter, "session-report.md", shared, ArtifactKind::Report);
    f.finish_days_ago(&done, 40);
    let reopened = f.task(Status::Done);
    let open = f.session(&reopened, "reopened.jsonl", b"reopened transcript");
    f.finish_days_ago(&reopened, 40);
    f.ctx
        .store
        .update(&reopened, |t| {
            t.status = Status::Ready;
            Ok(())
        })
        .unwrap();
    (done, reopened, [plain, pinned, kept], open)
}

#[test]
fn transcripts_are_kept_forever_without_a_retention_setting() {
    // Given transcripts of a task finished 40 days ago and no transcript retention.
    let f = Fixture::new(|store| {
        store.config.artifacts.retention.action = RetentionAction::Keep;
        assert_eq!(store.config.storage.transcript_retention_days, None);
    });
    let (done, reopened, [plain, pinned, kept], open) = transcripts(&f);
    // When storage GC applies.
    let report = artifact_gc(&f.ctx, false).unwrap();
    // Then every transcript stays (only legacy-copy dedup runs).
    assert!(
        report.actions.iter().all(|a| a.action == "dedup"),
        "{:?}",
        report.actions
    );
    for (task, a) in [
        (&done, &plain),
        (&done, &pinned),
        (&done, &kept),
        (&reopened, &open),
    ] {
        assert_eq!(f.current(task, a).purged_at, None);
    }
}

#[test]
fn thirty_day_retention_purges_only_unprotected_transcripts_of_finished_tasks() {
    // Given transcript_retention_days = 30 and transcripts finished 40 days ago.
    let f = Fixture::new(|store| {
        store.config.artifacts.retention.action = RetentionAction::Keep;
        store.config.storage.transcript_retention_days = Some(30);
    });
    let (done, reopened, [plain, pinned, kept], open) = transcripts(&f);
    // When storage GC applies.
    let report = artifact_gc(&f.ctx, false).unwrap();
    // Then only the plain transcript of the finished task becomes a stub.
    let purged: Vec<_> = report
        .actions
        .iter()
        .filter(|a| a.action != "dedup")
        .map(|a| (a.action.as_str(), a.path.as_str()))
        .collect();
    assert_eq!(purged, [("transcript", plain.path.as_str())]);
    let stub = f.current(&done, &plain);
    assert!(stub.purged_at.is_some());
    assert_eq!(f.blob(&plain).refs, 0);
    // Pinned, keep-referenced and reopened-task transcripts stay.
    for (task, a) in [(&done, &pinned), (&done, &kept), (&reopened, &open)] {
        assert_eq!(f.current(task, a).purged_at, None, "{}", a.path);
    }
}

#[test]
fn opening_an_artifact_reads_a_decoded_copy_under_the_cache_root() {
    // Given an attached report.
    let f = Fixture::new(|_| {});
    let task = f.task(Status::Ready);
    let a = f.add(&task, "report.md", b"# Report\n", ArtifactKind::Report);
    // When it is resolved by reference and opened.
    let resolved = resolve_artifact(&f.ctx, &Fixture::reference(&task, &a)).unwrap();
    let path = artifact_path(&f.ctx, &resolved).unwrap();
    // Then the opener gets a decoded copy under <cache_root>/decoded, never under .corral.
    let decoded = f.ctx.store.cache_root().unwrap().join("decoded");
    assert!(path.starts_with(&decoded), "{}", path.display());
    assert!(!path.starts_with(f.ctx.store.dir()));
    assert_eq!(fs::read(&path).unwrap(), b"# Report\n");
    // And the decoded path resolves back to the artifact.
    let back = resolve_artifact(&f.ctx, &path.display().to_string()).unwrap();
    assert_eq!(back.path, a.path);
}

fn counters(before: &work::WorkSnapshot, after: &work::WorkSnapshot) -> Counted {
    Counter::ALL
        .into_iter()
        .map(|c| (c, after.total(c) - before.total(c)))
        .collect()
}

/// The keyed artifact reads and writes of one active task, beside `grow` terminal tasks of
/// `grow` artifacts each, with `grow` more artifacts of the active task in another run.
/// Returns the read counters (every counter, bytes included), the write counters, board
/// listings and what the operations observed.
type Counted = Vec<(Counter, u64)>;

fn keyed_work(grow: usize) -> (Counted, Counted, u64, Vec<String>) {
    let f = Fixture::new(|store| {
        store
            .fixture_pin_now("2026-01-01T00:00:00Z".parse().unwrap())
            .unwrap();
    });
    let active = f.task(Status::Ready);
    let report = f.add(
        &active,
        "report.html",
        b"<p>report</p>",
        ArtifactKind::Report,
    );
    f.add(&active, "style.css", b"p{}", ArtifactKind::Other);
    let fixed: jiff::Timestamp = "2026-01-01T00:00:00Z".parse().unwrap();
    f.ctx
        .store
        .update(&active, |t| {
            let mut history = Vec::new();
            for i in 0..grow {
                let mut other = t.artifacts[0].clone();
                other.run = "R7".into();
                other.path = format!("artifacts/{}/R7/{i:08x}-old.html", t.id);
                other.title = format!("old {i}");
                history.push(other);
            }
            t.artifacts.extend(history);
            for a in &mut t.artifacts {
                a.added = fixed;
            }
            Ok(())
        })
        .unwrap();
    for t in 0..grow {
        let task = f.task(Status::Done);
        for i in 0..grow {
            let bytes = format!("terminal {t} artifact {i}");
            f.add(
                &task,
                &format!("a{i}.txt"),
                bytes.as_bytes(),
                ArtifactKind::Evidence,
            );
        }
    }
    let report = f.current(&active, &report);
    let reference = Fixture::reference(&active, &report);
    f.ctx.store.pin(&reference, None, false).unwrap();
    let gate_log = f.tmp.path().join("gate.log");
    fs::write(&gate_log, b"gate output\n").unwrap();
    let lists = work::count(Work::TaskList);
    let before = work::snapshot();
    let scope = work::HotScope::enter();
    let by_ref = resolve_artifact(&f.ctx, &reference).unwrap();
    let by_path = resolve_artifact(&f.ctx, &report.path).unwrap();
    let by_pin = resolve_artifact(&f.ctx, "1").unwrap();
    let opened = artifact_path(&f.ctx, &by_ref).unwrap();
    let read = work::snapshot();
    let added = add_artifact(&f.ctx, &active, &gate_log, Some(ArtifactKind::Log), None).unwrap();
    let added_ref = Fixture::reference(&active, &added);
    let invoker = corral_ops::command::Invoker::process();
    let tiered = set_artifact_tier(&f.ctx, &added_ref, ArtifactTier::Keep, &invoker).unwrap();
    drop(scope);
    let wrote = work::snapshot();
    let writes = counters(&read, &wrote)
        .into_iter()
        .filter(|(c, _)| !matches!(c, Counter::DecodedDocBytes))
        .collect();
    let observed = vec![
        by_ref.path,
        by_path.path,
        by_pin.path,
        format!("{:?}", tiered.tier()),
        fs::read_to_string(opened).unwrap(),
    ];
    let lists = work::count(Work::TaskList) - lists;
    (counters(&before, &read), writes, lists, observed)
}

#[test]
fn keyed_artifact_operations_do_the_same_work_with_ten_times_the_history() {
    // Given the same active task alone, then beside 10 terminal tasks of 10 artifacts each and
    // with 10 more of its own artifacts in another run.
    let (base_reads, base_writes, base_lists, base_seen) = keyed_work(1);
    let (grown_reads, grown_writes, grown_lists, grown_seen) = keyed_work(10);
    // Then resolving by reference, path and pin and opening read only that run's rows, never
    // hydrate the task, and do byte-identical work; nothing lists the board.
    assert_eq!((base_lists, grown_lists), (0, 0));
    let hydrations = |c: &[(Counter, u64)]| {
        c.iter()
            .find(|(k, _)| *k == Counter::FullTaskHydrations)
            .map(|(_, n)| *n)
    };
    assert_eq!(hydrations(&base_reads), Some(0));
    assert_eq!(base_reads, grown_reads);
    // And attaching is a head write (T-333) while re-tiering is still one full-task write: they
    // never list the board, and only the acted task's own history rows they read grow
    // (head-form re-tiering is a follow-up outside this task's write scope).
    let without_rows = |c: &[(Counter, u64)]| {
        c.iter()
            .filter(|(k, _)| *k != Counter::HistoryRowDecodes)
            .copied()
            .collect::<Vec<_>>()
    };
    assert_eq!(hydrations(&base_writes), Some(1));
    assert_eq!(without_rows(&base_writes), without_rows(&grown_writes));
    assert_eq!(base_seen, grown_seen);
    assert_eq!(base_seen[4], "<p>report</p>");
}

#[test]
fn an_index_failure_never_drops_a_committed_artifacts_blob_reference() {
    // Given a search index path that cannot be written.
    let f = Fixture::new(|_| {});
    let task = f.task(Status::Ready);
    let cache = f.ctx.store.cache_root().unwrap();
    fs::create_dir_all(cache.join("content-search.json")).unwrap();
    // When evidence is attached.
    let a = f.add(
        &task,
        "evidence.txt",
        b"durable evidence",
        ArtifactKind::Evidence,
    );
    // Then the artifact is committed and its blob stays referenced.
    assert_eq!(f.current(&task, &a).purged_at, None);
    assert_eq!(f.blob(&a).refs, 1);
    let key = format!("{task}/{}", a.sha256);
    assert!(
        f.ctx
            .store
            .record_row("artifact-blobs", &key)
            .unwrap()
            .is_some()
    );
}

#[test]
fn attaching_to_a_missing_task_stores_no_blob_reference() {
    // Given a file and no task T-999.
    let f = Fixture::new(|_| {});
    let path = f.tmp.path().join("orphan.txt");
    fs::write(&path, b"orphan").unwrap();
    // When attaching it to the missing task, it fails before any blob reference is written.
    assert!(add_artifact(&f.ctx, "T-999", &path, None, None).is_err());
    let hash = BlobHash::of(b"orphan");
    let key = format!("T-999/{hash}");
    assert!(
        f.ctx
            .store
            .record_row("artifact-blobs", &key)
            .unwrap()
            .is_none()
    );
    assert!(
        f.ctx
            .store
            .blob_row(&hash)
            .unwrap()
            .is_none_or(|row| row.refs == 0)
    );
}

#[test]
fn generic_purge_never_purges_transcripts_and_transcript_retention_decides_alone() {
    // Given a size-triggered purge rule on every tier and transcripts on open, recently
    // finished and long-dropped tasks.
    let configure = |retention: Option<u32>| {
        move |store: &mut Store| {
            store.config.artifacts.retention.action = RetentionAction::Purge;
            store.config.artifacts.retention.larger_than = Some("1B".into());
            store.config.storage.transcript_retention_days = retention;
        }
    };
    for retention in [None, Some(30)] {
        let f = Fixture::new(configure(retention));
        let open = f.task(Status::Ready);
        let recent = f.task(Status::Done);
        let dropped = f.task(Status::Dropped);
        let transcripts: Vec<_> = [&open, &recent, &dropped]
            .into_iter()
            .map(|task| {
                let bytes = format!("{task} transcript");
                (task, f.session(task, "ordinary.jsonl", bytes.as_bytes()))
            })
            .collect();
        let plain = f.add(&recent, "run.log", b"plain log", ArtifactKind::Log);
        f.finish_days_ago(&recent, 10);
        let at = jiff::Timestamp::now() - jiff::SignedDuration::from_hours(24 * 40);
        let event = Event {
            seq: 0,
            task: Some(dropped.clone()),
            ts: at,
            kind: "moved".into(),
            actor: None,
            via: None,
            detail: json!({"to": "dropped"}),
        };
        f.ctx
            .store
            .fixture_append_events(&format!("{}\n", serde_json::to_string(&event).unwrap()))
            .unwrap();
        // When retention applies.
        artifact_gc(&f.ctx, false).unwrap();
        // Then the generic rule purged the plain log but no transcript, and only a set
        // retention purged the transcript of the task dropped more than 30 days ago.
        assert!(f.current(&recent, &plain).purged_at.is_some());
        for (task, a) in &transcripts {
            let purged = f.current(task, a).purged_at.is_some();
            assert_eq!(
                purged,
                retention.is_some() && *task == &dropped,
                "{task} {retention:?}"
            );
        }
    }
}

#[test]
fn blob_only_artifacts_serve_gc_stats_and_search() {
    // Given an artifact whose legacy copy is gone.
    let f = Fixture::new(|_| {});
    let task = f.task(Status::Ready);
    let a = f.add(
        &task,
        "notes.md",
        b"blobonlyprobe text",
        ArtifactKind::Evidence,
    );
    assert!(!f.ctx.store.dir().join(&a.path).exists());
    // When retention, storage totals and search run.
    artifact_gc(&f.ctx, true).unwrap();
    artifact_gc(&f.ctx, false).unwrap();
    let stats = corral_ops::storage_stats(&f.ctx).unwrap();
    corral_ops::refresh_search_index(&f.ctx).unwrap();
    let hits = corral_ops::search_content(
        &f.ctx,
        "blobonlyprobe",
        &corral_ops::ContentSearch::default(),
    )
    .unwrap();
    // Then each works from the blob without recreating the legacy copy.
    assert_eq!(stats.stored_bytes, f.blob(&a).stored_bytes);
    assert_eq!(hits.len(), 1);
    assert!(!f.ctx.store.dir().join(&a.path).exists());
}

/// Search hits for `token`, and the counters of one index refresh after a body change of the
/// active task, beside `grow` terminal tasks with `grow` artifacts each.
fn feed_refresh(grow: usize) -> (Counted, u64, usize, usize) {
    let f = Fixture::new(|_| {});
    let active = f.task(Status::Ready);
    for t in 0..grow {
        let task = f.task(Status::Done);
        for i in 0..grow {
            let bytes = format!("terminal {t} artifact {i}");
            f.add(
                &task,
                &format!("a{i}.txt"),
                bytes.as_bytes(),
                ArtifactKind::Evidence,
            );
        }
    }
    corral_ops::refresh_search_index(&f.ctx).unwrap();
    f.ctx
        .store
        .update(&active, |t| {
            t.body = "feedprobe body".into();
            Ok(())
        })
        .unwrap();
    let lists = work::count(Work::TaskList);
    let before = work::snapshot();
    let scope = work::HotScope::enter();
    corral_ops::refresh_search_index_with_tasks(&f.ctx, &[]).unwrap();
    drop(scope);
    let after = work::snapshot();
    let lists = work::count(Work::TaskList) - lists;
    let search = |token: &str| {
        corral_ops::search_content(&f.ctx, token, &corral_ops::ContentSearch::default())
            .unwrap()
            .len()
    };
    let warm = search("feedprobe");
    // A deleted cache root rebuilds to the same results.
    fs::remove_dir_all(f.ctx.store.cache_root().unwrap()).unwrap();
    corral_ops::refresh_search_index(&f.ctx).unwrap();
    let cold = search("feedprobe");
    let counted = counters(&before, &after)
        .into_iter()
        .filter(|(c, _)| !matches!(c, Counter::DecodedDocBytes))
        .collect();
    (counted, lists, warm, cold)
}

#[test]
fn the_search_index_follows_the_change_feed_with_work_independent_of_the_board() {
    // Given an indexed board, then one task body change.
    let (base, base_lists, base_warm, base_cold) = feed_refresh(1);
    let (grown, grown_lists, grown_warm, grown_cold) = feed_refresh(10);
    // Then the refresh re-derives only that task (no board listing, identical counted work at
    // both sizes) and the change is searchable, warm and after a cold rebuild.
    assert_eq!((base_lists, grown_lists), (0, 0));
    assert_eq!(base, grown);
    assert_eq!((base_warm, base_cold, grown_warm, grown_cold), (1, 1, 1, 1));
}
