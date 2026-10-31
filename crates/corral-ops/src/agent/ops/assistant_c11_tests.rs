//! Setup for the private-server C11 host scenario. No pane input or gate operations.
use super::*;
use corral_core::agent::{AgentBinding, AgentRole, AgentRunState, HostHealth};
use corral_core::{Run, Status, Store};
use serde_json::{Value, json};
use std::path::Path;

fn source(id: &str, harness: &str, path: &Path, cwd: &Path) -> Run {
    serde_json::from_value(json!({
        "id":id, "agent":harness, "harness":harness, "worktree":cwd,
        "branch":"main", "base":"main", "base_sha":"fixture", "started":"2026-01-01T00:00:00Z",
        "session":{"policy":"fresh", "action":"new", "harness":harness, "directory":path.parent(), "transcript":path}
    })).unwrap()
}

fn native(path: &Path, role: AgentRole) -> Box<AgentRunState> {
    let mut state = AgentRunState::new(AgentBinding {
        role,
        store: "st-c11".into(),
        project: "corral".into(),
        generation: 1,
        health: HostHealth::Lost,
        callback: None,
        closing: false,
    });
    state.generations.push(serde_json::from_value(json!({
        "n":1,"bind_nonce_sha256":"fixture","pane":"fixture-source","created":"2026-01-01T00:00:00Z",
        "bound":{"host_cap_sha256":"fixture","ctl_cap_sha256":"fixture","session_id":"fixture",
            "session_file":path,"pid":1,"start_time":"fixture","request_id":"fixture","at":"2026-01-01T00:00:00Z"}
    })).unwrap());
    Box::new(state)
}

#[test]
#[ignore = "driven by the private-server assistant C11 cross-lane test"]
fn focused_assistant_c11_setup() {
    let Ok(input) = std::env::var("CORRAL_ASSISTANT_C11") else {
        return;
    };
    let input: Value = serde_json::from_str(&input).unwrap();
    assert_eq!(
        std::env::var("CORRAL_HERDR_SESSION").unwrap(),
        "corral-test"
    );
    let root = Path::new(input["root"].as_str().unwrap());
    let mut store = Store::init(root).unwrap();
    store.config.agent.assets_dir = Some(input["assets"].as_str().unwrap().into());
    store.config.assistant.agent = Some("c11".into());
    store.config.profiles.insert(
        "c11".into(),
        serde_json::from_value(
            json!({"harness":"pi-native", "model":"gpt-6.1-sol", "vendor":"openai"}),
        )
        .unwrap(),
    );
    store.config.resources.max_load_per_core = 1.0e6;
    store.config.projects.insert(
        "corral".into(),
        serde_json::from_value(json!({"path":"app"})).unwrap(),
    );
    store.save_config().unwrap();
    let ctx = Ctx::from_store(Store::discover(root).unwrap());
    let task = ctx
        .store
        .create(
            "C11 focused evidence",
            Some("corral"),
            Status::Running,
            &format!(
                "C11_BRIEF_SENTINEL: inspect transcripts without changing task authority.\n{}",
                "界".repeat(20000)
            ),
        )
        .unwrap();
    let cwd = root.join("app");
    let paths: Vec<_> = ["omo", "claude", "pi", "review", "missing"]
        .into_iter()
        .map(|label| root.join(format!("{label}.jsonl")))
        .collect();
    for (label, path) in ["OMO_C11", "CLAUDE_C11", "PI_C11", "REVIEW_C11"]
        .into_iter()
        .zip(&paths)
    {
        let text =
            json!({"type":"message", "cwd":cwd, "timestamp":"2026-10-01T00:00:00Z", "text":label})
                .to_string()
                + "\n";
        std::fs::write(path, text.repeat(250)).unwrap();
    }
    ctx.store
        .update(&task.id, |task| {
            task.runs.push(source("R1", "omo", &paths[0], &cwd));
            task.runs.push(source("R2", "claude", &paths[1], &cwd));
            let mut pi = source("R3", "pi-native", &paths[2], &cwd);
            pi.agent_host = Some(native(&paths[2], AgentRole::Worker));
            let mut review = source("R3-review1", "pi-native", &paths[3], &cwd);
            review.agent_host = Some(native(&paths[3], AgentRole::Reviewer));
            pi.review_runs.push(review);
            task.runs.push(pi);
            task.runs.push(source("R4", "omo", &paths[4], &cwd));
            Ok(())
        })
        .unwrap();
    let (session, prepared, _) = open_focused(
        &ctx,
        input["pane"].as_str().unwrap(),
        &Overrides::default(),
        false,
        Some(&task.id),
    )
    .unwrap();
    let bootstrap: crate::agent::launch::Bootstrap =
        serde_json::from_slice(&std::fs::read(&prepared.bootstrap).unwrap()).unwrap();
    let ready = crate::agent::launch::check(
        &bootstrap,
        prepared.bootstrap.parent().unwrap(),
        prepared.assets.as_deref(),
    )
    .unwrap();
    let secrets = crate::agent::launch::Secrets {
        host_cap: "e".repeat(64),
        ctl_cap: "d".repeat(64),
    };
    let launch = root.join("c11-launch.json");
    std::fs::write(
        &launch,
        crate::agent::launch::launch_message(&bootstrap, &ready, &secrets).unwrap(),
    )
    .unwrap();
    println!(
        "ASSISTANT_C11 {}",
        json!({"task":task.id, "session":session.id, "bootstrap":prepared.bootstrap, "launch":launch, "task_path":task.path, "missing":paths[4]})
    );
}
