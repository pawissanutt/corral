use crate::annotations::*;
use crate::backend::{BackendConfig, FaultInjector, ManualClock, fake::FakeBackend};
use crate::{Status, Store};
use std::sync::Arc;

fn thread(task: &str) -> AnnotationThread {
    AnnotationThread {
        version: 1,
        id: "N1".into(),
        revision: 1,
        state: AnnotationState::Open,
        anchor: AnnotationAnchor {
            version: 1,
            task: task.into(),
            run: "R1".into(),
            base: "a".repeat(40),
            head: "b".repeat(40),
            tree: "c".repeat(40),
            path: "src/lib.rs".into(),
            side: AnnotationSide::New,
            blob: Some("d".repeat(40)),
            range: None,
            excerpt: "code".into(),
            excerpt_truncated: false,
            excerpt_sha256: "e".repeat(64),
        },
        entries: vec![AnnotationEntry {
            version: 1,
            seq: 1,
            kind: AnnotationKind::Opened,
            state: AnnotationState::Open,
            author: AnnotationAuthor::Owner {
                version: 1,
                surface: AnnotationSurface::Cli,
            },
            at: jiff::Timestamp::UNIX_EPOCH,
            text: "check".into(),
            request: AnnotationRequest {
                key: "open".into(),
                payload_sha256: "f".repeat(64),
            },
        }],
    }
}
#[test]
fn annotations_codec_and_backends_preserve_rows_and_projection() {
    for fake in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::init(dir.path()).unwrap();
        let faults = Arc::new(FaultInjector::default());
        if fake {
            store = store.with_backend(Box::new(FakeBackend::new(
                BackendConfig::default(),
                Arc::new(ManualClock::new(jiff::Timestamp::UNIX_EPOCH)),
                faults.clone(),
            )));
        }
        let task = store
            .create("Annotations", None, Status::Ready, "body")
            .unwrap();
        let t = thread(&task.id);
        store
            .update(&task.id, |task| {
                task.annotations.push(t.clone());
                Ok(())
            })
            .unwrap();
        assert_eq!(store.head(&task.id).unwrap().annotation_summary.entries, 1);
        assert!(
            store
                .head(&task.id)
                .unwrap()
                .to_shell_task()
                .unwrap()
                .annotations
                .is_empty()
        );
        let before = store.get(&task.id).unwrap();
        let head = store.head(&task.id).unwrap();
        assert_eq!(
            head.annotation_request("open").unwrap().unwrap().1,
            t.entries[0]
        );
        if fake {
            let seq = store.view().unwrap().seq();
            let tail = store.event_tail().unwrap();
            // Fail after head/row work has begun, without a process-crash matrix.
            faults.arm(3);
            assert!(
                store
                    .update_head(&task.id, |head| {
                        let mut entry = t.entries[0].clone();
                        entry.seq = 2;
                        entry.kind = AnnotationKind::Reply;
                        entry.request.key = "failed-reply".into();
                        head.append_annotation(t.clone(), entry, false)?;
                        store.log_event(
                            Some(&task.id),
                            "annotation_replied",
                            serde_json::json!({"thread":"N1","entry":2}),
                        )?;
                        Ok(())
                    })
                    .is_err()
            );
            faults.disarm();
            assert_eq!(store.get(&task.id).unwrap(), before);
            assert_eq!(store.head(&task.id).unwrap().annotation_summary.entries, 1);
            assert_eq!(store.view().unwrap().seq(), seq);
            assert_eq!(store.event_tail().unwrap(), tail);
            assert!(
                store
                    .head(&task.id)
                    .unwrap()
                    .annotation_request("failed-reply")
                    .unwrap()
                    .is_none()
            );
        }
        store
            .update(&task.id, |task| {
                task.title = "unrelated".into();
                Ok(())
            })
            .unwrap();
        assert_eq!(store.get(&task.id).unwrap().annotations, vec![t.clone()]);
        let mut bad = t.clone();
        bad.entries[0].version = 2;
        assert!(
            store
                .update(&task.id, |task| {
                    task.annotations = vec![bad];
                    Ok(())
                })
                .is_err()
        );
        assert_eq!(store.get(&task.id).unwrap().annotations, vec![t]);
        if !fake {
            assert_eq!(
                Store::open(dir.path())
                    .unwrap()
                    .get(&task.id)
                    .unwrap()
                    .annotations
                    .len(),
                1
            );
        }
    }
}
#[test]
fn annotations_bounds_are_exact_and_refused_not_truncated() {
    assert!(validate_text(&"x".repeat(4096), true).is_ok());
    assert!(validate_text(&"x".repeat(4097), true).is_err());
    assert!(validate_text("  ", true).is_err());
    assert!(validate_text("\0", false).is_err());
    assert!(validate_path(&"p".repeat(1024)).is_ok());
    assert!(validate_path(&"p".repeat(1025)).is_err());
    assert!(validate_key(&"k".repeat(128)).is_ok());
    for key in ["", " space", "noné", &"k".repeat(129)] {
        assert!(validate_key(key).is_err());
    }
    assert!(LineRange { start: 1, end: 400 }.validate().is_ok());
    assert!(LineRange { start: 1, end: 401 }.validate().is_err());
    let mut summary = AnnotationSummary {
        threads: 255,
        entries: 63,
        text_bytes: TASK_TEXT_BYTES - 1,
        next_thread: 256,
    };
    assert!(summary.admit(63, "x", true).is_ok());
    assert!(summary.admit(64, "", false).is_err());
    assert!(summary.admit(63, "xx", false).is_err());
    summary.threads = 256;
    assert!(summary.admit(0, "", true).is_err());
    let mut t = thread("T-001");
    t.state = AnnotationState::Resolved;
    assert!(t.validate().is_err());
}
