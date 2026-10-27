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
    // And reads print the original bytes from the blob, without the legacy copy.
    fs::remove_file(f.ctx.store.dir().join(&a.path)).unwrap();
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
    let plain = f.add(
        &done,
        "R1-r1.transcript.log",
        b"raw reviewer transcript",
        ArtifactKind::Log,
    );
    let pinned = f.add(
        &done,
        "agent.transcript.log",
        b"pinned agent transcript",
        ArtifactKind::Log,
    );
    f.ctx
        .store
        .pin(&Fixture::reference(&done, &pinned), None, false)
        .unwrap();
    let shared = b"pi session transcript kept by a report";
    let kept = f.add(&done, "session.transcript.jsonl", shared, ArtifactKind::Log);
    // A keep-tier report elsewhere holds the same blob.
    let reporter = f.task(Status::Ready);
    f.add(&reporter, "session-report.md", shared, ArtifactKind::Report);
    f.finish_days_ago(&done, 40);
    let reopened = f.task(Status::Done);
    let open = f.add(
        &reopened,
        "R2.transcript.log",
        b"reopened transcript",
        ArtifactKind::Log,
    );
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

/// Counters of the keyed artifact operations on one active task, with `grow` extra terminal
/// tasks of `grow` artifacts each.
fn keyed_work(grow: usize) -> (Vec<(Counter, u64)>, u64, Vec<String>) {
    let f = Fixture::new(|_| {});
    let active = f.task(Status::Ready);
    let report = f.add(
        &active,
        "report.html",
        b"<p>report</p>",
        ArtifactKind::Report,
    );
    f.add(&active, "style.css", b"p{}", ArtifactKind::Other);
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
    let gate_log = f.tmp.path().join("gate.log");
    fs::write(&gate_log, b"gate output\n").unwrap();
    let reference = Fixture::reference(&active, &report);
    let lists = work::count(Work::TaskList);
    let before = work::snapshot();
    let scope = work::HotScope::enter();
    let added = add_artifact(&f.ctx, &active, &gate_log, Some(ArtifactKind::Log), None).unwrap();
    let by_ref = resolve_artifact(&f.ctx, &reference).unwrap();
    let by_path = resolve_artifact(&f.ctx, &report.path).unwrap();
    let tiered = set_artifact_tier(
        &f.ctx,
        &Fixture::reference(&active, &added),
        ArtifactTier::Keep,
    )
    .unwrap();
    let opened = artifact_path(&f.ctx, &by_ref).unwrap();
    drop(scope);
    let after = work::snapshot();
    let counters = Counter::ALL
        .into_iter()
        .filter(|c| !matches!(c, Counter::DecodedDocBytes))
        .map(|c| (c, after.total(c) - before.total(c)))
        .collect();
    let observed = vec![
        by_ref.path,
        by_path.path,
        format!("{:?}", tiered.tier()),
        fs::read_to_string(opened).unwrap(),
    ];
    (counters, work::count(Work::TaskList) - lists, observed)
}

#[test]
fn keyed_artifact_operations_do_the_same_work_with_ten_times_the_board() {
    // Given the same active task alone and beside 10 terminal tasks of 10 artifacts each.
    let (base, base_lists, base_seen) = keyed_work(1);
    let (grown, grown_lists, grown_seen) = keyed_work(10);
    // Then attaching, resolving, re-tiering and opening never list the board and do
    // identical work at both sizes, with the same results.
    assert_eq!((base_lists, grown_lists), (0, 0));
    assert_eq!(base, grown);
    assert_eq!(base_seen, grown_seen);
    assert_eq!(base_seen[3], "<p>report</p>");
}
