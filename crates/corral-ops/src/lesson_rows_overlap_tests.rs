use super::*;

#[test]
fn prepared_snapshot_keeps_due_and_first_signals_after_newer_preparation() {
    // Given a prepared Done task whose snapshot has never consumed signals.
    let (_dir, ctx) = fixture();
    let task = ctx
        .store
        .create("Done", Some("app"), Status::Done, "")
        .unwrap();
    let (expected_due, due_counts) = measure(|| crate::lessons_due(&ctx, ts(3600)).unwrap());
    let (expected_signals, signal_counts) = measure(|| crate::lesson_signals(&ctx, "app").unwrap());
    let frozen = Ctx {
        store: ctx.store.read_only_snapshot().unwrap(),
        ..ctx.clone()
    };
    let expected = legacy(&frozen, "app");
    prepare_evaluation(&frozen).unwrap();
    assert!(crate::lessons_due(&frozen, ts(3600)).unwrap().is_empty());

    // When a live evaluation prepares a failed gate absent from the frozen view.
    ctx.store
        .log_event(Some(&task.id), "gate", json!({"result":"fail","run":"R1"}))
        .unwrap();
    assert_eq!(crate::lessons_due(&ctx, ts(3600)).unwrap().len(), 1);

    // Then even first signal consumption and repeated due reads retain the old state.
    let (due, after_due_counts) = measure(|| crate::lessons_due(&frozen, ts(3600)).unwrap());
    let (signals, after_signal_counts) = measure(|| crate::lesson_signals(&frozen, "app").unwrap());
    assert_eq!(
        serde_json::to_vec(&due).unwrap(),
        serde_json::to_vec(&expected_due).unwrap()
    );
    assert_eq!(signals, expected);
    assert_eq!(
        serde_json::to_vec(&signals).unwrap(),
        serde_json::to_vec(&expected_signals).unwrap()
    );
    same_cells(&due_counts, &after_due_counts);
    same_cells(&signal_counts, &after_signal_counts);
    assert_ne!(crate::lesson_signals(&ctx, "app").unwrap(), expected);
}

#[test]
fn prepared_snapshot_keeps_first_signals_read_after_newer_preparation() {
    // Given a frozen evaluation with no prior signal consumption.
    let (_dir, ctx) = fixture();
    let task = ctx
        .store
        .create("Done", Some("app"), Status::Done, "")
        .unwrap();
    let (expected, counts) = measure(|| crate::lesson_signals(&ctx, "app").unwrap());
    let frozen = Ctx {
        store: ctx.store.read_only_snapshot().unwrap(),
        ..ctx.clone()
    };
    prepare_evaluation(&frozen).unwrap();
    // When another evaluation prepares a gate failure.
    ctx.store
        .log_event(Some(&task.id), "gate", json!({"result":"fail"}))
        .unwrap();
    assert_eq!(crate::lessons_due(&ctx, ts(3600)).unwrap().len(), 1);
    // Then first consumption cannot observe the newer generation.
    let (actual, after) = measure(|| crate::lesson_signals(&frozen, "app").unwrap());
    assert_eq!(
        serde_json::to_vec(&actual).unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
    same_cells(&counts, &after);
}

#[test]
fn prepared_snapshot_keeps_first_due_read_after_newer_preparation_and_cache_loss() {
    // Given preparation without consuming either derived output.
    let (_dir, ctx) = fixture();
    let task = ctx
        .store
        .create("Done", Some("app"), Status::Done, "")
        .unwrap();
    let frozen = Ctx {
        store: ctx.store.read_only_snapshot().unwrap(),
        ..ctx.clone()
    };
    let expected = legacy(&frozen, "app");
    prepare_evaluation(&frozen).unwrap();

    // When a writer advances the derivation before the old snapshot's first due read.
    ctx.store
        .log_event(Some(&task.id), "gate", json!({"result":"fail"}))
        .unwrap();
    assert_eq!(crate::lessons_due(&ctx, ts(3600)).unwrap().len(), 1);
    assert!(crate::lessons_due(&frozen, ts(3600)).unwrap().is_empty());
    std::fs::remove_dir_all(ctx.store.cache_root().unwrap()).unwrap();

    // Then the evaluation owns its prepared rows, not references into that directory.
    assert!(crate::lessons_due(&frozen, ts(3600)).unwrap().is_empty());
    assert_eq!(crate::lesson_signals(&frozen, "app").unwrap(), expected);
}

#[test]
fn prepared_snapshot_keeps_first_packet_approval_attribution_after_newer_preparation() {
    use crate::integration::{IntegrationPacket, RepairKey};
    // Given real divergent Git history and a prepared snapshot with one approval.
    let (dir, ctx) = fixture();
    let repo = dir.path().join("app");
    std::fs::create_dir(&repo).unwrap();
    let git = |args: &[&str]| {
        let out = crate::test_support::command("git", &repo)
            .arg("-C")
            .arg(&repo)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("base.rs"), "fn base() {}\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "base"]);
    let initial = git(&["rev-parse", "HEAD"]);
    git(&["checkout", "-qb", "candidate"]);
    std::fs::write(repo.join("candidate.rs"), "fn candidate() {}\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "candidate"]);
    let head = git(&["rev-parse", "HEAD"]);
    git(&["checkout", "-q", "main"]);
    std::fs::write(repo.join("landed.rs"), "fn landed() {}\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "landed"]);
    let base = git(&["rev-parse", "HEAD"]);
    let task = ctx
        .store
        .create("Candidate", Some("app"), Status::Review, "")
        .unwrap();
    let landed = ctx
        .store
        .create("Landed", Some("app"), Status::Done, "")
        .unwrap();
    ctx.store
        .log_event(
            Some(&landed.id),
            "approved",
            json!({"sha":base,"strategy":"rebase","run":"R1"}),
        )
        .unwrap();
    let frozen = Ctx {
        store: ctx.store.read_only_snapshot().unwrap(),
        ..ctx.clone()
    };
    prepare_evaluation(&frozen).unwrap();
    let run: Run = serde_json::from_value(json!({
        "id":"R1","agent":"worker","worktree":repo,"branch":"candidate","base":"main",
        "base_sha":initial,"started":ts(0),"outcome":"done"
    }))
    .unwrap();
    let key = RepairKey {
        task: task.id.clone(),
        run: run.id.clone(),
        head,
        base: base.clone(),
        reason: "merge".into(),
    };
    let packet = |ctx: &Ctx| {
        IntegrationPacket::build(
            ctx,
            &task,
            &run,
            &corral_core::ReviewPolicy::default(),
            key.clone(),
            vec![],
            None,
        )
        .unwrap()
    };
    // Warm Git facts using another evaluation; the frozen packet is still unconsumed.
    let expected = packet(&ctx);
    let (_, base_counts) = measure(|| packet(&ctx));

    // When a second approval of the same SHA is prepared before the first old packet.
    ctx.store
        .log_event(
            Some(&landed.id),
            "approved",
            json!({"sha":base,"strategy":"rebase","run":"R2"}),
        )
        .unwrap();
    ctx.store
        .update(&landed.id, |task| {
            task.title = "Renamed after snapshot".into();
            Ok(())
        })
        .unwrap();
    for i in 0..10 {
        let mut old = ctx
            .store
            .create("Unrelated history", Some("other"), Status::Done, "")
            .unwrap();
        old.runs.extend((0..10).map(|j| super::run(j + 1, "")));
        ctx.store.fixture_put_task(&old).unwrap();
        ctx.store
            .log_event(
                Some(&old.id),
                "approved",
                json!({"sha":format!("unrelated-{i}"),"run":"R1"}),
            )
            .unwrap();
    }
    let fresh = packet(&ctx);
    assert_eq!(fresh.dispatch_history[0].tasks.len(), 2);

    // Then the old packet includes only its committed approval and task title.
    let (old, grown_counts) = measure(|| packet(&frozen));
    assert_eq!(
        serde_json::to_vec(&old).unwrap(),
        serde_json::to_vec(&expected).unwrap()
    );
    same_cells(&base_counts, &grown_counts);
    assert_eq!(old.dispatch_history[0].tasks.len(), 1);
    assert_eq!(old.dispatch_history[0].tasks[0].run.as_deref(), Some("R1"));
    assert_eq!(
        old.dispatch_history[0].tasks[0].title.as_deref(),
        Some("Landed")
    );
    assert_eq!(
        old.missing_from_candidate[0].tasks,
        old.dispatch_history[0].tasks
    );
    assert_eq!(packet(&ctx), fresh);
}
