use super::*;
use corral_core::memory::*;
use serde_json::json;

fn put(store: &Store, scope: MemoryScope, id: &str) {
    let selector = MemorySelector {
        scope: scope.clone(),
        id: id.into(),
    };
    let entry: MemoryEntry = serde_json::from_value(json!({
        "metadata": { "version":1, "id":id, "scope":scope, "title":id,
            "kind":"guidance", "summary":format!("UNCOMMITTED-{id}"),
            "project_into_prompt":true,
            "provenance":{"kind":"imported_reference","sources":["fixture:source"]},
            "updated_at":"2026-01-01T00:00:00Z","author":"placeholder" },
        "body":"Detail stays on demand."
    }))
    .unwrap();
    store
        .memory_put(&selector, entry, None, "fixture", jiff::Timestamp::now())
        .unwrap();
}

#[test]
fn memory_launch_scope_and_fresh_resume_use_working_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.projects.insert(
        "demo".into(),
        serde_json::from_value(json!({"path":tmp.path()})).unwrap(),
    );
    for (scope, id) in [
        (MemoryScope::Workspace, "workspace"),
        (MemoryScope::Role(MemoryRole::Pm), "pm"),
        (MemoryScope::Role(MemoryRole::Assistant), "assistant"),
        (MemoryScope::Role(MemoryRole::Worker), "worker"),
        (MemoryScope::Project("demo".into()), "demo"),
        (MemoryScope::Project("other".into()), "other"),
    ] {
        put(&store, scope, id);
    }
    for (agent_role, own) in [
        (AgentRole::Pm, "pm"),
        (AgentRole::Assistant, "assistant"),
        (AgentRole::Worker, "worker"),
    ] {
        let text = projection(&store, agent_role, "demo").unwrap().unwrap();
        assert!(text.len() <= 12288);
        assert_eq!(text.matches("# Retained memory").count(), 1);
        assert!(text.contains("UNCOMMITTED-workspace"));
        assert!(text.contains(&format!("UNCOMMITTED-{own}")));
        assert!(text.contains("Provenance: imported_reference"));
        assert!(text.contains("fixture:source"));
        assert_eq!(text.contains("UNCOMMITTED-demo"), own == "worker");
        assert!(!text.contains("UNCOMMITTED-other"));
        for other in ["pm", "assistant", "worker"] {
            assert_eq!(text.contains(&format!("UNCOMMITTED-{other}")), other == own);
        }
    }
    put(&store, MemoryScope::Workspace, "later");
    assert!(
        projection(&store, AgentRole::Worker, "demo")
            .unwrap()
            .unwrap()
            .contains("UNCOMMITTED-later")
    );
    store.config.agent.capabilities.set_trusted_projects(vec![]);
    assert!(
        !projection(&store, AgentRole::Worker, "demo")
            .unwrap()
            .unwrap()
            .contains("UNCOMMITTED-demo")
    );
    for role in [
        AgentRole::Reviewer,
        AgentRole::Planner,
        AgentRole::PlanReviewer,
    ] {
        assert!(projection(&store, role, "demo").unwrap().is_none());
    }
}

#[test]
fn memory_dispatch_writes_private_generation_file_and_launch_reference() {
    use crate::agent::{BindTarget, launch};
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    let assets = tmp.path().join("assets");
    std::fs::create_dir(&assets).unwrap();
    std::fs::write(
        assets.join("compat.json"),
        r#"{"version":1,"verdict":"GO","bun":{"range":"1.4.2"},"memory_projection":1}"#,
    )
    .unwrap();
    store.config.agent.assets_dir = Some(assets.clone());
    store.config.projects.insert(
        "demo".into(),
        serde_json::from_value(json!({"path":tmp.path()})).unwrap(),
    );
    // prepare uses the supplied Store; the only bytes projected are its working tree.
    put(&store, MemoryScope::Workspace, "dispatch");
    let task = store
        .create("fixture", None, corral_core::Status::Running, "")
        .unwrap();
    store
        .update(&task.id, |task| {
            task.project = Some("demo".into());
            task.runs.push(crate::agent::test_support::run("R1", "p1"));
            Ok(())
        })
        .unwrap();
    let ctx = crate::Ctx::from_store(store);
    let target = BindTarget::Worker {
        task: task.id.clone(),
        run: "R1".into(),
    };
    let prepared = launch::prepare_bootstrap(
        &ctx,
        &target,
        "p1",
        &launch::HostSpec {
            cwd: tmp.path().into(),
            model: "fixture".into(),
            provider: None,
            effort: None,
            prompt_file: None,
            adopt: false,
        },
    )
    .unwrap();
    let dir = prepared.bootstrap.parent().unwrap();
    let bootstrap: launch::Bootstrap =
        serde_json::from_slice(&std::fs::read(&prepared.bootstrap).unwrap()).unwrap();
    let text = std::fs::read_to_string(dir.join(FILE)).unwrap();
    assert!(text.contains("UNCOMMITTED-dispatch"));
    let ready = launch::Ready {
        assets,
        bun: "/synthetic/bun".into(),
        bun_version: "1.4.2".into(),
        generation_dir: dir.into(),
    };
    let message: serde_json::Value = serde_json::from_slice(
        &launch::launch_message(
            &bootstrap,
            &ready,
            &launch::Secrets {
                host_cap: "h".repeat(64),
                ctl_cap: "c".repeat(64),
            },
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(message["memory_file"], dir.join(FILE).to_str().unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(dir.join(FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
}

#[test]
fn memory_host_compatibility_fails_closed() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("compat.json"), "{}").unwrap();
    assert!(compatible(tmp.path(), AgentRole::Worker).is_err());
    assert!(compatible(tmp.path(), AgentRole::Reviewer).is_ok());
    std::fs::write(tmp.path().join("compat.json"), r#"{"memory_projection":1}"#).unwrap();
    assert!(compatible(tmp.path(), AgentRole::Worker).is_ok());
}
