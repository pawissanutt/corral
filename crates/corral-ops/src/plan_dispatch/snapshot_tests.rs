use super::*;
use corral_core::{AgentCmd, ExecutionSnapshot, Store};
use serde_json::json;

#[cfg(unix)]
#[test]
fn planner_launch_consumes_persisted_snapshot_after_profile_deletion() {
    // Given a durable planner run prepared with a frozen execution configuration.
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("app");
    std::fs::create_dir(&repo).unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.projects.insert(
        "app".into(),
        corral_core::Project {
            path: "app".into(),
            dispatch: Default::default(),
            default_profile: None,
            base: None,
            review: Default::default(),
            artifact_retention: None,
            post_land: vec![],
        },
    );
    store.config.agents.insert(
        "planner-fixture".into(),
        AgentCmd {
            command: "original '{prompt_file}' '{task}'".into(),
        },
    );
    store.save_config().unwrap();
    let task = store
        .create("Plan", Some("app"), Status::Ready, "Spec")
        .unwrap();
    let resolved = store
        .config
        .resolve_profile(
            &ProfileChoice {
                profile: Some("planner-fixture".into()),
                ..Default::default()
            },
            None,
            None,
        )
        .unwrap();
    let mut snapshot = ExecutionSnapshot::new(&store.config, resolved);
    snapshot.instructions = "CONFIGURED_PLANNER_INSTRUCTIONS".into();
    let run: PlanRun = serde_json::from_value(json!({
        "id":"D1", "profile":"planner-fixture", "harness":"planner-fixture",
        "model":null, "vendor":null, "effort":null, "execution":snapshot,
        "route":corral_core::RouteSource::Override,
        "spec_revision":crate::plan_spec_revision(&task).unwrap(),
        "started":"2026-10-19T00:00:00Z",
        "output_dir":tmp.path().join("output"),"rounds":0,"state":"planning"
    }))
    .unwrap();
    let task = store
        .update(&task.id, |task| {
            task.plan_runs.push(run.clone());
            Ok(())
        })
        .unwrap();
    let engine = tmp.path().join("engine");
    std::fs::create_dir(&engine).unwrap();
    let bin = engine.join("herdr");
    std::os::unix::fs::symlink(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/plan_engine.sh"),
        &bin,
    )
    .unwrap();
    let mut ctx = crate::resources::FixedResourceProbe::ctx(store);
    ctx.herdr = corral_herdr::Herdr::with_bin(bin, Some("snapshot-fixture".into()));
    let claim = claim::Claim {
        run: run.id.clone(),
        attempt: "launch".into(),
        round: 0,
        previous_round: 0,
        plan: None,
        review: None,
        notes: None,
        force_resources: false,
        command: None,
        herdr: None,
    };
    crate::operating::authorize(
        &ctx,
        claim.identity(&task.id),
        crate::operating::Origin::Owner,
    )
    .unwrap();
    let prompt = run.output_dir.join("prompt.md");
    let inner = render_command(&ctx, &task, &run, &repo, &prompt).unwrap();
    let command = crate::plan_lifecycle::record_exit(
        &run,
        &crate::plan_sandbox::command(&repo, ctx.store.dir(), &inner),
    );
    // When the profile is deleted and the prepared launch is recovered after reopening storage.
    ctx.store.config.agents.remove("planner-fixture");
    ctx.store.save_config().unwrap();
    ctx.store = Store::open(tmp.path()).unwrap();
    let task = ctx.store.get(&task.id).unwrap();
    let (launched, _) = launch(&ctx, &task, &repo, claim).unwrap();
    // Then the actual submitted command, instructions, and identity use only the snapshot.
    assert_eq!(
        std::fs::read_to_string(engine.join("launch-command"))
            .unwrap()
            .trim_end(),
        command
    );
    let launched = launched.plan_runs.last().unwrap();
    assert_eq!(launched.id, run.id);
    assert_eq!(launched.execution, run.execution);
    assert_eq!(launched.route, run.route);
    let text = std::fs::read_to_string(prompt).unwrap();
    assert!(text.ends_with(&format!("{}\n", snapshot.instructions)));
    assert!(text.contains(&format!("`{}`", crate::skill::PLANNER_SKILL)));
}

#[test]
fn planner_execution_snapshot_survives_configuration_deletion_and_round_changes() {
    // Given a saved planner configuration with deliberately unset model and effort.
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.agents.insert(
        "planner-fixture".into(),
        AgentCmd {
            command: "original '{prompt_file}' '{task}'".into(),
        },
    );
    let task = store.create("Plan", None, Status::Ready, "Spec").unwrap();
    let resolved = store
        .config
        .resolve_profile(
            &ProfileChoice {
                profile: Some("planner-fixture".into()),
                ..Default::default()
            },
            None,
            None,
        )
        .unwrap();
    let run: PlanRun = serde_json::from_value(json!({
        "id":"D1", "profile":resolved.profile, "harness":resolved.harness,
        "model":null, "vendor":null, "effort":null,
        "execution":ExecutionSnapshot::new(&store.config, resolved),
        "route":corral_core::RouteSource::Override,
        "spec_revision":"spec-1","started":"2026-10-19T00:00:00Z",
        "output_dir":tmp.path().join("output"),"rounds":0,"state":"planning"
    }))
    .unwrap();
    let mut ctx = crate::resources::FixedResourceProbe::ctx(store);
    let file = run.output_dir.join("prompt.md");
    let command = render_command(&ctx, &task, &run, tmp.path(), &file).unwrap();
    // When the binding is removed and a revision reuses the same persisted run.
    ctx.store.config.agents.remove("planner-fixture");
    let mut restored: PlanRun =
        serde_yaml_ng::from_str(&serde_yaml_ng::to_string(&run).unwrap()).unwrap();
    restored.rounds = 1;
    // Then rendering uses the original command without rebinding unset settings.
    assert_eq!(
        render_command(&ctx, &task, &restored, tmp.path(), &file).unwrap(),
        command
    );
    assert_eq!(restored.id, run.id);
    assert_eq!(restored.execution, run.execution);
    assert_eq!(restored.route, run.route);
    assert_eq!(restored.model, None);
    assert_eq!(restored.effort, None);
}

#[test]
fn legacy_planner_without_snapshot_remains_readable_and_uses_current_command() {
    // Given a planner record written before snapshots and route provenance.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store.create("Plan", None, Status::Ready, "Spec").unwrap();
    let run: PlanRun = serde_json::from_value(json!({
        "id":"D1", "profile":"omo", "harness":"omo",
        "model":null, "vendor":null, "effort":null,
        "spec_revision":"spec-1","started":"2026-10-19T00:00:00Z",
        "output_dir":tmp.path().join("output"),"rounds":0,"state":"planning"
    }))
    .unwrap();
    let mut ctx = crate::resources::FixedResourceProbe::ctx(store);
    // When the current harness command changes.
    ctx.store.config.harnesses.get_mut("omo").unwrap().command = "legacy-current '{task}'".into();
    let command = render_command(&ctx, &task, &run, tmp.path(), Path::new("prompt.md")).unwrap();
    // Then absent snapshot fields retain legacy behavior.
    assert_eq!(run.execution, None);
    assert_eq!(run.route, None);
    assert_eq!(command, format!("legacy-current '{}'", task.id));
}
