use super::*;
use corral_core::{Run, Status};
use std::{path::Path, process::Command};

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "core.fsmonitor")
        .env("GIT_CONFIG_VALUE_0", "false")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.test")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.test")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}
fn owner() -> AnnotationActor<'static> {
    AnnotationActor::authenticated(|_, _| {
        Ok(AnnotationAuthor::Owner {
            version: 1,
            surface: AnnotationSurface::Cli,
        })
    })
}
struct Fixture {
    dir: tempfile::TempDir,
    store: Store,
    task: String,
    action: OpenAnnotation,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-b", "main"]);
        std::fs::write(dir.path().join("code.txt"), "old\n").unwrap();
        std::fs::write(dir.path().join("unchanged"), "unchanged\n").unwrap();
        git(dir.path(), &["add", "code.txt", "unchanged"]);
        git(dir.path(), &["commit", "-m", "base"]);
        let base = git(dir.path(), &["rev-parse", "HEAD"]);
        git(dir.path(), &["branch", "base"]);
        std::fs::write(dir.path().join("code.txt"), "one\ntwo\nthree\n").unwrap();
        std::fs::write(dir.path().join("binary"), [0, 1, 2]).unwrap();
        git(dir.path(), &["add", "code.txt", "binary"]);
        git(dir.path(), &["commit", "-m", "candidate"]);
        let head = git(dir.path(), &["rev-parse", "HEAD"]);
        let store = Store::init(dir.path()).unwrap();
        let task = store
            .create("Annotated", None, Status::Review, "Spec")
            .unwrap();
        store
            .update(&task.id, |t| {
                let r: Run =
                    serde_json::from_value(json!({"id":"R1","worktree":dir.path(),"branch":"main",
                "base":"base","base_sha":base,"agent":"test","started":"2026-01-01T00:00:00Z"}))
                    .unwrap();
                t.runs.push(r);
                Ok(())
            })
            .unwrap();
        Self {
            dir,
            store,
            task: task.id,
            action: OpenAnnotation {
                expected_run: "R1".into(),
                base,
                head,
                path: "code.txt".into(),
                side: AnnotationSide::New,
                range: Some(LineRange { start: 1, end: 2 }),
                text: "Check the bound".into(),
            },
        }
    }
    fn open(&self) -> AnnotationResult {
        open(
            &self.store,
            &owner(),
            &self.task,
            "open",
            self.action.clone(),
        )
        .unwrap()
    }
    fn change(&self, revision: u32) -> ChangeAnnotation {
        ChangeAnnotation {
            thread: "N1".into(),
            expected_revision: revision,
            text: "note".into(),
        }
    }
    fn snapshot(&self) -> Value {
        use corral_core::backend::{TaskNum, TaskParts};
        let view = self.store.view().unwrap();
        let num = TaskNum(self.task[2..].parse().unwrap());
        json!({"seq":view.seq(),"task":view.task(num, &TaskParts::All).unwrap()})
    }
}
fn code(error: anyhow::Error) -> AnnotationCode {
    error.downcast::<AnnotationError>().unwrap().code
}

#[test]
fn annotations_durable_retry_status_and_identity_isolation() {
    let f = Fixture::new();
    let ctx = crate::Ctx::from_store(f.store.clone());
    let before = f.store.head(&f.task).unwrap();
    let identity = crate::verification::capture_identity(
        &ctx,
        &before,
        before.current_run.as_ref().unwrap(),
        "gate",
    )
    .unwrap();
    f.store
        .update(&f.task, |t| {
            t.runs[0].gate = Some(
                serde_json::from_value(json!({"result":"pass","sha":f.action.head,
            "identity":identity,"at":"2026-01-01T00:00:00Z"}))
                .unwrap(),
            );
            t.runs[0].review = Some(
                serde_json::from_value(json!({"verdict":"approve","sha":f.action.head,
            "reviewer":"owner","evidence_digest":"packet-digest","at":"2026-01-01T00:00:00Z"}))
                .unwrap(),
            );
            Ok(())
        })
        .unwrap();
    let before = f.store.head(&f.task).unwrap();
    let mut audit =
        crate::completion_check(&ctx, &f.task, "Verified: fixture\nNot verified: none").unwrap();
    let spec = corral_core::store::spec_revision(&f.store.get(&f.task).unwrap()).unwrap();
    assert_eq!(f.open().thread, "N1");
    resolve(&f.store, &owner(), &f.task, "resolve", f.change(1)).unwrap();
    let after = f.store.head(&f.task).unwrap();
    assert_eq!(before.current_run, after.current_run);
    assert_eq!(
        crate::verification::capture_identity(
            &ctx,
            &after,
            after.current_run.as_ref().unwrap(),
            "gate"
        )
        .unwrap(),
        identity
    );
    let new_audit =
        crate::completion_check(&ctx, &f.task, "Verified: fixture\nNot verified: none").unwrap();
    audit.checked = new_audit.checked;
    assert_eq!(
        serde_json::to_value(audit).unwrap(),
        serde_json::to_value(new_audit).unwrap()
    );
    let initial = f.snapshot();
    let view = list(&f.store, &f.task, &AnnotationsQuery::default()).unwrap();
    let AnnotationsPage::List { items } = view.page else {
        panic!()
    };
    assert!(matches!(items[0].anchor_status, AnchorStatus::Current));
    let anchor = items[0].anchor.clone();
    assert_eq!(f.snapshot(), initial);
    let reopened = Store::open(f.dir.path()).unwrap();
    assert_eq!(
        reopened.get(&f.task).unwrap().annotations,
        f.store.get(&f.task).unwrap().annotations
    );
    // Full compatibility writes preserve the rows and exclude them from the spec identity.
    f.store
        .update(&f.task, |t| {
            t.result = None;
            Ok(())
        })
        .unwrap();
    assert_eq!(f.store.get(&f.task).unwrap().annotations.len(), 1);
    assert_eq!(
        corral_core::store::spec_revision(&f.store.get(&f.task).unwrap()).unwrap(),
        spec
    );
    let before = f.snapshot();
    assert!(
        open(&reopened, &owner(), &f.task, "open", f.action.clone())
            .unwrap()
            .replayed
    );
    assert_eq!(f.snapshot(), before);
    std::fs::write(f.dir.path().join("code.txt"), "new head\n").unwrap();
    git(f.dir.path(), &["add", "code.txt"]);
    git(f.dir.path(), &["commit", "-m", "move"]);
    let stale = list(&f.store, &f.task, &AnnotationsQuery::default()).unwrap();
    let AnnotationsPage::List { items } = stale.page else {
        panic!()
    };
    assert!(
        matches!(&items[0].anchor_status, AnchorStatus::Stale { changes } if changes.iter().any(|c| c.field == "head"))
    );
    assert_eq!(items[0].anchor, anchor);
    assert!(
        open(&f.store, &owner(), &f.task, "open", f.action.clone())
            .unwrap()
            .replayed
    );
    assert_eq!(
        code(open(&f.store, &owner(), &f.task, "new-key", f.action.clone()).unwrap_err()),
        AnnotationCode::StaleCandidate
    );
    f.store
        .update(&f.task, |t| {
            let mut run = t.runs[0].clone();
            run.id = "R2".into();
            t.runs.push(run);
            Ok(())
        })
        .unwrap();
    let mut old_run = f.action.clone();
    old_run.head = git(f.dir.path(), &["rev-parse", "HEAD"]);
    let before_retarget = f.snapshot();
    let err = open(&f.store, &owner(), &f.task, "old-run", old_run)
        .unwrap_err()
        .downcast::<AnnotationError>()
        .unwrap();
    assert_eq!(err.code, AnnotationCode::StaleCandidate);
    assert_eq!(err.detail["run"], "R2");
    assert_eq!(f.snapshot(), before_retarget);
    // No current worktree: replay precedes any Git reads, and superseded wins over unavailable.
    f.store
        .update(&f.task, |t| {
            t.runs.last_mut().unwrap().worktree = f.dir.path().join("gone");
            Ok(())
        })
        .unwrap();
    let before = f.snapshot();
    let unavailable = list(&f.store, &f.task, &AnnotationsQuery::default()).unwrap();
    assert!(matches!(
        unavailable.candidate,
        CandidateAvailability::Unavailable { .. }
    ));
    let AnnotationsPage::List { items } = unavailable.page else {
        panic!()
    };
    assert!(matches!(items[0].anchor_status, AnchorStatus::Superseded));
    assert_eq!(items[0].anchor, anchor);
    assert!(
        open(&f.store, &owner(), &f.task, "open", f.action.clone())
            .unwrap()
            .replayed
    );
    let mut changed = f.action.clone();
    changed.text.push('!');
    assert_eq!(
        code(open(&f.store, &owner(), &f.task, "open", changed).unwrap_err()),
        AnnotationCode::RequestKeyConflict
    );
    let denied = AnnotationActor::authenticated(|_, _| {
        Err(annotation_error(
            AnnotationCode::NotOwner,
            "no",
            Value::Null,
        ))
    });
    assert_eq!(
        code(open(&f.store, &denied, &f.task, "open", f.action.clone()).unwrap_err()),
        AnnotationCode::NotOwner
    );
    assert_eq!(before, f.snapshot());
}

#[test]
fn annotations_invalid_anchors_are_noops() {
    let f = Fixture::new();
    let before = f.snapshot();
    for (path, range, expected) in [
        ("/etc/passwd", None, AnnotationCode::InvalidPath),
        ("../x", None, AnnotationCode::InvalidPath),
        (".git/config", None, AnnotationCode::InvalidPath),
        ("", None, AnnotationCode::InvalidPath),
        ("absent", None, AnnotationCode::PathOutOfScope),
        ("unchanged", None, AnnotationCode::PathOutOfScope),
        (
            "code.txt",
            Some(LineRange { start: 0, end: 2 }),
            AnnotationCode::InvalidRange,
        ),
        (
            "code.txt",
            Some(LineRange { start: 2, end: 1 }),
            AnnotationCode::InvalidRange,
        ),
        (
            "code.txt",
            Some(LineRange { start: 1, end: 401 }),
            AnnotationCode::InvalidRange,
        ),
        (
            "code.txt",
            Some(LineRange { start: 1, end: 4 }),
            AnnotationCode::InvalidRange,
        ),
        (
            "binary",
            Some(LineRange { start: 1, end: 1 }),
            AnnotationCode::BinaryRange,
        ),
    ] {
        let mut a = f.action.clone();
        a.path = path.into();
        a.range = range;
        assert_eq!(
            code(open(&f.store, &owner(), &f.task, "invalid", a).unwrap_err()),
            expected
        );
        assert_eq!(f.snapshot(), before);
    }
}

#[test]
fn annotations_reply_resolve_race_uses_revision_cas() {
    let f = Fixture::new();
    f.open();
    let barrier = std::sync::Barrier::new(2);
    let results = std::thread::scope(|s| {
        let a = s.spawn(|| {
            barrier.wait();
            reply(&f.store, &owner(), &f.task, "race-reply", f.change(1))
        });
        let b = s.spawn(|| {
            barrier.wait();
            resolve(&f.store, &owner(), &f.task, "race-resolve", f.change(1))
        });
        vec![a.join().unwrap(), b.join().unwrap()]
    });
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        code(results.into_iter().find_map(Result::err).unwrap()),
        AnnotationCode::ThreadRevisionConflict
    );
    let t = f
        .store
        .head(&f.task)
        .unwrap()
        .annotation_thread("N1")
        .unwrap()
        .unwrap();
    assert_eq!(t.revision, 2);
    let next = if t.state == AnnotationState::Open {
        resolve(&f.store, &owner(), &f.task, "close", f.change(2))
    } else {
        reply(&f.store, &owner(), &f.task, "resolved-reply", f.change(2))
    }
    .unwrap();
    assert_eq!(next.state, AnnotationState::Resolved);
    assert_eq!(
        reopen(&f.store, &owner(), &f.task, "reopen", f.change(3))
            .unwrap()
            .state,
        AnnotationState::Open
    );
}

#[test]
fn annotations_file_level_hash_streams_beyond_the_line_blob_cap() {
    let f = Fixture::new();
    // Smallest blob over the named 8-MiB line-anchor cap; not a performance fixture.
    let size = 8 * 1024 * 1024 + 1;
    std::fs::File::create(f.dir.path().join("large"))
        .unwrap()
        .set_len(size)
        .unwrap();
    git(f.dir.path(), &["add", "large"]);
    git(f.dir.path(), &["commit", "-m", "large blob"]);
    let mut a = f.action.clone();
    a.path = "large".into();
    a.head = git(f.dir.path(), &["rev-parse", "HEAD"]);
    let before = f.snapshot();
    assert_eq!(
        code(open(&f.store, &owner(), &f.task, "large", a.clone()).unwrap_err()),
        AnnotationCode::FileTooLarge
    );
    assert_eq!(f.snapshot(), before);
    a.range = None;
    let h = f.store.head(&f.task).unwrap();
    let c = current(&h, &a).unwrap();
    let anchored = anchor(&h, &a, &c).unwrap();
    assert!(anchored.excerpt.len() <= 2048 && anchored.excerpt_truncated);
    assert_eq!(anchored.excerpt_sha256, hash(&vec![0; size as usize]));
    assert_eq!(f.snapshot(), before);
}

#[test]
fn annotations_pages_freeze_upper_revision_and_bound_encoded_bytes() {
    let f = Fixture::new();
    f.open();
    // One legal thread, not a large history: escaping is the named frame-size defect.
    for revision in 1..63 {
        let mut a = f.change(revision);
        a.text = "\u{1}".repeat(4096);
        reply(&f.store, &owner(), &f.task, &format!("r{revision}"), a).unwrap();
    }
    let first = show(&f.store, &f.task, "N1", &AnnotationsQuery::default()).unwrap();
    let mut a = f.change(63);
    a.text = "64".into();
    reply(&f.store, &owner(), &f.task, "r63", a).unwrap();
    let mut page = first;
    let mut seqs = vec![];
    loop {
        assert!(serde_json::to_vec(&page).unwrap().len() <= RESPONSE_BYTES);
        let AnnotationsPage::Thread { entries, .. } = &page.page else {
            panic!()
        };
        seqs.extend(entries.iter().map(|e| e.seq));
        let Some(cursor) = page.next_cursor else {
            break;
        };
        page = show(
            &f.store,
            &f.task,
            "N1",
            &AnnotationsQuery {
                cursor: Some(cursor),
                ..Default::default()
            },
        )
        .unwrap();
    }
    assert_eq!(seqs, (1..=63).collect::<Vec<_>>());
    let mut q = AnnotationsQuery::default();
    let mut all = vec![];
    loop {
        let page = show(&f.store, &f.task, "N1", &q).unwrap();
        assert!(serde_json::to_vec(&page).unwrap().len() <= RESPONSE_BYTES);
        let AnnotationsPage::Thread { entries, .. } = page.page else {
            panic!()
        };
        all.extend(entries.into_iter().map(|e| e.seq));
        q.cursor = page.next_cursor;
        if q.cursor.is_none() {
            break;
        }
    }
    assert_eq!(all, (1..=64).collect::<Vec<_>>());
    let before = f.snapshot();
    assert_eq!(
        code(reply(&f.store, &owner(), &f.task, "65", f.change(64)).unwrap_err()),
        AnnotationCode::AnnotationLimit
    );
    assert_eq!(f.snapshot(), before);
}
