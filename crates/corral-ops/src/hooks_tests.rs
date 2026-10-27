use super::*;
use corral_core::{Status, Store};

#[test]
fn hook_intent_is_durable_before_the_landing_callback_commits() {
    // Given a project hook and a landing inside a task's unit of work.
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("app");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let mut store = Store::init(root.path()).unwrap();
    store.config.projects.get_mut("app").unwrap().post_land = vec![PostLandHook {
        name: "check".into(),
        argv: vec!["true".into()],
        cwd: ".".into(),
        env: Default::default(),
        timeout_secs: 10,
        max_attempts: 1,
        identity: None,
    }];
    store.save_config().unwrap();
    let ctx = crate::resources::FixedResourceProbe::ctx(store);
    let task = ctx
        .store
        .create("Landing", Some("app"), Status::Review, "")
        .unwrap();
    let run: Run = serde_json::from_value(serde_json::json!({
        "id": "R1", "agent": "generic", "worktree": repo, "branch": "fixture",
        "base": "main", "base_sha": "before", "started": "2026-10-26T00:00:00Z"
    }))
    .unwrap();
    // When the callback stops after preparing the independently committed Git effect.
    let result = ctx.store.update(&task.id, |task| {
        prepare(&ctx, task, &run, &repo, "before", "after")?.unwrap();
        anyhow::bail!("interrupted landing callback");
    });
    // Then restart still sees the exact intent and can reconcile Git before running hooks.
    assert!(result.is_err());
    let restarted = crate::resources::FixedResourceProbe::ctx(Store::open(root.path()).unwrap());
    let operations = post_land_operations(&restarted, Some("app")).unwrap();
    assert_eq!(operations.len(), 1);
    assert_eq!(operations[0].base_before, "before");
    assert_eq!(operations[0].landed_sha, "after");
    assert!(!operations[0].landed);
    assert_eq!(operations[0].hooks[0].state, HookState::Pending);
    assert_eq!(restarted.store.get(&task.id).unwrap(), task);
}
