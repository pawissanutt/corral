//! Exercise actual plan dispatch and its OS sandbox with an isolated engine.
use std::path::Path;
use std::process::Stdio;

use corral_core::{AgentCmd, PlanRunState, Status, Store};
use corral_herdr::Herdr;
use corral_ops::{Ctx, DispatchOpts, DispatchOutcome};

mod support;

fn fixture() -> (tempfile::TempDir, Ctx, String) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("app");
    std::fs::create_dir(&repo).unwrap();
    let output = support::command("git", tmp.path())
        .args(["init", "-q", "-b", "main"])
        .arg(&repo)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success());
    std::fs::write(repo.join("protected.txt"), "original").unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.agents.insert("planner".into(), AgentCmd {
        command: "printf changed > '{worktree}/protected.txt'; printf artifact > '{prompt_file}.result'".into(),
    });
    store.config.defaults.profile = Some("planner".into());
    store.save_config().unwrap();
    let engine = tmp.path().join("engine");
    std::fs::create_dir(&engine).unwrap();
    let bin = engine.join("herdr");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/plan_engine.sh"),
        &bin,
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let task = store
        .create(
            "Design widget",
            Some("app"),
            Status::Ready,
            "Choose the widget contract",
        )
        .unwrap();
    let ctx = Ctx {
        store,
        herdr: Herdr::with_bin(bin, Some("corral-test".into())),
        resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
    };
    (tmp, ctx, task.id)
}

#[test]
fn explicit_plan_dispatch_enforces_read_only_without_git_candidate() {
    // Given a repository and an inert agent that attempts a source write.
    let (tmp, ctx, id) = fixture();
    // When the real dispatcher launches it inside plan mode.
    let outcome = corral_ops::dispatch(
        &ctx,
        &id,
        &DispatchOpts {
            plan: true,
            ..Default::default()
        },
    )
    .unwrap();
    // Then only plan history/output exist, and the kernel denied the source write.
    assert!(matches!(outcome, DispatchOutcome::Started { .. }));
    let task = ctx.store.get(&id).unwrap();
    assert!(task.runs.is_empty());
    assert!(task.launch.is_none());
    assert_eq!(task.plan_runs.len(), 1);
    assert_eq!(task.plan_runs[0].state, PlanRunState::Planning);
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("app/protected.txt")).unwrap(),
        "original"
    );
    assert_eq!(
        std::fs::read_to_string(task.plan_runs[0].output_dir.join("prompt.md.result")).unwrap(),
        "artifact"
    );
    assert!(!tmp.path().join("app/.worktrees").exists());
    assert!(!tmp.path().join("app/.git/refs/heads/corral").exists());
    assert_eq!(corral_ops::slot_usage(&ctx).unwrap().tasks, 1);
}

#[test]
fn required_and_design_tasks_route_to_planning_automatically() {
    for design in [true, false] {
        // Given either a design flag or a required-plan policy.
        let (_tmp, ctx, id) = fixture();
        ctx.store
            .update(&id, |t| {
                t.design = design;
                t.plan_required = (!design).then_some(true);
                Ok(())
            })
            .unwrap();
        // When dispatch has no explicit planning override.
        corral_ops::dispatch(&ctx, &id, &DispatchOpts::default()).unwrap();
        // Then it uses the planning path, never implementation.
        let task = ctx.store.get(&id).unwrap();
        assert!(task.runs.is_empty());
        assert_eq!(task.plan_runs.len(), 1);
    }
}

#[test]
fn queued_explicit_plan_preserves_mode_and_obeys_capacity() {
    // Given a full board and an explicit planning request.
    let (tmp, mut ctx, id) = fixture();
    ctx.store.config.limits.running = 1;
    ctx.store.save_config().unwrap();
    ctx.store
        .create("Busy", Some("app"), Status::Running, "")
        .unwrap();
    // When queue admission cannot grant a slot.
    let result = corral_ops::dispatch(
        &ctx,
        &id,
        &DispatchOpts {
            plan: true,
            queue_if_blocked: true,
            ..Default::default()
        },
    )
    .unwrap();
    // Then no agent/candidate is created and the queued mode stays pinned.
    assert!(matches!(result, DispatchOutcome::Queued { .. }));
    let task = ctx.store.get(&id).unwrap();
    assert!(task.queued);
    assert!(task.dispatch_plan.unwrap().plan);
    assert!(task.plan_runs.is_empty());
    assert!(task.runs.is_empty());
    assert!(!tmp.path().join("engine/launch-command").exists());
}

#[test]
fn resource_pressure_refuses_plan_dispatch_before_history_or_pane() {
    // Given pressure with the same admission policy used by implementation.
    let (tmp, mut ctx, id) = fixture();
    ctx.store.config.resources.max_load_per_core = 0.001;
    ctx.store.save_config().unwrap();
    // When a plan is dispatched without an owner force override.
    let result = corral_ops::dispatch(
        &ctx,
        &id,
        &DispatchOpts {
            plan: true,
            ..Default::default()
        },
    );
    // Then the request is refused without consuming a slot or creating a pane.
    assert!(result.is_err());
    assert!(ctx.store.get(&id).unwrap().plan_runs.is_empty());
    assert!(!tmp.path().join("engine/launch-command").exists());
}
