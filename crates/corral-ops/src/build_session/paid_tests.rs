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
fn parent_child_reservations_are_serialized_and_resume_retains_uncertain_spending() {
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
    assert_eq!(results.iter().filter(|(_, admitted)| *admitted).count(), 1);
    let admitted = results.iter().find(|(_, admitted)| *admitted).unwrap().0;
    let mut settle = call(&launch, admitted, 100);
    settle["action"] = json!("settle");
    settle["response"] = json!("successful-response");
    settle["tokens"] = json!(2);
    budget(&store, settle.clone()).unwrap();
    budget(&store, settle).unwrap(); // durable idempotent settlement replay
    assert_eq!(
        budget(&store, call(&launch, "uncertain-retry", 600)).unwrap()["admitted"],
        true
    );
    assert_eq!(
        budget(&store, call(&launch, "second-physical-retry", 600)).unwrap()["admitted"],
        false
    );
    let resumed = open(&store, "build-api", &binding, Some(&launch.session)).unwrap();
    assert_eq!(resumed.generation, launch.generation + 1);
    assert_eq!(
        store
            .build_paid_ledger(&launch.session)
            .unwrap()
            .unwrap()
            .ledger
            .totals()
            .1,
        700
    );
    assert_eq!(
        budget(&store, call(&resumed, "after-resume", 400)).unwrap()["admitted"],
        false
    );
    assert!(budget(&store, call(&launch, "stale-generation", 1)).is_err());
    assert!(open(&store, "another-profile", &binding, Some(&launch.session)).is_err());
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
    assert!(open(&changed, "build-api", &binding, Some(&launch.session)).is_err());
    assert!(budget(&changed, call(&resumed, "revoked-policy", 1)).is_err());
}
