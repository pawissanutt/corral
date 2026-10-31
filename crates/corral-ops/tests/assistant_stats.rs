use corral_core::{Store, Usage, UsageEntry};
use corral_ops::{StatsBy, assistant_role_stats, assistant_stats, stats};

#[test]
fn assistant_usage_has_its_own_role_and_does_not_enter_task_cohorts() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    store
        .create_assistant("pi", None, Some("pane-1"), None)
        .unwrap();
    store
        .create_assistant("pi", Some("T-42"), Some("pane-2"), None)
        .unwrap();
    let rows = assistant_stats(&store, None).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter()
            .all(|row| row.usage.is_none() && row.cost == "unknown")
    );
    assert_eq!(rows[0].pane.as_deref(), Some("pane-1"));
    assert_eq!(rows[1].focus, "T-42");
    let usage = Usage::from_entries(vec![UsageEntry {
        input: 12,
        output: 3,
        reported_usd: Some(0.5),
        ..UsageEntry::default()
    }]);
    store
        .update_assistant("A-1", |session| {
            session.usage = Some(usage.clone());
            Ok(())
        })
        .unwrap();
    let role = assistant_role_stats(&store, None).unwrap();
    assert_eq!(role.role, "assistant");
    assert_eq!(role.usage.unwrap().total_tokens(), 15);
    assert!(role.cost.contains("partial"));
    store.end_assistant("A-1").unwrap();
    assert_eq!(assistant_stats(&store, None).unwrap()[0].usage, Some(usage));
    assert!(stats(&store, StatsBy::Profile, None).unwrap().is_empty());
    let cutoff = jiff::Timestamp::now()
        .checked_add(std::time::Duration::from_secs(1))
        .unwrap();
    assert!(assistant_stats(&store, Some(cutoff)).unwrap().is_empty());
}

#[test]
fn bound_transcript_usage_is_deduplicated_across_generations_and_retained_after_end() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let session = store.create_assistant("pi", None, None, None).unwrap();
    let transcript = tmp.path().join("session.jsonl");
    std::fs::write(&transcript, concat!(
        "{\"type\":\"session\",\"id\":\"sid\"}\n",
        "{\"type\":\"message\",\"id\":\"m1\",\"timestamp\":\"2026-10-01T00:00:00Z\",",
        "\"message\":{\"role\":\"assistant\",\"usage\":{\"input\":10,\"output\":2,\"cost\":{\"total\":0.5}}}}\n"
    )).unwrap();
    store.update_assistant_container(&session.id, |container| {
        for n in 1..=2 {
            container.state.generations.push(serde_json::from_value(serde_json::json!({
                "n": n, "bind_nonce_sha256": "nonce", "pane": "pane", "created": session.created,
                "bound": {
                    "host_cap_sha256": "host", "ctl_cap_sha256": "ctl", "session_id": "sid",
                    "session_file": transcript, "pid": 123, "start_time": "start", "request_id": "hello",
                    "at": session.created
                }
            })).unwrap());
        }
        container.state.binding.generation = 2;
        Ok(())
    }).unwrap();
    let before = assistant_stats(&store, None)
        .unwrap()
        .remove(0)
        .usage
        .unwrap();
    assert_eq!(before.total_tokens(), 12);
    assert_eq!(before.reported_usd, Some(0.5));
    store.end_assistant(&session.id).unwrap();
    let after = assistant_stats(&store, None)
        .unwrap()
        .remove(0)
        .usage
        .unwrap();
    assert_eq!(before, after);
    assert_eq!(
        assistant_role_stats(&store, None).unwrap().usage,
        Some(after)
    );
}
