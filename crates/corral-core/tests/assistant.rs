use std::sync::{Arc, Barrier};

use corral_core::{
    Actor, Store,
    agent::{AgentGeneration, AgentRole, GenerationBound, HostHealth},
};

#[test]
fn sessions_and_containers_round_trip_and_end_retains_transcripts() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    store
        .set_attribution(Actor::Assistant, "assistant:A-1")
        .unwrap();
    let first = store
        .create_assistant("pi", None, Some("pane-1"), Some("/tmp/socket".into()))
        .unwrap();
    let second = store
        .create_assistant("pi", Some("T-42"), Some("pane-2"), None)
        .unwrap();
    assert_eq!((&*first.id, &*second.id), ("A-1", "A-2"));
    assert!(
        store.list().unwrap().is_empty(),
        "no synthetic tasks or coding slots"
    );
    assert_eq!(
        store.list_assistants().unwrap(),
        vec![first.clone(), second.clone()]
    );
    assert!(store.assistant_path("A-1").unwrap().is_file());
    assert!(store.assistant_container_path("A-1").unwrap().is_file());
    let transcript = tmp.path().join("transcript.jsonl");
    std::fs::write(&transcript, "kept transcript").unwrap();
    store
        .update_assistant_container("A-1", |container| {
            assert_eq!(container.state.binding.role, AgentRole::Assistant);
            assert_eq!(
                container.herdr_socket.as_deref(),
                Some(std::path::Path::new("/tmp/socket"))
            );
            container
                .state
                .wakes
                .push(corral_core::agent::WakeSubscription {
                    key: "pending".into(),
                    request_id: "wait".into(),
                    kind: corral_core::agent::WakeKind::Wait,
                    target: "wait-1".into(),
                    state: corral_core::agent::WakeState::Armed,
                    at: first.created,
                    fired_at: None,
                    message_id: None,
                });
            container.state.binding.generation = 1;
            container.state.binding.health = HostHealth::Healthy;
            container.state.generations.push(AgentGeneration {
                n: 1,
                bind_nonce_sha256: "digest".into(),
                pane: "pane-1".into(),
                created: first.created,
                bound: Some(GenerationBound {
                    host_cap_sha256: "host".into(),
                    ctl_cap_sha256: "ctl".into(),
                    session_id: "sid".into(),
                    session_file: transcript.clone(),
                    pid: 123,
                    start_time: "start".into(),
                    request_id: "hello".into(),
                    at: first.created,
                    herdr_seq_epoch: None,
                }),
                fenced: None,
                nudged_at: None,
                event_seq: 0,
                todo: false,
                capabilities: Some(
                    store
                        .config
                        .agent
                        .capabilities
                        .resolve(AgentRole::Assistant, None),
                ),
                capability_transition: None,
            });
            Ok(())
        })
        .unwrap();
    store
        .update_assistant("A-1", |session| {
            session.summary = "Research result".into();
            session.body = "# Research result\n\nDetailed notes.\n".into();
            Ok(())
        })
        .unwrap();
    let ended = store.end_assistant("A-1").unwrap();
    assert!(ended.ended.is_some());
    assert_eq!(store.end_assistant("A-1").unwrap(), ended);
    assert!(store.get_assistant("A-2").unwrap().ended.is_none());
    let reopened = Store::open(tmp.path()).unwrap();
    assert_eq!(reopened.get_assistant("A-1").unwrap(), ended);
    let container = reopened.get_assistant_container("A-1").unwrap();
    assert!(container.state.binding.closing);
    assert_eq!(container.state.binding.health, HostHealth::Fenced);
    assert!(container.state.generations[0].fenced.is_some());
    assert_eq!(
        container.state.wakes[0].state,
        corral_core::agent::WakeState::Cancelled
    );
    assert_eq!(
        container.state.generations[0]
            .bound
            .as_ref()
            .unwrap()
            .session_file,
        transcript
    );
    assert_eq!(
        std::fs::read_to_string(transcript).unwrap(),
        "kept transcript"
    );
    assert!(
        reopened
            .update_assistant_container("A-1", |_| Ok(()))
            .is_err()
    );
    assert_eq!(
        store
            .events()
            .unwrap()
            .iter()
            .filter(|e| e.kind == "assistant_ended")
            .count(),
        1
    );
    let event = store
        .events()
        .unwrap()
        .into_iter()
        .find(|e| e.kind == "assistant_created")
        .unwrap();
    assert_eq!(event.actor, Some(Actor::Assistant));
    assert_eq!(event.via.as_deref(), Some("assistant:A-1"));
}

#[test]
fn concurrent_allocations_use_the_board_lock_and_durable_counter() {
    let tmp = tempfile::tempdir().unwrap();
    Store::init(tmp.path()).unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let root = tmp.path().to_owned();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let store = Store::open(&root).unwrap();
                barrier.wait();
                store.create_assistant("pi", None, None, None).unwrap().id
            })
        })
        .collect();
    barrier.wait();
    let mut ids: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    ids.sort();
    assert_eq!(ids, ["A-1", "A-2"]);
    let store = Store::open(tmp.path()).unwrap();
    assert_eq!(store.list_assistants().unwrap().len(), 2);
    assert_eq!(
        store.create_assistant("pi", None, None, None).unwrap().id,
        "A-3"
    );
}

#[test]
fn invalid_ids_never_escape_the_assistant_roots() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    for id in [
        "../A-1",
        "A-0",
        "A-01",
        "A-+1",
        "A-1/../../evil",
        "T-1",
        "A-18446744073709551616",
    ] {
        assert!(store.assistant_path(id).is_err(), "{id}");
        assert!(store.assistant_container_path(id).is_err(), "{id}");
        assert!(store.get_assistant(id).is_err(), "{id}");
    }
}

#[test]
fn interrupted_reservation_is_not_reused() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    std::fs::create_dir_all(store.state_dir()).unwrap();
    // Exact persisted crash boundary: reservation committed, neither session file written yet.
    std::fs::write(store.state_dir().join("assistant-counter"), "1").unwrap();
    assert_eq!(
        store.create_assistant("pi", None, None, None).unwrap().id,
        "A-2"
    );
}

#[test]
fn assistant_capabilities_use_the_shared_resolver() {
    use corral_core::agent_capabilities::{AgentCapabilities, Preset};
    let mut caps = AgentCapabilities::default();
    let standard = caps.resolve(AgentRole::Assistant, None);
    assert!(standard.enabled("web.enabled"));
    assert!(standard.enabled("todo"));
    assert!(standard.enabled("lsp"));
    assert!(standard.enabled("subagents"));
    assert!(!standard.enabled("hooks"));
    assert!(!standard.enabled("browser"));
    assert!(!standard.enabled("lessons"));
    caps.set_preset(Preset::Full);
    let full = caps.resolve(AgentRole::Assistant, None);
    assert!(full.enabled("browser"));
    assert!(full.enabled("bash_rules"));
    assert!(full.enabled("oracle"));
    assert!(!full.enabled("llama_cpp.enabled"));
    // Keys that need session artifacts, session-owned jobs or scratch roots stay out of the column.
    for key in ["session_export", "background_shell", "checkpoints", "hooks"] {
        assert!(!full.features.contains_key(key), "{key}");
    }
    assert_ne!(standard.digest, full.digest);
}
