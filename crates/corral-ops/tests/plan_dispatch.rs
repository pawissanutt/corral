//! Exercise actual plan dispatch and its OS sandbox with an isolated engine.
#![cfg(target_os = "linux")] // This suite asserts bubblewrap kernel enforcement.
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
    std::fs::write(tmp.path().join("engine/execute"), "").unwrap();
    // When the real dispatcher launches it inside plan mode.
    let outcome = corral_ops::dispatch(
        &ctx,
        &id,
        &DispatchOpts {
            plan: true,
            ..Default::default()
        },
    )
    .unwrap_err();
    // Then only plan history/output exist, and the kernel denied the source write.
    assert!(outcome.to_string().contains("exited with status 0"));
    let task = ctx.store.get(&id).unwrap();
    assert!(task.runs.is_empty());
    assert!(task.launch.is_none());
    assert_eq!(task.plan_runs.len(), 1);
    assert_eq!(task.plan_runs[0].state, PlanRunState::Failed);
    assert_eq!(task.status, Status::NeedsMe);
    assert!(task.artifacts.iter().any(|a| a.run == "D1"));
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
    assert_eq!(corral_ops::slot_usage(&ctx).unwrap().tasks, 0);
}

#[test]
fn planner_crash_during_launch_captures_exit_and_releases_slot() {
    // Given an inert harness that emits the observed Bun crash and exits 134.
    let (tmp, mut ctx, id) = fixture();
    ctx.store.config.agents.get_mut("planner").unwrap().command =
        "printf 'panic(main thread): abort() called\\n' >&2; exit 134".into();
    ctx.store.save_config().unwrap();
    std::fs::write(tmp.path().join("engine/execute"), "").unwrap();
    // When it exits within the launch observation window.
    let error = corral_ops::dispatch(
        &ctx,
        &id,
        &DispatchOpts {
            plan: true,
            ..Default::default()
        },
    )
    .unwrap_err();
    // Then the command refuses success and records a terminal planner failure.
    assert!(error.to_string().contains("134"));
    let task = ctx.store.get(&id).unwrap();
    assert_eq!(task.status, Status::NeedsMe);
    assert_eq!(task.plan_runs[0].state, PlanRunState::Failed);
    assert!(task.plan_runs[0].ended.is_some());
    assert_eq!(corral_ops::slot_usage(&ctx).unwrap().total(), 0);
    let artifact = task
        .artifacts
        .iter()
        .find(|a| a.kind == corral_core::ArtifactKind::Log)
        .unwrap();
    let log = corral_ops::artifact_read(&ctx, artifact).unwrap();
    assert!(String::from_utf8_lossy(&log).contains("abort()"));
    assert!(!tmp.path().join("engine/planner-live").exists());
}

#[test]
fn planner_launch_records_exit_when_pane_shell_is_fish() {
    // Given a fish pane and an inert planner with an observable nonzero exit.
    let (tmp, mut ctx, id) = fixture();
    if support::command("fish", tmp.path())
        .args(["--no-config", "--version"])
        .stdin(Stdio::null())
        .output()
        .is_err()
    {
        eprintln!("skipping fish launch regression: fish is not installed");
        return;
    }
    ctx.store.config.agents.get_mut("planner").unwrap().command =
        "printf '%s\\n' 'planner'\\''s $HOME `uname` \\ marker'; exit 17".into();
    ctx.store.save_config().unwrap();
    for marker in ["execute", "execute-fish"] {
        std::fs::write(tmp.path().join("engine").join(marker), "").unwrap();
    }
    // When dispatch sends the actual sandbox and receipt wrapper to fish.
    let error = corral_ops::dispatch(
        &ctx,
        &id,
        &DispatchOpts {
            plan: true,
            ..Default::default()
        },
    )
    .unwrap_err();
    // Then the planner ran and its exit is recorded instead of a shell parse error.
    let output = std::fs::read_to_string(tmp.path().join("engine/output")).unwrap();
    assert!(
        error.to_string().contains("status 17"),
        "{error:#}\n{output}"
    );
    let task = ctx.store.get(&id).unwrap();
    assert_eq!(
        std::fs::read_to_string(task.plan_runs[0].output_dir.join("exit-0.status")).unwrap(),
        "17\n"
    );
    assert_eq!(task.plan_runs[0].state, PlanRunState::Failed);
    assert_eq!(task.status, Status::NeedsMe);
    assert_eq!(corral_ops::slot_usage(&ctx).unwrap().total(), 0);
    assert_eq!(output, "planner's $HOME `uname` \\ marker\n");
}

#[test]
fn planner_returning_to_shell_during_launch_fails_without_exit_receipt() {
    // Given a pane that reports only its shell after accepting the launch input.
    let (tmp, ctx, id) = fixture();
    std::fs::write(tmp.path().join("engine/shell-only"), "").unwrap();
    // When the launch window completes.
    assert!(
        corral_ops::dispatch(
            &ctx,
            &id,
            &DispatchOpts {
                plan: true,
                ..Default::default()
            }
        )
        .is_err()
    );
    // Then no apparently Running planner or held slot remains.
    let task = ctx.store.get(&id).unwrap();
    assert_eq!(task.plan_runs[0].state, PlanRunState::Failed);
    assert_eq!(task.status, Status::NeedsMe);
    assert_eq!(corral_ops::slot_usage(&ctx).unwrap().total(), 0);
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

#[test]
fn accepted_replacement_plan_updates_self_link_and_admits_implementation() {
    let (tmp, mut ctx, id) = fixture();
    let repo = tmp.path().join("app");
    assert!(
        support::command("git", &repo)
            .current_dir(&repo)
            .args(["add", "protected.txt"])
            .status()
            .unwrap()
            .success()
    );
    assert!(
        support::command("git", &repo)
            .current_dir(&repo)
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-qm",
                "Fixture"
            ])
            .status()
            .unwrap()
            .success()
    );
    ctx.store.config.agents.remove("planner");
    ctx.store.config.defaults.profile = Some("gpt-sol".into());
    let mut harness = ctx.store.config.harnesses["omo"].clone();
    harness.command = "true {model_args}".into();
    harness.flags = Some("generic".into());
    harness.model_flag = Some("--model".into());
    ctx.store.config.harnesses.insert("fixture".into(), harness);
    let mut profile = corral_core::Profile {
        harness: "fixture".into(),
        model: Some("fixture-planner".into()),
        vendor: Some("openai".into()),
        effort: None,
        args: Vec::new(),
        escalate_to: None,
    };
    ctx.store
        .config
        .profiles
        .insert("gpt-sol".into(), profile.clone());
    profile.model = Some("fixture-reviewer".into());
    ctx.store
        .config
        .profiles
        .insert("gpt-review".into(), profile);
    ctx.store.config.review.reviewer = Some("gpt-review".into());
    ctx.store.save_config().unwrap();
    ctx.store
        .update(&id, |task| {
            task.design = true;
            task.profile = Some("gpt-sol".into());
            Ok(())
        })
        .unwrap();

    for revision in ["P1", "P2"] {
        corral_ops::dispatch(
            &ctx,
            &id,
            &DispatchOpts {
                plan: true,
                profile: Some("gpt-sol".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let path = tmp.path().join(format!("{revision}.md"));
        std::fs::write(&path, format!("---\ngoal: Widget {revision}\nnon_goals: []\ndecisions: []\naffected_paths: [widget.rs]\ncontracts: []\ndependencies: []\nacceptance: [{{scenario: spin, expected: clockwise}}]\nverification: [check]\nowner_decisions: []\n---\n")).unwrap();
        let plan = corral_ops::plan_submit(&ctx, &id, &path).unwrap();
        assert_eq!(plan.id, revision);
        let attempt = corral_ops::plan_review(&ctx, &id, None).unwrap();
        for reviewer in [Some(attempt.id.as_str()), None] {
            corral_ops::plan_verdict(
                &ctx,
                &id,
                &corral_ops::PlanVerdict {
                    plan: revision,
                    attempt: reviewer,
                    verdict: corral_core::Verdict::Approve,
                    notes: "Reviewed contract",
                },
            )
            .unwrap();
        }
        assert_eq!(
            ctx.store.get(&id).unwrap().plan,
            Some(format!("{id}@{revision}"))
        );
    }
    let task = ctx.store.get(&id).unwrap();
    assert_eq!(task.plans.len(), 2);
    assert_eq!(task.plan_runs.len(), 2);
    assert!(task.plans.iter().all(|plan| plan.approval.is_some()));
    assert_eq!(
        corral_ops::approved_plan(&ctx, &task).unwrap().unwrap().id,
        "P2"
    );
    assert!(matches!(
        corral_ops::dispatch(&ctx, &id, &DispatchOpts::default()).unwrap(),
        DispatchOutcome::Started { .. }
    ));
    let implementation = ctx.store.get(&id).unwrap();
    assert_eq!(implementation.runs.len(), 1);
    assert_eq!(implementation.plan_runs.len(), 2);
}
