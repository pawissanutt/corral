//! Immutable execution across a failed launch, config edits, and store restart.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::Stdio;

use corral_core::{Harness, Profile, Status, Store};
use corral_ops::{Ctx, DispatchOpts};
use serde_json::json;

mod support;

fn git(repo: &Path, args: &[&str]) {
    let output = support::command("git", repo)
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

fn fixture() -> (tempfile::TempDir, Ctx, String, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("app");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "fixture",
        ],
    );
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.harnesses.insert(
        "frozen".into(),
        Harness {
            command: "original {model_args} '{prompt_file}' '{name}'".into(),
            flags: Some("omo".into()),
            model_flag: None,
            effort_flag: None,
            efforts: vec![],
            prompt: None,
        },
    );
    store.config.profiles.insert("worker".into(), Profile {
        harness: "frozen".into(),
        model: Some("openai/exact-v1".into()), vendor: Some("openai".into()),
        effort: Some("high".into()), args: vec!["--old-argument".into()],
        escalate_to: None,
        metadata: Some(serde_json::from_value(json!({
            "display_name":"Worker", "purpose":"Implementation",
            "guidance":{"use_when":["Implement bounded work"]},
            "instructions":"FROZEN_CONFIG_INSTRUCTIONS",
            "eligibility":{"enabled":true,"automatic":true,"roles":["implementation","planning"]},
            "revision":"binding-1","evidence":"evidence-1"
        })).unwrap()),
    });
    store.save_config().unwrap();
    std::fs::create_dir_all(store.prompts_dir()).unwrap();
    std::fs::write(store.prompts_dir().join("run.md"), "{{id}}\n{{body}}\n").unwrap();
    let engine = tmp.path().join("engine");
    std::fs::create_dir(&engine).unwrap();
    let bin = engine.join("herdr");
    std::os::unix::fs::symlink(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshot_engine.sh"),
        &bin,
    )
    .unwrap();
    let task = store
        .create("Snapshot", Some("app"), Status::Ready, "Spec")
        .unwrap();
    let ctx = Ctx {
        store,
        herdr: corral_herdr::Herdr::with_bin(bin, Some("snapshot-fixture".into())),
        resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
    };
    (tmp, ctx, task.id, engine)
}

fn reserve(ctx: &Ctx, id: &str, engine: &Path) -> corral_core::Task {
    std::fs::write(engine.join("fail"), "").unwrap();
    assert!(
        corral_ops::dispatch(
            ctx,
            id,
            &DispatchOpts {
                profile: Some("worker".into()),
                ..Default::default()
            }
        )
        .is_err()
    );
    let task = ctx.store.get(id).unwrap();
    assert!(task.launch.is_some());
    assert_eq!(task.runs.len(), 1);
    task
}

#[test]
fn execution_snapshot_survives_binding_changes_deletion_and_launch_resume() {
    // Given a persisted run whose launch stopped before pane creation.
    let (tmp, mut ctx, id, engine) = fixture();
    let task = reserve(&ctx, &id, &engine);
    let run = task.current_run().unwrap().clone();
    let prompt = ctx.store.prompts_dir().join(format!("{id}-R1.md"));
    let command = corral_ops::render_command(&ctx, &run.agent, &task, &run, &prompt).unwrap();
    let snapshot = run.execution.as_ref().unwrap();
    assert_eq!(snapshot.revision.as_deref(), Some("binding-1"));
    assert_eq!(snapshot.evidence.as_deref(), Some("evidence-1"));
    let old_prompt = corral_ops::task_prompt(&ctx, &task, &run).unwrap();
    assert!(old_prompt.ends_with(&format!("{}\n", snapshot.instructions)));
    // When both profile binding and harness configuration change, then disappear.
    let profile = ctx.store.config.profiles.get_mut("worker").unwrap();
    profile.harness = "omo".into();
    profile.args = vec!["--new-argument".into()];
    profile.model = Some("anthropic/new-v2".into());
    profile.metadata.as_mut().unwrap().instructions = "NEW_CONFIG_INSTRUCTIONS".into();
    ctx.store
        .config
        .harnesses
        .get_mut("frozen")
        .unwrap()
        .command = "new {model_args}".into();
    let reserved = corral_ops::route(&ctx, &id, &corral_core::ProfileChoice::default())
        .unwrap()
        .implementer;
    assert_eq!(reserved, task.dispatch_plan.as_ref().unwrap().route);
    std::fs::write(ctx.store.prompts_dir().join("run.md"), "NEW_TEMPLATE").unwrap();
    assert_eq!(
        corral_ops::render_command(&ctx, &run.agent, &task, &run, &prompt).unwrap(),
        command
    );
    assert_eq!(
        corral_ops::task_prompt(&ctx, &task, &run).unwrap(),
        old_prompt
    );
    let future = ctx.store.config.profiles["worker"].clone();
    ctx.store
        .config
        .profiles
        .insert("future-worker".into(), future);
    ctx.store.save_config().unwrap();
    let pinned = corral_ops::implementer_set(&ctx, &id, "future-worker").unwrap();
    assert_eq!(pinned.dispatch_plan, task.dispatch_plan);
    assert_eq!(pinned.current_run().unwrap().execution, run.execution);
    ctx.store.config.profiles.remove("worker");
    ctx.store.config.harnesses.remove("frozen");
    ctx.store.save_config().unwrap();
    std::fs::remove_file(ctx.store.prompts_dir().join("run.md")).unwrap();
    ctx.store = Store::open(tmp.path()).unwrap();
    std::fs::remove_file(engine.join("fail")).unwrap();
    corral_ops::dispatch(&ctx, &id, &DispatchOpts::default()).unwrap();
    // Then the original command and run identity survive, including unset overrides.
    assert_eq!(
        std::fs::read_to_string(engine.join("command")).unwrap(),
        command
    );
    let resumed = ctx.store.get(&id).unwrap();
    assert_eq!(resumed.runs.len(), 1);
    let resumed = resumed.current_run().unwrap();
    assert_eq!(resumed.id, run.id);
    assert_eq!(resumed.branch, run.branch);
    assert_eq!(resumed.worktree, run.worktree);
    assert_eq!(resumed.execution, run.execution);
    assert_eq!(resumed.vendor, run.vendor);
    assert_eq!(resumed.model, run.model);
    assert_eq!(resumed.effort, run.effort);
    assert_eq!(resumed.route, run.route);
}

#[test]
fn legacy_execution_keeps_current_profile_rendering_and_launch_validation() {
    // Given a legacy run with no execution snapshot.
    let (_tmp, mut ctx, id, engine) = fixture();
    let task = reserve(&ctx, &id, &engine);
    ctx.store
        .update(&id, |task| {
            task.current_run_mut().unwrap().execution = None;
            Ok(())
        })
        .unwrap();
    let task = ctx.store.get(&task.id).unwrap();
    let run = task.current_run().unwrap();
    let prompt = ctx.store.prompts_dir().join("legacy.md");
    let original = corral_ops::render_command(&ctx, &run.agent, &task, run, &prompt).unwrap();
    // When legacy bindings change and are later deleted.
    ctx.store.config.profiles.get_mut("worker").unwrap().args = vec!["--legacy-current".into()];
    let changed = corral_ops::render_command(&ctx, &run.agent, &task, run, &prompt).unwrap();
    ctx.store.config.profiles.remove("worker");
    ctx.store.save_config().unwrap();
    // Then legacy rendering follows current args and recovery rejects a missing profile.
    assert_ne!(changed, original);
    assert!(corral_ops::render_command(&ctx, &run.agent, &task, run, &prompt).is_err());
    assert!(corral_ops::dispatch(&ctx, &id, &DispatchOpts::default()).is_err());
    assert_eq!(ctx.store.get(&id).unwrap().runs.len(), 1);
}

#[test]
fn execution_snapshot_preserves_unknown_model_effort_and_vendor() {
    // Given a run launched without model, effort, or vendor overrides.
    let (_tmp, mut ctx, id, engine) = fixture();
    let profile = ctx.store.config.profiles.get_mut("worker").unwrap();
    profile.model = None;
    profile.effort = None;
    profile.vendor = None;
    ctx.store.save_config().unwrap();
    let task = reserve(&ctx, &id, &engine);
    let run = task.current_run().unwrap();
    let prompt = ctx.store.prompts_dir().join("prompt.md");
    let command = corral_ops::render_command(&ctx, &run.agent, &task, run, &prompt).unwrap();
    // When the profile gains explicit settings for future runs.
    let profile = ctx.store.config.profiles.get_mut("worker").unwrap();
    profile.model = Some("openai/new".into());
    profile.effort = Some("max".into());
    profile.vendor = Some("openai".into());
    // Then unset settings and unknown vendor remain unknown for the saved run.
    assert_eq!(
        corral_ops::render_command(&ctx, &run.agent, &task, run, &prompt).unwrap(),
        command
    );
    assert!(ctx.store.config.run_vendor(run).is_err());
}
