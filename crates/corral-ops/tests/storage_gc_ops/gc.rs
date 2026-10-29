use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use corral_core::Status;
use corral_core::backend::{ScratchKind, ScratchState};
use corral_ops::storage_gc::{Bucket, Disposition, GcReport, storage_gc};
use serde_json::json;

use crate::fixture::{Fx, Resident, tree_digest};

/// Every class the acceptance names, with the paths the assertions look at.
struct Board {
    fx: Fx,
    done_clean: (String, PathBuf),
    dropped_clean: (String, PathBuf),
    done_dirty: PathBuf,
    kept: PathBuf,
    review: (String, PathBuf),
    unmanaged: PathBuf,
    unknown: PathBuf,
    open_scratch: PathBuf,
    closed_scratch: PathBuf,
    deferred_scratch: PathBuf,
    terminal_session: PathBuf,
    open_session: PathBuf,
    build_output: PathBuf,
    orphan: String,
    referenced: String,
    stray: PathBuf,
    rollback: PathBuf,
    _busy: Resident,
}

fn board() -> Board {
    let fx = Fx::new();
    let done_clean = fx.finished(Status::Done);
    let dropped_clean = fx.finished(Status::Dropped);
    let (_, done_dirty) = fx.finished(Status::Done);
    std::fs::write(done_dirty.join("notes.txt"), "unsaved\n").unwrap();
    let (kept_id, kept) = fx.finished(Status::Done);
    fx.ctx
        .store
        .log_event(
            Some(&kept_id),
            "approved",
            json!({"run": "R1", "worktree_removed": false, "reason": "worktree kept"}),
        )
        .unwrap();
    let review = fx.finished(Status::Review);
    let unmanaged = fx.app.join(".worktrees").join("stray-checkout");
    std::fs::create_dir_all(&unmanaged).unwrap();
    std::fs::write(unmanaged.join("file.txt"), "mine\n").unwrap();
    let unknown_id = fx.task(Status::Running);
    let unknown = fx.app.join(".worktrees").join(&unknown_id);
    std::fs::create_dir_all(&unknown).unwrap();
    std::fs::write(unknown.join("main.rs"), "fn main() {}\n").unwrap();
    fx.start(&unknown_id, "R1", &unknown, Status::Done);

    let store = &fx.ctx.store;
    let scratch = |id: &str| {
        let dir = store
            .register_scratch(
                ScratchKind::Gate,
                id,
                &done_clean.0,
                "R1",
                &format!("gates/{id}"),
            )
            .unwrap();
        std::fs::write(dir.join("log.txt"), vec![1u8; 512]).unwrap();
        dir
    };
    let open_scratch = scratch("gate-open");
    let closed_scratch = scratch("gate-closed");
    store.close_scratch("gate-closed").unwrap();
    let deferred_scratch = scratch("gate-deferred");
    store.close_scratch("gate-deferred").unwrap();
    let later = corral_core::clock::now()
        .checked_add(jiff::SignedDuration::from_secs(3600))
        .unwrap();
    store
        .defer_scratch("gate-deferred", ScratchState::Closed, "busy", later)
        .unwrap();
    let busy = Resident::start(&deferred_scratch, None);

    let sessions = store.dir().join("agent").join("sessions");
    let terminal_session = sessions.join(&done_clean.0);
    std::fs::create_dir_all(terminal_session.join("R1")).unwrap();
    std::fs::write(
        terminal_session.join("R1").join("session.jsonl"),
        "{\"turn\":1}\n",
    )
    .unwrap();
    let open_session = sessions.join(&review.0);
    std::fs::create_dir_all(open_session.join("R1")).unwrap();
    std::fs::write(
        open_session.join("R1").join("session.jsonl"),
        "{\"turn\":1}\n",
    )
    .unwrap();

    let build_output = store.dir().join("state").join("reviews").join("T-9-build");
    std::fs::create_dir_all(&build_output).unwrap();
    std::fs::write(
        build_output.join("CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .unwrap();
    std::fs::write(build_output.join("lib.rlib"), vec![3u8; 2048]).unwrap();

    let mut orphan = String::new();
    store
        .put_evidence("gc-test", "orphan", b"orphaned evidence", |r| {
            orphan = r.to_string();
            json!({"blob": r})
        })
        .unwrap();
    store.delete_record("gc-test", "orphan").unwrap();
    let mut referenced = String::new();
    store
        .put_evidence("gc-test", "kept", b"referenced evidence", |r| {
            referenced = r.to_string();
            json!({"blob": r})
        })
        .unwrap();
    let stray = store.dir().join("blobs/sha256/ab").join("c".repeat(62));
    std::fs::create_dir_all(stray.parent().unwrap()).unwrap();
    std::fs::write(&stray, b"stray bytes").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&stray)
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000))
        .unwrap();
    let rollback = store.dir().join("rollback").join("r-20261029").join("db");
    std::fs::create_dir_all(&rollback).unwrap();
    std::fs::write(rollback.join("corral.db"), vec![9u8; 1024]).unwrap();
    Board {
        done_clean,
        dropped_clean,
        done_dirty,
        kept,
        review,
        unmanaged,
        unknown,
        open_scratch,
        closed_scratch,
        deferred_scratch,
        terminal_session,
        open_session,
        build_output,
        orphan: orphan.trim_start_matches("blob:sha256:").to_owned(),
        referenced: referenced.trim_start_matches("blob:sha256:").to_owned(),
        stray,
        rollback: rollback.parent().unwrap().to_path_buf(),
        _busy: busy,
        fx,
    }
}

fn line<'a>(report: &'a GcReport, target: &Path) -> &'a corral_ops::storage_gc::GcEntry {
    let target = target.display().to_string();
    report
        .entries
        .iter()
        .find(|e| e.target == target)
        .unwrap_or_else(|| panic!("no entry for {target}: {:#?}", report.entries))
}

fn blob_line<'a>(report: &'a GcReport, hash: &str) -> Option<&'a corral_ops::storage_gc::GcEntry> {
    let target = format!("blob:sha256:{hash}");
    report.entries.iter().find(|e| e.target == target)
}

fn seq(ctx: &corral_ops::Ctx) -> u64 {
    ctx.store.view().unwrap().seq().0
}

/// Pins this thread's clock past the 24-hour blob grace, so the orphan is collectable.
fn past_grace() {
    let at = corral_core::clock::now()
        .checked_add(jiff::SignedDuration::from_hours(25))
        .unwrap();
    corral_core::clock::pin_thread(Some(at));
}

#[test]
fn dry_run_reports_every_class_and_changes_nothing() {
    let b = board();
    let root = b.fx.root.clone();
    let before_seq = seq(&b.fx.ctx);
    let Board { fx, .. } = &b;
    let before = tree_digest(&root);
    past_grace();
    let report = {
        let cold = fx.reopen();
        let report = storage_gc(&cold, false).unwrap();
        assert_eq!(seq(&cold), before_seq, "the change feed did not move");
        report
    };
    assert_eq!(tree_digest(&root), before, "no file changed");
    assert!(report.dry_run);

    let check = |path: &Path, disposition: Disposition| {
        let entry = line(&report, path);
        assert_eq!(entry.disposition, disposition, "{entry:?}");
        entry
    };
    check(&b.done_clean.1, Disposition::Reclaimable);
    check(&b.dropped_clean.1, Disposition::Reclaimable);
    assert!(
        check(&b.done_dirty, Disposition::Deferred)
            .reason
            .as_deref()
            .unwrap()
            .starts_with("dirty")
    );
    assert!(
        check(&b.kept, Disposition::Retained)
            .reason
            .as_deref()
            .unwrap()
            .starts_with("kept")
    );
    assert_eq!(
        check(&b.review.1, Disposition::Retained).reason.as_deref(),
        Some("task is in review")
    );
    assert!(
        check(&b.unmanaged, Disposition::Retained)
            .reason
            .as_deref()
            .unwrap()
            .starts_with("unmanaged")
    );
    assert!(
        check(&b.unknown, Disposition::Retained)
            .reason
            .as_deref()
            .unwrap()
            .starts_with("unknown")
    );
    assert_eq!(
        check(&b.open_scratch, Disposition::Retained)
            .reason
            .as_deref(),
        Some("attempt open")
    );
    check(&b.closed_scratch, Disposition::Reclaimable);
    assert!(
        check(&b.deferred_scratch, Disposition::Deferred)
            .reason
            .as_deref()
            .unwrap()
            .contains("process")
    );
    let session = check(&b.terminal_session, Disposition::Reclaimable);
    assert_eq!(session.bucket, Bucket::CompressedIntoBlobs);
    assert!(
        check(&b.open_session, Disposition::Retained)
            .reason
            .as_deref()
            .unwrap()
            .starts_with("non-terminal")
    );
    let build = check(&b.build_output, Disposition::Reclaimable);
    assert!(build.bytes >= 2048);
    assert_eq!(
        blob_line(&report, &b.orphan).unwrap().disposition,
        Disposition::Reclaimable
    );
    assert!(
        blob_line(&report, &b.referenced).is_none(),
        "referenced blobs are not candidates"
    );
    check(&b.stray, Disposition::Reclaimable);
    let archive = check(&b.rollback, Disposition::Reported);
    assert_eq!(archive.bucket, Bucket::RollbackArchive);
    for root in ["db", "blobs", "agent/sessions", "lock", "rollback"] {
        let entry = check(&fx.ctx.store.dir().join(root), Disposition::Reported);
        assert_eq!(entry.bucket, Bucket::FixedRoot);
    }
    let legacy = check(&fx.ctx.store.dir().join("state"), Disposition::Reported);
    assert_eq!(legacy.bucket, Bucket::LegacyAwaitingFinalize);
    check(&root.join(".scratch"), Disposition::Reported);
    for bucket in [
        Bucket::Reclaimed,
        Bucket::CompressedIntoBlobs,
        Bucket::MovedOutsideCorral,
        Bucket::LegacyAwaitingFinalize,
        Bucket::RollbackArchive,
        Bucket::FixedRoot,
        Bucket::Retained,
    ] {
        assert!(
            report.totals.get(&bucket).copied().unwrap_or(0) > 0,
            "{bucket:?}: {:?}",
            report.totals
        );
    }
}

#[test]
fn yes_removes_only_reclaimable_entries_with_one_event_each() {
    let b = board();
    let fx = &b.fx;
    past_grace();
    let report = storage_gc(&fx.ctx, true).unwrap();
    assert!(!report.dry_run);
    let gone = [
        &b.done_clean.1,
        &b.dropped_clean.1,
        &b.closed_scratch,
        &b.terminal_session,
        &b.build_output,
        &b.stray,
    ];
    for path in gone {
        assert!(!path.exists(), "{} should be removed", path.display());
        assert_eq!(line(&report, path).disposition, Disposition::Reclaimed);
    }
    let stays = [
        &b.done_dirty,
        &b.kept,
        &b.review.1,
        &b.unmanaged,
        &b.unknown,
        &b.open_scratch,
        &b.deferred_scratch,
        &b.open_session,
        &b.rollback,
    ];
    for path in stays {
        assert!(path.exists(), "{} should stay", path.display());
    }
    for task in [&b.done_clean.0, &b.dropped_clean.0] {
        assert!(
            fx.branch_exists(&format!("corral/{task}-R1")),
            "branch kept"
        );
    }
    let rows = fx.ctx.store.blob_rows().unwrap();
    assert!(
        rows.iter().all(|row| row.hash.as_str() != b.orphan),
        "orphan collected"
    );
    assert!(
        rows.iter().any(|row| row.hash.as_str() == b.referenced),
        "referenced kept"
    );
    let promoted = fx.ctx.store.artifact_rows(&b.done_clean.0, None).unwrap();
    let transcript = b.terminal_session.join("R1").join("session.jsonl");
    assert!(
        promoted
            .iter()
            .any(|a| a.source == transcript.display().to_string()),
        "the session transcript was promoted before removal: {promoted:?}"
    );
    let removed = report
        .entries
        .iter()
        .filter(|e| e.disposition == Disposition::Reclaimed)
        .count();
    assert_eq!(
        fx.events("storage_reclaimed").len(),
        removed,
        "one audited event per deletion"
    );
    assert_eq!(fx.events("storage_gc").len(), 1);
}
