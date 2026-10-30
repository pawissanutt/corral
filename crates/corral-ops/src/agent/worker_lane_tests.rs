//! Store setup for the worker-role cross-lane fixture
//! (packages/corral-pi/src/roles/worker/cross-lane.test.ts): a Running task in project `corral`
//! whose R1 is a pi-native inbox run in the fixture's private herdr pane, a run prompt with a
//! Gate contract section, and generation 1 opened for the in-process host. It prints one
//! `WORKER_LANE <json>` line with the bind nonce the host proves at hello.
use std::path::PathBuf;

use corral_core::agent::paths;
use corral_core::{Status, Store};
use serde_json::{Value, json};

use crate::Ctx;
use crate::agent::{BindTarget, open_generation};

#[test]
#[ignore = "driven by packages/corral-pi/src/roles/worker/cross-lane.test.ts"]
fn worker_cross_lane_setup() {
    let Ok(spec) = std::env::var("CORRAL_WORKER_LANE") else {
        return;
    };
    let spec: Value = serde_json::from_str(&spec).unwrap();
    let text = |key: &str| spec[key].as_str().unwrap().to_owned();
    let root = PathBuf::from(text("root"));
    let mut store = Store::init(&root).unwrap();
    store.config.review.gate = Some(vec!["true".into()]);
    store.config.dispatch.paused = true;
    store.config.limits.count_gates = false;
    store.config.resources.max_load_per_core = 1.0e6;
    let project = serde_json::from_value(json!({"path": "app"})).unwrap();
    store.config.projects.insert("corral".into(), project);
    store.save_config().unwrap();
    let ctx = Ctx::from_store(Store::discover(&root).unwrap());
    let task = ctx
        .store
        .create("Pi worker lane", Some("corral"), Status::Running, "spec")
        .unwrap();
    let app = root.join("app");
    let base = crate::test_support::command("git", &app)
        .arg("-C")
        .arg(&app)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let run: corral_core::Run = serde_json::from_value(json!({
        "id": "R1", "agent": "pi", "harness": "pi-native", "delivery": "inbox", "worktree": app,
        "branch": "main", "base": "main",
        "base_sha": String::from_utf8_lossy(&base.stdout).trim(),
        "started": jiff::Timestamp::now(),
        "herdr": {"workspace_id": text("workspace"), "pane_id": text("pane"),
                  "socket": ctx.herdr.socket_path().unwrap()},
    }))
    .unwrap();
    ctx.store
        .update(&task.id, |t| {
            t.runs.push(run);
            Ok(())
        })
        .unwrap();
    let prompt = crate::agent::hooks::surfaces::prompt_file(&ctx, &task.id, "R1");
    std::fs::create_dir_all(prompt.parent().unwrap()).unwrap();
    let gate = "## Gate contract (authoritative)\n\n- Tier: full\n- Gate: call corral_gate once.\n";
    std::fs::write(&prompt, format!("# Task\n\nShip it.\n\n{gate}\n## Done\n")).unwrap();
    let section =
        crate::agent::hooks::surfaces::worker_gate_section(&ctx, &ctx.store.get(&task.id).unwrap())
            .unwrap()
            .unwrap();
    let target = BindTarget::Worker {
        task: task.id.clone(),
        run: "R1".into(),
    };
    let grant = open_generation(&ctx.store, &target, &text("pane")).unwrap();
    let line = json!({
        "task": task.id, "run": "R1", "generation": grant.generation, "nonce": grant.bind_nonce,
        "store": paths::store_id(&ctx.store), "project": "corral", "prompt_file": prompt,
        "contract_sha256": section.contract_sha256,
        "sessions": paths::container_sessions_dir(&ctx.store, &grant.container),
    });
    println!("WORKER_LANE {line}");
}
