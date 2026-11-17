//! Workspace PM scope regressions; all engines and credentials are disposable fixtures.
use super::{WORKSPACE, tests::fixture};
use crate::agent::{self, bridge::fixture as f, test_support as ts};
use corral_core::{AgentCmd, OperatingMode, OperatingSelection, Status};
use serde_json::json;

fn call(
    fx: &crate::agent::bridge::fixture::Fx,
    host: &ts::Host,
    op: &str,
    id: &str,
    payload: serde_json::Value,
) -> agent::CallResponse {
    agent::call(
        &fx.ctx,
        op,
        &ts::envelope(
            host,
            op,
            &agent::tool_request_id(&host.identity.session_id, id),
            payload,
            false,
        ),
    )
    .unwrap()
}

#[test]
fn configured_workspace_preflight_and_authenticated_operations_preserve_legacy_bindings() {
    let mut fx = fixture();
    let mut tasks = vec![fx.id.clone()];
    for project in ["second", "third"] {
        fx.ctx.store.config.projects.insert(
            project.into(),
            serde_json::from_value(json!({"path":"app"})).unwrap(),
        );
        fx.ctx.store.save_config().unwrap();
        tasks.push(
            fx.ctx
                .store
                .create(project, Some(project), Status::Ready, "fixture")
                .unwrap()
                .id,
        );
    }
    let preview = fx
        .ctx
        .store
        .preview_operating_policy(&OperatingSelection {
            tasks: tasks.iter().cloned().collect(),
            ..Default::default()
        })
        .unwrap();
    fx.ctx
        .store
        .confirm_operating_policy(&preview, "owner", jiff::Timestamp::now())
        .unwrap();
    // The same entry point used by `corral pi --pm`, including preflight.
    super::profile(&fx.ctx).unwrap();
    assert_eq!(super::project(&fx.ctx).unwrap(), WORKSPACE);
    let path = crate::pm::save_native_context(&fx.ctx).unwrap();
    let input: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(input["version"], 1);
    assert_eq!(input["workspace"], fx.root().to_str().unwrap());
    assert_eq!(
        input["projects"].as_object().unwrap().len(),
        fx.ctx.store.config.projects.len()
    );
    assert!(input["projects"].get("second").is_some());
    assert!(input["policy"]["authorization"].is_string());
    let process = agent::bridge::fixture::HostProc::spawn(true);
    let host = fx.bind(
        &agent::BindTarget::Pm {
            project: WORKSPACE.into(),
        },
        "pm-pane",
        &process,
    );
    let list = ts::committed(&call(&fx, &host, "read_list", "list", json!({})));
    assert_eq!(list.result["total"], 3);
    for task in &tasks {
        let receipt = ts::committed(&call(
            &fx,
            &host,
            "comment",
            task,
            json!({"task":task,"text":"workspace triage"}),
        ));
        assert_eq!(receipt.state, corral_core::agent::ReceiptState::Committed);
        assert_eq!(fx.ctx.store.get(task).unwrap().comments.len(), 1);
    }
    // Workspace conversation includes configured projects outside the automatic grant.
    fx.ctx.store.config.projects.insert(
        "outside".into(),
        serde_json::from_value(json!({"path":"app"})).unwrap(),
    );
    fx.ctx.store.save_config().unwrap();
    let outside = fx
        .ctx
        .store
        .create("outside", Some("outside"), Status::Ready, "")
        .unwrap();
    let list = ts::committed(&call(&fx, &host, "read_list", "expanded-list", json!({})));
    assert_eq!(list.result["total"], 4);
    ts::committed(&call(
        &fx,
        &host,
        "comment",
        "outside",
        json!({"task":outside.id,"text":"workspace triage"}),
    ));
    assert_eq!(fx.ctx.store.get(&outside.id).unwrap().comments.len(), 1);
    assert!(
        !fx.ctx
            .store
            .effective_operating_policy()
            .unwrap()
            .allows(&outside.id, corral_core::OperatingActionKind::Dispatch)
    );
    let legacy = fx.bind(
        &agent::BindTarget::Pm {
            project: "corral".into(),
        },
        "legacy-pm-pane",
        &process,
    );
    let list = ts::committed(&call(&fx, &legacy, "read_list", "legacy-list", json!({})));
    assert_eq!(list.result["total"], 1);
    assert_eq!(
        ts::code(&call(
            &fx,
            &legacy,
            "read_show",
            "legacy-outside",
            json!({"task":outside.id,"section":"spec"})
        )),
        Some("out_of_scope")
    );
}

/// Reuse the disposable worker repository and engine, without creating an operating grant.
fn grantless_fixture() -> f::Fx {
    let mut fx = f::fixture(false, 1);
    let config = &mut fx.ctx.store.config;
    config.pm.agent = "pm-pi".into();
    config.profiles.insert(
        "pm-pi".into(),
        serde_json::from_value(json!({
            "harness":"corral-pi","model":"gpt-fixture","vendor":"openai"
        }))
        .unwrap(),
    );
    let assets = config.agent.assets_dir.as_ref().unwrap();
    for module in ["roles/pm", "delivery", "auth"] {
        let dir = assets.join("src").join(module);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.ts"), "export {}").unwrap();
    }
    std::fs::write(
        assets.join("compat.json"),
        r#"{"verdict":"GO","memory_projection":1}"#,
    )
    .unwrap();
    config.agents.insert(
        "fake".into(),
        AgentCmd {
            command: "true".into(),
        },
    );
    config.projects.get_mut("corral").unwrap().default_profile = Some("fake".into());
    config.routing.rules.clear();
    config.dispatch.paused = false;
    fx.ctx.store.save_config().unwrap();
    fx
}

#[test]
fn assist_pm_launch_and_paused_relaunch_keep_the_conversation_without_granting_execution() {
    use corral_core::agent::{ContainerRef, paths, pm_state};
    use corral_core::inbox::Recipient;

    let fx = grantless_fixture();
    fx.ctx.store.pause_operating_policy(true).unwrap();
    let ready = fx
        .ctx
        .store
        .create(
            "Not delegated",
            Some("corral"),
            Status::Ready,
            "## Acceptance\nWorks.\n",
        )
        .unwrap();
    assert!(fx.ctx.store.operating_state().unwrap().approval.is_none());
    let initial_policy = fx.ctx.store.effective_operating_policy().unwrap();
    assert_eq!(initial_policy.mode, OperatingMode::Assist);
    assert!(initial_policy.paused);
    assert!(initial_policy.authorization.is_none());
    let initial_config = std::fs::read(fx.ctx.store.config_path()).unwrap();

    // Start the real PM service in paused Assist with no authorization or approval receipt.
    let pane = crate::pm::open_pm(&fx.ctx).unwrap();
    assert_eq!(fx.calls("pane run").len(), 1);
    let mut process = f::HostProc::spawn(true);
    let unbound = ts::Host {
        identity: serde_json::from_value(json!({
            "store":paths::store_id(&fx.ctx.store),"project":"","role":"pm",
            "session_id":f::SESSION,"generation":0,
        }))
        .unwrap(),
        host_cap: String::new(),
        ctl_cap: String::new(),
        nonce: String::new(),
        pane: pane.clone(),
        sessions: paths::container_sessions_dir(&fx.ctx.store, &ContainerRef::Pm),
    };
    let bound = fx.hello_next(&unbound, &Recipient::Pm, &process);
    f::write_session(&bound, &f::session(&[], &[]));
    ts::committed(&call(
        &fx,
        &bound,
        "comment",
        "before-relaunch",
        json!({"task":ready.id,"text":"continue this conversation"}),
    ));
    assert_eq!(
        std::fs::read(fx.ctx.store.config_path()).unwrap(),
        initial_config
    );

    // Paused automation does not prevent an actual successor from resuming that session.
    process.kill();
    let reopened = f::ctx_at(fx.root());
    assert_eq!(
        crate::pm::relaunch(&reopened, "host_lost", false).unwrap(),
        pane
    );
    assert_eq!(fx.calls("pane run").len(), 2);
    let dir = agent::launch::generation_dir(&fx.ctx.store, &Recipient::Pm, 2).unwrap();
    let bootstrap: agent::launch::Bootstrap =
        serde_json::from_slice(&std::fs::read(dir.join("bootstrap.json")).unwrap()).unwrap();
    assert_eq!(
        bootstrap.resume_session,
        Some(bound.sessions.join(format!("{}.jsonl", f::SESSION)))
    );
    assert!(!bootstrap.fresh_session);
    assert_eq!(bootstrap.project, WORKSPACE);
    let next_process = f::HostProc::spawn(true);
    let next = fx.hello_next(&bound, &Recipient::Pm, &next_process);
    let list = ts::committed(&call(&fx, &next, "read_list", "after-list", json!({})));
    assert_eq!(list.result["total"], 2);
    ts::committed(&call(
        &fx,
        &next,
        "comment",
        "after-relaunch",
        json!({"task":ready.id,"text":"same conversation after relaunch"}),
    ));
    assert_eq!(fx.ctx.store.get(&ready.id).unwrap().comments.len(), 2);
    let current = pm_state::load(&fx.ctx.store).unwrap().unwrap();
    assert_eq!(current.state.binding.generation, 2);
    assert!(current.state.generation(1).unwrap().fenced.is_some());
    assert_eq!(
        current
            .state
            .generation(2)
            .unwrap()
            .bound
            .as_ref()
            .unwrap()
            .session_id,
        f::SESSION
    );

    // Conversation availability is not automatic dispatch authorization.
    let before = fx.ctx.store.get(&ready.id).unwrap();
    let error = crate::dispatch::dispatch_automatic(
        &reopened,
        &ready.id,
        json!({"source":"ready"}),
        jiff::Timestamp::now(),
    )
    .unwrap_err();
    assert!(
        format!("{error:#}").contains("automatic Dispatch is not authorized"),
        "{error:#}"
    );
    assert_eq!(fx.ctx.store.get(&ready.id).unwrap(), before);
    assert_eq!(fx.calls("pane run").len(), 2);
    assert!(!fx.app().join(".worktrees").exists());
    assert_eq!(
        std::fs::read(fx.ctx.store.config_path()).unwrap(),
        initial_config
    );
    let policy = fx.ctx.store.effective_operating_policy().unwrap();
    assert_eq!(policy.mode, OperatingMode::Assist);
    assert!(policy.paused);
    assert!(policy.authorization.is_none());
    assert!(fx.ctx.store.operating_state().unwrap().approval.is_none());
}

#[test]
fn workspace_context_accepts_absent_stale_paused_and_empty_automation_scopes() {
    for variant in ["absent", "stale", "paused", "empty-workspace"] {
        let mut fx = ts::fx();
        fx.ctx.store.config.projects.insert(
            "corral".into(),
            serde_json::from_value(json!({"path":"."})).unwrap(),
        );
        fx.ctx.store.save_config().unwrap();
        if variant == "stale" {
            // Configuration alone cannot supply a matching canonical approval.
            fx.ctx.store.config.automation.mode = OperatingMode::Autopilot;
            fx.ctx.store.config.automation.authorization = Some("stale-grant".into());
            fx.ctx.store.save_config().unwrap();
        } else if variant == "paused" {
            fx.ctx.store.pause_operating_policy(true).unwrap();
        } else if variant == "empty-workspace" {
            fx.ctx.store.config.projects.clear();
            fx.ctx.store.save_config().unwrap();
        }
        let before = std::fs::read(fx.ctx.store.config_path()).unwrap();
        assert_eq!(super::project(&fx.ctx).unwrap(), WORKSPACE, "{variant}");
        let input: serde_json::Value = serde_json::from_slice(
            &std::fs::read(crate::pm::save_native_context(&fx.ctx).unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            input["projects"].as_object().unwrap().len(),
            usize::from(variant != "empty-workspace"),
            "{variant}"
        );
        assert!(input["policy"]["authorization"].is_null(), "{variant}");
        assert_eq!(input["policy"]["authority"]["dispatch"], false, "{variant}");
        assert!(fx.ctx.store.operating_state().unwrap().approval.is_none());
        assert_eq!(std::fs::read(fx.ctx.store.config_path()).unwrap(), before);
        assert!(
            corral_core::agent::pm_state::load(&fx.ctx.store)
                .unwrap()
                .is_none()
        );
    }
}
