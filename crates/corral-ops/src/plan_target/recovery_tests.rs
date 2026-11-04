use super::*;

#[test]
fn cancelling_execution_plan_retains_dirty_bytes_and_restart_inventory() {
    let f = Fixture::new();
    let run = f.task.current_run().unwrap();
    fs::write(run.worktree.join("first.txt"), b"unfinished\0bytes").unwrap();
    fs::write(run.worktree.join("untracked"), b"only copy").unwrap();
    // Owner-origin Plans keep the existing guarded disposition policy. The board's
    // Drop action and the CLI's --owner-ok use this same non-destructive operation.
    let mut plan = f.ctx.store.get(&f.target.plan).unwrap();
    plan.origin = Some(Origin::Owner);
    f.ctx.store.fixture_put_task(&plan).unwrap();
    let cancelled = crate::move_task_with_decision(
        &f.ctx,
        &f.target.plan,
        Status::Dropped,
        false,
        Some("Stop coordination, retain work"),
        "owner",
        true,
    )
    .unwrap();
    assert_eq!(cancelled.status, Status::Dropped);
    assert!(crate::move_task(&f.ctx, &f.target.plan, Status::Ready, true).is_err());
    crate::plan_cancel(&f.reopen(), &f.target.plan).unwrap();
    assert_eq!(
        fs::read(run.worktree.join("first.txt")).unwrap(),
        b"unfinished\0bytes"
    );
    assert_eq!(
        fs::read(run.worktree.join("untracked")).unwrap(),
        b"only copy"
    );
    let progress = crate::plan_progress(&f.reopen(), &f.target.plan).unwrap();
    let value = serde_json::to_value(progress).unwrap();
    let recovery = &value["recovery"];
    assert_eq!(recovery["plan"], f.target.plan);
    assert!(recovery["resources"].as_array().unwrap().iter().any(|r| {
        r["path"] == json!(run.worktree) && r["status"].as_str().unwrap().contains("untracked")
    }));
    assert!(!recovery["outstanding"].as_array().unwrap().is_empty());
    assert!(corral_git::rev_sha(&f.repo, &run.branch).is_ok());
    let consumer = f.ctx.store.get(&cancelled.children[1]).unwrap();
    assert!(
        crate::tracker::target_blocker(&f.ctx.store, &consumer)
            .unwrap()
            .unwrap()
            .contains("cancelled")
    );
    f.untouched_main();
}

#[test]
fn cancelled_plan_does_not_publish_a_late_member_landing() {
    let mut f = Fixture::new();
    f.prepare();
    f.move_git();
    // Cancellation wins coordination; Git movement remains a recovery obligation, not admission.
    crate::plan_cancel(&f.ctx, &f.target.plan).unwrap();
    assert!(reconcile_all(&f.reopen()).is_err());
    assert_eq!(f.approvals(), 0);
    assert_eq!(
        f.ctx.store.get(&f.target.plan).unwrap().status,
        Status::Dropped
    );
    assert_eq!(f.ctx.store.get(&f.task.id).unwrap().status, Status::Review);
    let target = Target::load(&f.repo, "feature/plan").unwrap().unwrap();
    assert!(target.pending.is_some() || target.receipts.iter().any(|r| !r.is_published()));
    assert!(f.task.current_run().unwrap().worktree.is_dir());
    f.untouched_main();
}

#[test]
fn missing_session_retry_continues_committed_plan_work_in_one_new_run() {
    let mut f = Fixture::new();
    let engine = f._dir.path().join("herdr");
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/plan_engine.sh"),
        &engine,
    )
    .unwrap();
    f.ctx.herdr = corral_herdr::Herdr::with_bin(engine, Some("corral-test".into()));
    f.ctx.store.config.agents.insert(
        "fixture".into(),
        corral_core::AgentCmd {
            command: "true".into(),
        },
    );
    f.ctx.store.config.defaults.profile = Some("fixture".into());
    f.ctx.store.save_config().unwrap();
    let previous = f.task.current_run().unwrap().clone();
    let head = corral_git::rev_sha(&previous.worktree, "HEAD").unwrap();
    let outcome = crate::retry_run(
        &f.ctx,
        &f.task.id,
        "R1",
        &corral_core::ProfileChoice::default(),
    )
    .unwrap();
    assert!(matches!(outcome, crate::DispatchOutcome::Started { .. }));
    let task = f.ctx.store.get(&f.task.id).unwrap();
    assert_eq!(task.runs.len(), 2);
    let current = task.current_run().unwrap();
    assert_eq!(current.id, "R2");
    assert_ne!(current.worktree, previous.worktree);
    assert_eq!(
        corral_git::rev_sha(&current.worktree, "HEAD").unwrap(),
        head
    );
    assert_eq!(
        fs::read(current.worktree.join("first.txt")).unwrap(),
        b"accepted work\n"
    );
    assert!(previous.worktree.is_dir());
    assert!(
        crate::retry_run(
            &f.ctx,
            &task.id,
            "R1",
            &corral_core::ProfileChoice::default()
        )
        .is_err()
    );
    assert_eq!(f.ctx.store.get(&task.id).unwrap().runs.len(), 2);
    f.untouched_main();
}

#[test]
fn failed_plan_landing_cleanup_keeps_refs_and_retry_obligations() {
    let mut f = Fixture::new();
    f.prepare();
    let mut journal = serde_json::to_value(&f.target).unwrap();
    journal["pending"]["remove_worktree"] = json!(true);
    f.target = serde_json::from_value(journal).unwrap();
    f.target.save().unwrap();
    f.move_git();
    fs::write(
        f.task.current_run().unwrap().worktree.join("untracked"),
        "retain",
    )
    .unwrap();
    reconcile_all(&f.reopen()).unwrap();
    let journal =
        serde_json::to_value(Target::load(&f.repo, "feature/plan").unwrap().unwrap()).unwrap();
    assert_eq!(journal["receipts"][0]["worktree_removed"], false);
    assert_eq!(journal["receipts"][0]["branch_deleted"], false);
    assert!(
        journal["receipts"][0]["cleanup_reason"]
            .as_str()
            .unwrap()
            .contains("dirty")
    );
    assert_eq!(
        fs::read(f.task.current_run().unwrap().worktree.join("untracked")).unwrap(),
        b"retain"
    );
    assert!(corral_git::rev_sha(&f.repo, &f.task.current_run().unwrap().branch).is_ok());
    let approved = f
        .ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .find(|e| e.kind == "approved")
        .unwrap();
    assert_eq!(approved.detail["cleanup"]["remove_worktree"], true);
}
