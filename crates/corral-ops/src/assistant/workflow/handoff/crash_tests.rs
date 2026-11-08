//! Slow: publication/crash matrix over durable workflow records; owning trigger is handoff/.
use super::tests::{planning, record_source, repo, request};
use super::*;
use crate::agent::{FAILPOINT, test_support as ts};

#[test]
fn handoff_publication_crash_matrix_reconciles_package_and_reservation_without_effect_replay() {
    for (point, occurrence) in [
        ("handoff_snapshot_captured", 1),
        ("handoff_content_published", 1),
        ("handoff_content_published", 6),
        ("handoff_restore_verified", 1),
        ("handoff_package_prepared", 1),
        ("handoff_preview_published", 1),
        ("handoff_reservation_staged", 1),
    ] {
        let mut f = ts::fx();
        let (session, container, expected) = planning(&mut f);
        let source = f._tmp.path().join("source");
        let base = repo(&source);
        record_source(&f, &session, &source, &base);
        let before = git::scan(&source, &base, "work").unwrap().state;
        let seen = std::cell::Cell::new(0);
        FAILPOINT.with_borrow_mut(|hook| {
            *hook = Some(Box::new(move |p| {
                if p == point {
                    seen.set(seen.get() + 1);
                    if seen.get() == occurrence {
                        panic!("crash at {point} #{occurrence}");
                    }
                }
            }))
        });
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let prepared = prepare(
                &f.ctx.store,
                &session,
                &container,
                &expected,
                request(),
                Some((&source, &base, "work")),
            )
            .unwrap();
            f.ctx
                .store
                .update_assistant_container(&session, |c| {
                    reserve(&f.ctx.store, &session, c, &prepared)
                })
                .unwrap();
        }));
        FAILPOINT.with_borrow_mut(|hook| *hook = None);
        assert!(
            crashed.is_err(),
            "fault boundary {point} #{occurrence} was not reached"
        );
        assert_eq!(
            f.ctx.store.get_assistant_container(&session).unwrap(),
            container
        );
        assert_eq!(git::scan(&source, &base, "work").unwrap().state, before);
        assert!(f.ctx.store.record_rows(JOURNALS).unwrap().is_empty());
        let published = usize::from(matches!(
            point,
            "handoff_preview_published" | "handoff_reservation_staged"
        ));
        assert_eq!(
            f.ctx.store.record_rows(PACKAGES).unwrap().len(),
            published,
            "interrupted capture is not a completed preview"
        );
        assert_eq!(
            f.ctx.store.record_rows(INTENTS).unwrap().len(),
            published,
            "intent lookup publishes atomically with the preview"
        );
        let prepared = prepare(
            &f.ctx.store,
            &session,
            &container,
            &expected,
            request(),
            Some((&source, &base, "work")),
        )
        .unwrap();
        // Simulate committed publication with a lost acknowledgement, then reopen/read/recover.
        let journal = f
            .ctx
            .store
            .update_assistant_container(&session, |c| reserve(&f.ctx.store, &session, c, &prepared))
            .unwrap();
        let reopened = Store::open(f._tmp.path()).unwrap();
        assert_eq!(
            read_journal(&reopened, &journal.id).unwrap(),
            Some(journal.clone())
        );
        let recovered = recover(&reopened, &journal.id).unwrap();
        let again = reopened
            .update_assistant_container(&session, |c| reserve(&reopened, &session, c, &recovered))
            .unwrap();
        assert_eq!(again, journal);
        assert_eq!(reopened.record_rows(JOURNALS).unwrap().len(), 1);
        assert_eq!(reopened.record_rows(PACKAGES).unwrap().len(), 1);
        assert_eq!(reopened.record_rows(INTENTS).unwrap().len(), 1);
        assert!(
            reopened.list().unwrap().is_empty(),
            "C2 cannot file or dispatch"
        );
        assert_eq!(
            reopened
                .get_assistant_container(&session)
                .unwrap()
                .workflow
                .unwrap()
                .current
                .owner,
            ScopeOwner::Local
        );
        assert!(require_source_releasable(&reopened, &session, &source).is_err());
    }
}

// Slow: one held-publication interruption, source-session loss and blocked attachments; owning trigger is handoff/.
#[test]
fn handoff_publication_crash_matrix_recovers_one_held_card_after_source_close_and_attachment_failure()
 {
    use super::plan_adoption_tests::instruction;
    let mut f = ts::fx();
    let (session, c, expected) = planning(&mut f);
    f.ctx.store.save_config().unwrap();
    let prepared = prepare(&f.ctx.store, &session, &c, &expected, request(), None).unwrap();
    let choice =
        transfer::preview(&f.ctx, prepared.id(), "Held source recovery", None, true).unwrap();
    FAILPOINT.with_borrow_mut(|hook| {
        *hook = Some(Box::new(|point| {
            if point == "handoff_task_created" {
                panic!("held card durable before adoption/ownership");
            }
        }))
    });
    let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        transfer::commit(&f.ctx, &choice, &choice.digest().unwrap(), instruction()).unwrap()
    }));
    FAILPOINT.with_borrow_mut(|hook| *hook = None);
    assert!(interrupted.is_err());
    let tasks = f.ctx.store.list().unwrap();
    assert_eq!(tasks.len(), 1);
    let held = f.ctx.store.get(&tasks[0].id).unwrap();
    assert!(held.dispatch_hold && held.runs.is_empty() && held.plan_runs.is_empty());
    assert!(
        crate::operating::require(&f.ctx.store, &held, corral_core::OperatingActionKind::Plan)
            .is_err()
    );
    let journal = read_journal(&f.ctx.store, prepared.id()).unwrap().unwrap();
    assert!(!journal.transfer.as_ref().unwrap().ownership_committed);
    assert!(f.ctx.store.pm_inbox_items().unwrap().is_empty());
    // Canonical source-session closure is a fixture for losing source availability;
    // no actual SDK host/process existed in this generation-zero source.
    f.ctx.store.end_assistant(&session).unwrap();
    let cap = f.ctx.store.config.artifacts.max_bytes;
    f.ctx.store.config.artifacts.max_bytes = 1;
    f.ctx.store.save_config().unwrap();
    let mut reopened = f.ctx.clone();
    reopened.store = Store::open(f.ctx.store.root()).unwrap();
    recovery::reconcile(&reopened).unwrap();
    let blocked = read_journal(&reopened.store, prepared.id())
        .unwrap()
        .unwrap();
    assert!(!blocked.transfer.as_ref().unwrap().ownership_committed && blocked.waiting.is_some());
    assert_eq!(
        reopened
            .store
            .get_assistant_container(&session)
            .unwrap()
            .workflow
            .unwrap()
            .current
            .owner,
        ScopeOwner::Local
    );
    assert!(reopened.store.pm_inbox_items().unwrap().is_empty());
    reopened.store.config.artifacts.max_bytes = cap;
    reopened.store.save_config().unwrap();
    recovery::reconcile(&reopened).unwrap();
    recovery::reconcile(&reopened).unwrap();
    let final_task = reopened.store.get(&held.id).unwrap();
    let complete = lifecycle::for_task(&reopened.store, &final_task)
        .unwrap()
        .unwrap();
    let transfer = complete.transfer.unwrap();
    assert!(
        transfer.ownership_committed && transfer.adoption_proven && !transfer.notification_pending
    );
    assert!(
        final_task.dispatch_hold && final_task.runs.is_empty() && final_task.plan_runs.is_empty()
    );
    assert_eq!(reopened.store.list().unwrap().len(), 1);
    assert_eq!(reopened.store.pm_inbox_items().unwrap().len(), 1);
    assert!(
        reopened
            .store
            .get_assistant(&session)
            .unwrap()
            .ended
            .is_some()
    );
    assert_eq!(
        reopened
            .store
            .get_assistant_container(&session)
            .unwrap()
            .workflow
            .unwrap()
            .current
            .owner,
        ScopeOwner::Board {
            task: held.id.clone()
        }
    );
    let events = reopened.store.events().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "handoff_ownership_committed")
            .count(),
        1
    );
    eprintln!(
        "HELD_PUBLICATION_RECOVERY: one durable card survived source-session closure and attachment-cap failure; no early owner/outbox, then once-only Board ownership/notification and zero launches; no live-host death claimed"
    );
}

// Slow: persisted partial seed/index restart under a moved integration base; owning trigger is handoff/.
#[test]
fn handoff_materialization_restart_preserves_pinned_layers_and_the_moved_base() {
    let mut f = ts::fx();
    let source = f._tmp.path().join("repo");
    let base = repo(&source);
    git::git(&source, &["branch", "main", &base], None).unwrap();
    f.ctx.store.config.projects.insert(
        "app".into(),
        serde_json::from_value(serde_json::json!({"path":"repo","base":"main"})).unwrap(),
    );
    f.ctx.store.save_config().unwrap();
    let integration = f._tmp.path().join("integration");
    git::git(
        &source,
        &["worktree", "add", integration.to_str().unwrap(), "main"],
        None,
    )
    .unwrap();
    std::fs::write(source.join("committed"), b"source commit\0\xff").unwrap();
    git::git(&source, &["add", "committed"], None).unwrap();
    git::git(&source, &["commit", "-m", "source work"], None).unwrap();
    std::fs::write(source.join("file"), b"staged\0\xff\r\n").unwrap();
    git::git(&source, &["add", "file"], None).unwrap();
    std::fs::write(source.join("file"), b"work\0\xfe\r\n").unwrap();
    std::fs::remove_file(source.join("deleted")).unwrap();
    std::fs::write(source.join("untracked"), b"retained\r\n\0\xff").unwrap();
    let snapshot = Snapshot::capture(&f.ctx.store, &source, &base, "work").unwrap();
    let task = f
        .ctx
        .store
        .create(
            "Reserved materialization",
            Some("app".into()),
            corral_core::Status::Backlog,
            "",
        )
        .unwrap();
    f.ctx
        .store
        .update(&task.id, |t| {
            t.dispatch_hold = true;
            Ok(())
        })
        .unwrap();
    let destination = f._tmp.path().join("interrupted-seed");
    let branch = "reserved/restart-seed";
    git::git(
        &source,
        &[
            "worktree",
            "add",
            "--no-checkout",
            "-b",
            branch,
            destination.to_str().unwrap(),
            &snapshot.state.head,
        ],
        None,
    )
    .unwrap();
    // Persist a real prefix of materialization: linked worktree, raw index and one
    // exact file. No success/adoption/approval/PASS is recorded for this prefix.
    let index = String::from_utf8(
        git::git(
            &destination,
            &["rev-parse", "--path-format=absolute", "--git-path", "index"],
            None,
        )
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        index.trim(),
        snapshot.state.index.load(&f.ctx.store).unwrap(),
    )
    .unwrap();
    std::fs::write(destination.join("file"), b"work\0\xfe\r\n").unwrap();
    assert!(!destination.join("untracked").exists());
    std::fs::write(
        integration.join("integration-only"),
        b"new main must survive",
    )
    .unwrap();
    git::git(&integration, &["add", "integration-only"], None).unwrap();
    git::git(&integration, &["commit", "-m", "advance integration"], None).unwrap();
    let moved = corral_git::rev_sha(&source, "main").unwrap();
    assert_ne!(moved, base);
    let mut resumed = f.ctx.clone();
    resumed.store = Store::open(f.ctx.store.root()).unwrap();
    let task = resumed.store.get(&task.id).unwrap();
    adopt::restore_worktree(&resumed, &task, &snapshot, &destination, branch).unwrap();
    adopt::verify_worktree(&snapshot, &destination, branch).unwrap();
    adopt::restore_worktree(&resumed, &task, &snapshot, &destination, branch).unwrap();
    assert_eq!(corral_git::rev_sha(&source, "main").unwrap(), moved);
    assert_eq!(
        std::fs::read(integration.join("integration-only")).unwrap(),
        b"new main must survive"
    );
    assert_eq!(
        git::scan(&source, &snapshot.state.base, "work")
            .unwrap()
            .state,
        snapshot.state
    );
    assert_eq!(
        std::fs::read(destination.join("untracked")).unwrap(),
        b"retained\r\n\0\xff"
    );
    std::fs::write(destination.join("file"), b"later owner work").unwrap();
    assert!(adopt::restore_worktree(&resumed, &task, &snapshot, &destination, branch).is_err());
    assert_eq!(
        std::fs::read(destination.join("file")).unwrap(),
        b"later owner work"
    );
    assert!(resumed.store.get(&task.id).unwrap().runs.is_empty());
    eprintln!(
        "MATERIALIZATION_RESTART: exact committed/index/binary/work/untracked layers recovered from persisted prefix; moved main and later owner edits untouched; internal materializer contract, no approval/PASS or code execution"
    );
}
