//! A native lifecycle append must not erase the container's canonical financial binding.
use corral_core::{
    AgentHostHead, Status, Store,
    agent::{AgentRunState, BudgetRequest, ContainerRef, NativeRecord, paths},
    connection::{ConnectionBinding, ConnectionId},
    paid_episode::{PaidApproval, PaidBinding, PaidRoot},
};
use serde_json::json;

#[test]
fn paid_episode_survives_native_head_append_and_store_reload() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let task = store
        .create("Paid lifecycle", Some("app"), Status::Running, "")
        .unwrap();
    let owner = PaidBinding {
        container: ContainerRef::Worker {
            task: task.id.clone(),
            run: "R1".into(),
        },
        generation: 1,
        binding: ConnectionBinding::new(
            ConnectionId::OpenaiApi,
            Some("approved".into()),
            "fixture".into(),
            None,
        )
        .unwrap(),
    };
    let mut state = AgentRunState::new(
        serde_json::from_value(json!({
            "role":"worker", "store":paths::store_id(&store), "project":"app",
            "generation":1, "health":"healthy"
        }))
        .unwrap(),
    );
    // Legacy heads omit the optional link and must remain readable as unbound.
    let doc = serde_json::to_value(AgentHostHead::of(&state)).unwrap();
    assert!(doc.get("paid_episode").is_none());
    assert_eq!(
        serde_json::from_value::<AgentHostHead>(doc)
            .unwrap()
            .paid_episode,
        None
    );
    state.generations.push(
        serde_json::from_value(json!({
            "n":1, "bind_nonce_sha256":"00", "pane":"fixture", "created":"2026-11-01T00:00:00Z",
            "connection":owner.binding,
            "bound":{"host_cap_sha256":"00", "ctl_cap_sha256":"00", "endpoint":"/fixture.sock",
                "session_id":"fixture", "session_file":"/fixture.jsonl", "pid":1,
                "start_time":"fixture", "request_id":"hello", "at":"2026-11-01T00:00:00Z"}
        }))
        .unwrap(),
    );
    store
        .update(&task.id, |task| {
            task.runs.push(serde_json::from_value(json!({
                "id":"R1", "agent":"worker", "worktree":"/fixture", "branch":"fixture",
                "base":"main", "base_sha":"00", "started":"2026-11-01T00:00:00Z",
                "agent_host":state
            }))?);
            Ok(())
        })
        .unwrap();
    let link = store
        .create_paid_episode(
            PaidRoot {
                store: paths::store_id(&store),
                role: owner.role(),
                subject: task.id.clone(),
                attempt: "R1".into(),
            },
            PaidApproval {
                policy_digest: "11".repeat(32),
                approval: "fixture-approval".into(),
                allowance_nanos: 100,
                candidates: vec![owner.binding.clone()],
            },
            owner.clone(),
        )
        .unwrap();
    store
        .update(&task.id, |task| {
            let mut episode = store.paid_episode(&link.episode)?;
            episode.activate(&link, &owner, task.runs[0].agent_host.as_mut().unwrap())?;
            store.save_paid_episode(&episode)
        })
        .unwrap();
    store
        .update_head(&task.id, |head| {
            head.put_agent_record(
                &owner.container,
                NativeRecord::Turn(serde_json::from_value(json!({
                    "generation":1, "seq":1, "origin":{"kind":"bridge"},
                    "started":"2026-11-01T00:00:01Z"
                }))?),
            )
            .map(drop)
        })
        .unwrap();
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    let full = store.get(&task.id).unwrap();
    let state = full.runs[0].agent_host.as_ref().unwrap();
    assert_eq!(state.paid_episode.as_ref(), Some(&link));
    assert_eq!(state.turns.len(), 1);
    let projected = store
        .head(&task.id)
        .unwrap()
        .current_agent_state()
        .unwrap()
        .unwrap();
    assert_eq!(projected.paid_episode.as_ref(), Some(&link));
    store
        .records_transaction(|| {
            let saved = projected.paid_episode.as_ref().unwrap();
            let mut episode = store.paid_episode(&saved.episode)?;
            assert!(episode.reserve(
                saved,
                &owner,
                BudgetRequest {
                    connection: Some(ConnectionId::OpenaiApi),
                    request: "after-turn".into(),
                    generation: 1,
                    tokens: 1,
                    usd_nanos: 40,
                    priced: true,
                    unbounded: false,
                    response: None,
                },
            )?);
            store.save_paid_episode(&episode)
        })
        .unwrap();
    assert_eq!(
        store.paid_episode(&link.episode).unwrap().ledger.totals(),
        (1, 40)
    );
}
