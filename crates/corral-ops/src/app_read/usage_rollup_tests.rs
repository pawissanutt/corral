use super::*;
use corral_core::connection::ConnectionId;
use corral_core::{Run, Status, Store};

#[test]
fn truncated_assistant_transcript_does_not_starve_task_usage() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let store = Store::init(tmp.path())?;
    let session = store.create_assistant("pi", None, None, None)?;
    let transcript = tmp.path().join("session.jsonl");
    std::fs::write(
        &transcript,
        concat!(
            "{\"type\":\"session\",\"id\":\"sid\"}\n",
            "{\"type\":\"message\",\"id\":\"user\",\"message\":{\"role\":\"user\"}}\n",
            "{\"type\":\"message\",\"id\":\"a\",\"message\":{\"role\":\"assistant\",\"usage\":{\"input\":0,\"output\":0,\"cost\":{\"total\":0.25}}}}\n",
            "{\"type\":\"message\",\"id\":\"b\",\"message\":{\"role\":\"assistant\",\"usage\":{\"input\":0,\"output\":0,\"cost\":{\"total\":0.25}}}}\n",
            "{\"type\":\"message\",\"id\":\"c\",\"message\":{\"role\":\"assistant\",\"usage\":{\"input\":0,\"output\":0,\"cost\":{\"total\":0.25}}}}\n",
        ),
    )?;
    store.update_assistant_container(&session.id, |container| {
        container
            .state
            .generations
            .push(serde_json::from_value(serde_json::json!({
                "n": 1, "bind_nonce_sha256": "nonce", "pane": "pane",
                "created": session.created,
                "bound": {
                    "host_cap_sha256": "host", "ctl_cap_sha256": "ctl", "session_id": "sid",
                    "session_file": transcript, "pid": 123, "start_time": "start",
                    "request_id": "hello", "at": session.created
                }
            }))?);
        container.state.binding.generation = 1;
        Ok(())
    })?;
    let mut task = store.create(
        "task usage survives transcript limit",
        None,
        Status::Running,
        "",
    )?;
    let mut run: Run = serde_json::from_value(serde_json::json!({
        "id": "R1", "agent": "task-profile", "worktree": "/unused", "branch": "fixture",
        "base": "main", "base_sha": "fixture", "started": Timestamp::now()
    }))?;
    run.usage = Some(Usage::from_entries(vec![UsageEntry {
        at: Some(Timestamp::now()),
        connection: Some(ConnectionId::AnthropicApi),
        input: 1,
        reported_usd: Some(0.5),
        ..UsageEntry::default()
    }]));
    task.runs.push(run);
    store.fixture_put_task(&task)?;
    let container = store.get_assistant_container(&session.id)?;
    let collected = crate::agent::usage::collect_state(&container.state, None, &store)?;
    assert_eq!(collected.reported_usd, Some(0.75));
    let ctx = crate::resources::FixedResourceProbe::ctx(store);
    let now = Timestamp::now();

    let bounded = facts_with_limit(&ctx, now, 3)?;
    assert!(bounded.budget.coverage.partial);
    // One assistant record, two transcript lines, and two task rows were scanned.
    assert_eq!(bounded.budget.coverage.scanned_rows, 5);
    let task_group = bounded
        .groups
        .get(&("anthropic-api".into(), Some("task-profile".into())))
        .unwrap();
    assert_eq!(task_group.values().next().unwrap().cost(), Some(0.5));

    let full = facts_with_limit(&ctx, now, 100)?;
    assert!(!full.budget.coverage.partial);
    let full_cost: f64 = full
        .groups
        .values()
        .flat_map(|days| days.values())
        .map(|total| total.cost().unwrap())
        .sum();
    assert_eq!(full_cost, collected.reported_usd.unwrap() + 0.5);
    let saved = facts_with_limit(&ctx, now, 3)?;
    assert_eq!(saved.budget.coverage.scanned_rows, 1); // only the assistant header
    assert!(!saved.budget.coverage.partial);
    task.runs[0].usage = Some(Usage::from_entries(vec![UsageEntry {
        at: Some(now),
        connection: Some(ConnectionId::AnthropicApi),
        reported_usd: Some(0.8),
        ..UsageEntry::default()
    }]));
    ctx.store.fixture_put_task(&task)?;
    let changed = facts_with_limit(&ctx, now, 3)?;
    let task_group = changed
        .groups
        .get(&("anthropic-api".into(), Some("task-profile".into())))
        .unwrap();
    assert_eq!(task_group.values().next().unwrap().cost(), Some(0.8));
    Ok(())
}
