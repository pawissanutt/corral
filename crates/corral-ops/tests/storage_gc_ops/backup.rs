use std::sync::{Arc, Barrier};

use corral_core::Store;
use serde_json::json;

use corral_ops::storage_gc::{backup_storage, check_storage};

use crate::fixture::Fx;

#[test]
fn backup_while_writers_commit_copies_every_referenced_blob_verified() {
    let fx = Fx::new();
    for n in 0..8 {
        let bytes = format!("evidence before the backup {n}");
        fx.ctx
            .store
            .put_evidence(
                "backup-test",
                &format!("before-{n}"),
                bytes.as_bytes(),
                |r| json!({"blob": r}),
            )
            .unwrap();
    }
    let start = Arc::new(Barrier::new(3));
    let writer = {
        let store = fx.ctx.store.reopen().unwrap();
        let start = Arc::clone(&start);
        std::thread::spawn(move || {
            start.wait();
            for n in 0..40 {
                let bytes = format!("evidence during the backup {n}");
                store
                    .put_evidence(
                        "backup-test",
                        &format!("during-{n}"),
                        bytes.as_bytes(),
                        |r| json!({"blob": r}),
                    )
                    .unwrap();
                // Unreferencing earlier evidence makes it a GC candidate while the copy runs.
                if n % 2 == 0 {
                    store
                        .delete_record("backup-test", &format!("before-{}", n / 10))
                        .ok();
                }
            }
        })
    };
    let collector = {
        let store = fx.ctx.store.reopen().unwrap();
        let start = Arc::clone(&start);
        std::thread::spawn(move || {
            start.wait();
            for _ in 0..20 {
                store.blob_gc(&corral_core::backend::SystemClock).unwrap();
            }
        })
    };
    let dir = fx.root.join("backup");
    start.wait();
    let report = backup_storage(&fx.ctx.store, &dir).unwrap();
    writer.join().unwrap();
    collector.join().unwrap();
    assert!(report.blobs >= 8, "{report:?}");

    let copy = Store::open(&dir).unwrap();
    let check = check_storage(&copy).unwrap();
    assert!(check.is_ok(), "{check:?}");
    let referenced: Vec<_> = copy
        .blob_rows()
        .unwrap()
        .into_iter()
        .filter(|row| row.refs > 0)
        .collect();
    assert!(!referenced.is_empty());
    let blobs = copy.blob_store().unwrap();
    for row in referenced {
        assert!(blobs.path_of(&row.hash).is_file(), "{} copied", row.hash);
    }
    assert_eq!(
        check.blobs_checked as usize,
        copy.blob_rows()
            .unwrap()
            .iter()
            .filter(|r| r.state == corral_core::backend::BlobState::Live)
            .count()
    );
}

#[test]
fn backup_refuses_an_existing_target() {
    let fx = Fx::new();
    let dir = fx.root.join("backup");
    std::fs::create_dir_all(dir.join(".corral")).unwrap();
    let error = backup_storage(&fx.ctx.store, &dir).unwrap_err();
    assert!(format!("{error:#}").contains("already exists"), "{error:#}");
}
