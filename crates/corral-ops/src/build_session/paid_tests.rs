use super::*;
use corral_core::connection::ConnectionId;
use std::sync::{Arc, Barrier};

fn configured() -> (tempfile::TempDir, Store, ConnectionBinding) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::init(dir.path()).unwrap();
    let profile = serde_json::from_value(json!({
        "harness":"corral-pi", "vendor":"openai", "model":"gpt-5.4", "connection":"openai-api",
        "metadata": {"display_name":"fixture", "purpose":"fixture", "guidance":{}, "instructions":"fixture", "revision":"fixture",
            "eligibility":{"enabled":true,"automatic":false,"roles":["implementation"],"projects":[]}, "evidence":"synthetic",
            "candidates":[{"id":"exact-api","harness":"corral-pi","connection":"openai-api","provider":"openai","model":"gpt-5.4",
                "args":[],"evidence":"synthetic","cost":{"kind":"metered","currency":"USD","maximum_micros":1}}],
            "api_start":{"roles":["assistant"],"spending":{"currency":"USD","maximum_micros":1},"approval":"explicit-fixture"}}
    })).unwrap();
    store.config.profiles.insert("build-api".into(), profile);
    store.save_config().unwrap();
    let binding = ConnectionBinding::new(
        ConnectionId::OpenaiApi,
        Some("exact-api".into()),
        "gpt-5.4".into(),
        None,
    )
    .unwrap();
    (dir, store, binding)
}
fn call(launch: &PaidLaunch, id: &str, nanos: u64) -> Value {
    json!({"version":1,"session":launch.session,"generation":launch.generation,"cap":launch.cap,"kind":"budget","action":"reserve",
        "request":id,"tokens":10,"usd_nanos":nanos,"priced":true,"unbounded":false,"provider":"openai","model":"gpt-5.4"})
}

#[test]
fn derived_mixed_build_meters_frozen_routes_and_refuses_a_changed_preset_on_resume() {
    let (_dir, mut store, binding) = configured();
    let preset = corral_core::BuildProfileCfg {
        model: "openai-codex/gpt-5.4".into(),
        effort: None,
        description: None,
    };
    store
        .config
        .build
        .profiles
        .insert("hard".into(), preset.clone());
    store.save_config().unwrap();
    let presets = [("hard".into(), "build-api".into())].into();
    let routes = [("build-api".into(), binding.clone())].into();
    let launch = open_mixed(&store, "subscription", None, &presets, &routes, None).unwrap();
    let reserve = |id| {
        let mut input = call(&launch, id, 5_000);
        input["binding_digest"] = json!(binding.digest);
        input
    };
    let state = budget(&store, reserve("child")).unwrap();
    assert_eq!(state["admitted"], true);
    assert!(state["limits"]["max_usd_nanos"].is_null());

    // Renaming the preset leaves the exact paid binding unchanged, but changes the mapping.
    store.config.build.profiles.remove("hard");
    store.config.build.profiles.insert("heavy".into(), preset);
    store.save_config().unwrap();
    let changed = [("heavy".into(), "build-api".into())].into();
    let err = open_mixed(
        &store,
        "subscription",
        None,
        &changed,
        &routes,
        Some(&launch.session),
    )
    .unwrap_err();
    assert!(err.to_string().contains("route is sticky"), "{err:#}");
    let state = budget(&store, reserve("next-child")).unwrap();
    assert_eq!(state["paid"]["consumed_or_reserved_nanos"], 10_000);
}

#[test]
fn concurrent_reservations_are_recorded_and_allowance_edits_do_not_block_resume() {
    let (_dir, store, binding) = configured();
    let launch = open(&store, "build-api", &binding, None).unwrap();
    let root = store.root().to_path_buf();
    let barrier = Arc::new(Barrier::new(3));
    let workers: Vec<_> = ["parent", "child"]
        .into_iter()
        .map(|id| {
            let root = root.clone();
            let input = call(&launch, id, 600);
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let store = Store::open(&root).unwrap();
                barrier.wait();
                (
                    id,
                    budget(&store, input).unwrap()["admitted"]
                        .as_bool()
                        .unwrap(),
                )
            })
        })
        .collect();
    barrier.wait();
    let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    // Spending caps are disabled: both serialized reservations are admitted and recorded.
    assert!(results.iter().all(|(_, admitted)| *admitted));
    let mut settle = call(&launch, "parent", 100);
    settle["action"] = json!("settle");
    settle["response"] = json!("successful-response");
    settle["tokens"] = json!(2);
    budget(&store, settle.clone()).unwrap();
    budget(&store, settle).unwrap(); // durable idempotent settlement replay
    let reply = budget(&store, call(&launch, "uncertain-retry", 600)).unwrap();
    assert_eq!(reply["admitted"], true);
    assert_eq!(reply["exhausted"], false);
    assert_eq!(
        budget(&store, call(&launch, "small-request", 1)).unwrap()["admitted"],
        true
    );
    assert!(open(&store, "another-profile", &binding, Some(&launch.session)).is_err());
    // Editing the approved allowance keeps the session resumable with its recorded usage.
    let mut changed = Store::open(store.root()).unwrap();
    changed
        .config
        .profiles
        .get_mut("build-api")
        .unwrap()
        .metadata
        .as_mut()
        .unwrap()
        .api_start
        .as_mut()
        .unwrap()
        .spending
        .maximum_micros = 2;
    changed.save_config().unwrap();
    let resumed = open(&changed, "build-api", &binding, Some(&launch.session)).unwrap();
    assert_eq!(resumed.generation, launch.generation + 1);
    let ledger = changed.build_paid_ledger(&launch.session).unwrap().unwrap();
    assert_eq!(ledger.ledger.totals().1, 1301);
    assert_eq!(
        budget(&changed, call(&resumed, "after-resume", 400)).unwrap()["admitted"],
        true
    );
    assert!(budget(&changed, call(&launch, "fenced-generation", 1)).is_err());
}
