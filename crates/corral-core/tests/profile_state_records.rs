use corral_core::Store;
use corral_core::agent::{
    AgentBinding, AgentPm, AgentRole, AgentRunState, HostHealth, RECEIPTS_MAX, RequestReceipt,
    pm_state,
};
use serde_json::json;

#[path = "state_record_support/mod.rs"]
mod support;

fn pm(store: &Store) -> AgentPm {
    AgentPm::new(
        "fixture-pane".into(),
        AgentRunState::new(AgentBinding {
            role: AgentRole::Pm,
            store: corral_core::agent::paths::store_id(store),
            project: "fixture".into(),
            generation: 0,
            health: HostHealth::Unbound,
            callback: None,
            closing: false,
        }),
    )
}

#[test]
fn agent_pm_reads_are_snapshot_consistent_and_clear_deletes_the_record() {
    // Given a saved PM and a pinned read view.
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let first = pm(&store);
    pm_state::update(&store, |saved| {
        *saved = Some(first.clone());
        Ok(((), true))
    })
    .unwrap();
    let snapshot = store.read_only_snapshot().unwrap();
    // When another handle clears the PM binding.
    let writer = Store::open(root.path()).unwrap();
    pm_state::update(&writer, |saved| {
        *saved = None;
        Ok(((), true))
    })
    .unwrap();
    // Then the old view keeps its binding, but current readers see absence, not a JSON null.
    assert_eq!(pm_state::load(&snapshot).unwrap(), Some(first));
    assert_eq!(pm_state::load(&store).unwrap(), None);
    assert!(store.record_row("agent-pm", "current").unwrap().is_none());
    assert!(!store.dir().join("state").join("agent-pm.json").exists());
}

#[test]
fn agent_pm_failed_publication_keeps_previous_binding() {
    // Given a saved PM and a failure on its next record publication.
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let first = pm(&store);
    store.save_record("agent-pm", "current", &first).unwrap();
    let (failing, _fault) = support::fail_records(&store, "agent-pm", "current");
    // When the PM update attempts to publish a changed pane.
    let result = pm_state::update(&failing, |saved| {
        saved.as_mut().unwrap().pane = "replacement".into();
        Ok(((), true))
    });
    // Then reopening sees only the original binding.
    assert!(result.is_err());
    assert_eq!(
        pm_state::load(&Store::open(root.path()).unwrap()).unwrap(),
        Some(first)
    );
}

#[test]
fn agent_pm_update_refuses_a_writer_that_bypasses_the_dispatch_lease() {
    // Given a saved PM and a second writer deliberately bypassing the process lease.
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let first = pm(&store);
    store.save_record("agent-pm", "current", &first).unwrap();
    let mut concurrent = first;
    concurrent.pane = "concurrent-pane".into();
    // When the competing write commits after the update reads its original binding.
    let result = pm_state::update(&store, |saved| {
        std::thread::scope(|scope| {
            scope
                .spawn(|| store.save_record("agent-pm", "current", &concurrent))
                .join()
                .unwrap()
                .unwrap();
        });
        saved.as_mut().unwrap().pane = "stale-pane".into();
        Ok(((), true))
    });
    // Then version CAS refuses the stale update and keeps the competing writer's value.
    assert!(result.is_err());
    assert_eq!(pm_state::load(&store).unwrap(), Some(concurrent));
}

#[test]
fn config_observation_failure_does_not_publish_its_change_event() {
    // Given an observed configuration and a failure on the observation row.
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let before = store.record_row("config-observation", "current").unwrap();
    let (failing, _fault) = support::fail_records(&store, "config-observation", "current");
    // When a config edit writes its file but cannot commit the corresponding observation.
    let result = failing.edit_config(|doc| doc.set_str("ui.done_max", "17"));
    // Then no partial event/observation is visible; a later opener observes the file exactly once.
    assert!(result.is_err());
    assert_eq!(
        store.record_row("config-observation", "current").unwrap(),
        before
    );
    assert!(
        !store
            .events()
            .unwrap()
            .iter()
            .any(|event| event.kind == "config_changed")
    );
    let reopened = Store::open(root.path()).unwrap();
    assert_eq!(reopened.config.ui.done_max, 17);
    assert_eq!(
        reopened
            .events()
            .unwrap()
            .iter()
            .filter(|event| event.kind == "config_changed")
            .count(),
        1
    );
}

#[test]
fn failed_task_callback_discards_agent_pm_and_automation_records() {
    // Given state records and one task on the same backend.
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let task = store
        .create("fixture", None, corral_core::Status::Backlog, "")
        .unwrap();
    let binding = pm(&store);
    // When a task callback buffers records and then fails.
    let result = store.update(&task.id, |_| {
        store.save_record("agent-pm", "current", &binding)?;
        store.save_record("automation", "current", &json!({"version": 1}))?;
        anyhow::bail!("abort state mutation")
    });
    // Then neither record escapes the callback.
    assert!(result.is_err());
    assert_eq!(pm_state::load(&store).unwrap(), None);
    assert!(store.record_row("automation", "current").unwrap().is_none());
}

fn receipt(store: &Store, n: usize, generation: u32, owed: bool) -> RequestReceipt {
    serde_json::from_value(json!({
        "version": 1, "request_id": format!("r-{n}"), "op": "event", "payload_sha256": "0",
        "identity": {"store": corral_core::agent::paths::store_id(store), "project": "fixture",
            "role": "pm", "session_id": "s", "generation": generation},
        "state": "committed", "result": {}, "at": "2026-01-01T00:00:00Z",
        "post_pending": if owed { vec!["event:agent_bound"] } else { vec![] },
    }))
    .unwrap()
}

#[test]
fn full_pm_receipt_family_reclaims_settled_receipts_of_ended_generations() {
    // Given a full PM receipt family: generation 1 is ended, its first receipt still owes an
    // effect, and the newest receipt belongs to the current generation 2.
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let mut seeded = pm(&store);
    seeded.state.binding.generation = 2;
    seeded.state.receipts = (0..RECEIPTS_MAX)
        .map(|n| receipt(&store, n, if n + 1 == RECEIPTS_MAX { 2 } else { 1 }, n == 0))
        .collect();
    pm_state::update(&store, |saved| {
        *saved = Some(seeded);
        Ok(((), true))
    })
    .unwrap();
    // When a later append reclaims under the limit check, in the same transaction.
    let evicted = pm_state::update_selected(&store, &pm_state::Selection::default(), |saved| {
        let evicted = pm_state::retain(&store, &pm_state::RetentionLimits::PM)?.receipts;
        let pm = saved.as_mut().unwrap();
        pm.state
            .receipts
            .push(receipt(&store, RECEIPTS_MAX, 2, false));
        Ok((evicted, true))
    })
    .unwrap();
    // Then the oldest settled half is gone, the append counted, and owed/current receipts stay.
    assert_eq!(evicted, RECEIPTS_MAX as u64 / 2);
    assert_eq!(
        pm_state::child_count(&store, "receipts").unwrap(),
        RECEIPTS_MAX as u64 / 2 + 1
    );
    let state = pm_state::load(&store).unwrap().unwrap().state;
    let kept = |id: &str| state.receipt(id).is_some();
    assert!(kept("r-0") && !kept("r-1") && !kept(&format!("r-{}", RECEIPTS_MAX / 2)));
    assert!(kept(&format!("r-{}", RECEIPTS_MAX / 2 + 1)));
    assert!(kept(&format!("r-{}", RECEIPTS_MAX - 1)) && kept(&format!("r-{RECEIPTS_MAX}")));
}
