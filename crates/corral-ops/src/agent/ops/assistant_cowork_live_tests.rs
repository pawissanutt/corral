//! Setup for the private-server co-work scenario (T-544 C9 and C10): project `corral` with a
//! passing gate and owner acceptance, assistant A-1 opened in the test's private pane, and two
//! of its scratch worktrees with one commit each: `cache-fix` to finish and `spike-idea` to
//! offer from the shell and promote.
use super::*;
use corral_core::{Accept, Store};
use serde_json::{Value, json};
use std::path::Path;

fn commit(dir: &Path, file: &str, message: &str) {
    std::fs::write(dir.join(file), format!("{message}\n")).unwrap();
    for args in [&["add", file][..], &["commit", "-q", "-m", message]] {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?}: {output:?}");
    }
}

#[test]
#[ignore = "driven by the private-server co-work live test"]
fn cowork_live_setup() {
    let Ok(input) = std::env::var("CORRAL_COWORK_LIVE") else {
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
    store.config.assistant.agent = Some("owner".into());
    store.config.profiles.insert(
        "owner".into(),
        serde_json::from_value(
            json!({"harness":"pi-native", "model":"gpt-6.1-sol", "vendor":"openai"}),
        )
        .unwrap(),
    );
    store.config.review.accept = Some(Accept::Owner);
    store.config.review.gate = Some(vec!["true".into()]);
    store.config.resources.max_load_per_core = 1.0e6;
    store.config.projects.insert(
        "corral".into(),
        serde_json::from_value(json!({"path":"app"})).unwrap(),
    );
    store.save_config().unwrap();
    let ctx = Ctx::from_store(Store::discover(root).unwrap());
    let (session, prepared, _) = open(
        &ctx,
        input["pane"].as_str().unwrap(),
        &Overrides::default(),
        false,
    )
    .unwrap();
    let mut scratch = Vec::new();
    for (slug, file, message) in [
        ("cache-fix", "cache.txt", "Fix the cache"),
        ("spike-idea", "spike.txt", "Spike an idea"),
    ] {
        let made = crate::assistant::scratch::create(&ctx, &session.id, "corral", Some(slug), None)
            .unwrap();
        commit(&made.path, file, message);
        scratch.push(made.path);
    }
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
    let launch = root.join("cowork-launch.json");
    std::fs::write(
        &launch,
        crate::agent::launch::launch_message(&bootstrap, &ready, &secrets).unwrap(),
    )
    .unwrap();
    println!(
        "COWORK_LIVE {}",
        json!({"session": session.id, "bootstrap": prepared.bootstrap, "launch": launch,
            "scratch": scratch})
    );
}
