use super::*;
use corral_core::{
    Store,
    agent::{AgentBinding, AgentRunState},
};
use std::io::Write;

fn response(id: &str, at: Option<Timestamp>) -> serde_json::Value {
    serde_json::json!({
        "type": "message", "id": id, "timestamp": at,
        "message": {"role": "assistant", "responseId": id, "model": "fixture-model",
            "provider": "openai-codex", "usage": {"input": 1_000_000, "output": 0}}
    })
}

fn write_rows(path: &std::path::Path, rows: &[serde_json::Value]) -> Result<()> {
    let mut file = std::fs::File::create(path)?;
    for row in rows {
        writeln!(file, "{row}")?;
    }
    Ok(())
}

fn total(facts: &Aggregate) -> f64 {
    facts
        .groups
        .values()
        .flat_map(|days| days.values())
        .map(|total| total.cost().unwrap())
        .sum()
}

fn full(ctx: &Ctx, state: &AgentRunState, cutoff: Timestamp, now: Timestamp) -> Result<Aggregate> {
    let mut facts = Aggregate::new(usize::MAX);
    for entry in crate::agent::usage::collect_state(state, None, &ctx.store)?.entries {
        let connection = entry
            .connection
            .map(|c| c.id().to_owned())
            .unwrap_or_default();
        facts.add(connection, Some("fixture".into()), entry, cutoff, now);
    }
    Ok(facts)
}

#[test]
fn saved_closed_rollup_plus_live_scan_matches_full_repricing_and_append() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let mut store = Store::init(tmp.path())?;
    store.config.pricing.insert(
        "fixture-model".into(),
        serde_json::from_value(serde_json::json!({"input": 2.0, "output": 0.0}))?,
    );
    let now: Timestamp = "2026-11-15T12:00:00Z".parse()?;
    let cutoff: Timestamp = "2026-10-17T00:00:00Z".parse()?;
    let yesterday: Timestamp = "2026-11-14T23:59:59Z".parse()?;
    let closed = tmp.path().join("a-closed.jsonl");
    let live = tmp.path().join("z-live.jsonl");
    let header = serde_json::json!({"type":"session", "id":"same-session"});
    write_rows(
        &closed,
        &[
            header.clone(),
            response("old", Some("2026-10-01T00:00:00Z".parse()?)),
            response("a", Some(yesterday)),
            response("undated", None),
        ],
    )?;
    // A copied/resumed transcript repeats a paid response; it must not multiply spend.
    let mut duplicate = response("a", Some(yesterday));
    duplicate["message"]["usage"]["input"] = 2_000_000.into();
    write_rows(&live, &[header, duplicate, response("b", Some(now))])?;
    let binding: AgentBinding = serde_json::from_value(serde_json::json!({
        "role":"assistant", "store":"fixture", "project":"fixture", "generation":2
    }))?;
    let mut state = AgentRunState::new(binding);
    for (n, path, fenced) in [(1, &closed, Some(yesterday)), (2, &live, None)] {
        state.generations.push(serde_json::from_value(serde_json::json!({
            "n":n, "bind_nonce_sha256":"nonce", "pane":"fixture", "created":yesterday,
            "fenced":fenced.map(|at| serde_json::json!({"at":at, "reason":"ended"})),
            "bound":{"host_cap_sha256":"host", "ctl_cap_sha256":"ctl", "session_id":"same-session",
                "session_file":path, "pid":1, "start_time":"fixture", "request_id":"hello", "at":yesterday}
        }))?);
    }
    let mut ctx = crate::resources::FixedResourceProbe::ctx(store);
    let read = |ctx: &Ctx, max| {
        rollup::state(
            ctx,
            rollup::Source {
                key: "fixture",
                state: &state,
                ended: false,
                profile: Some("fixture".into()),
            },
            cutoff,
            now,
            max,
        )
    };
    let first = read(&ctx, 100)?;
    assert_eq!(first.budget.coverage.scanned_rows, 7);
    assert_eq!(total(&first), 6.0);
    assert_eq!(total(&first), total(&full(&ctx, &state, cutoff, now)?));
    assert_eq!(first.budget.coverage.undated_entries, 1);
    let group = first.groups.values().next().unwrap();
    assert_eq!(group.len(), 3); // yesterday, today, and explicitly undated
    let saved = read(&ctx, 1)?;
    assert_eq!(saved.budget.coverage.scanned_rows, 0);
    assert!(!saved.budget.coverage.partial);
    assert_eq!(total(&saved), total(&first));

    ctx.store
        .config
        .pricing
        .get_mut("fixture-model")
        .unwrap()
        .input = 3.0;
    let repriced = read(&ctx, 100)?;
    assert_eq!(total(&repriced), 9.0);
    assert_eq!(total(&repriced), total(&full(&ctx, &state, cutoff, now)?));
    let mut file = std::fs::OpenOptions::new().append(true).open(&live)?;
    writeln!(file, "{}", response("c", Some(now)))?;
    let appended = read(&ctx, 100)?;
    // Only the live file is scanned. Closed rows were summarized completely once.
    assert_eq!(appended.budget.coverage.scanned_rows, 4);
    assert_eq!(total(&appended), 12.0);
    assert_eq!(total(&appended), total(&full(&ctx, &state, cutoff, now)?));

    let next_file = tmp.path().join("zz-next.jsonl");
    write_rows(
        &next_file,
        &[
            serde_json::json!({"type":"session", "id":"new-session"}),
            response("d", Some(now)),
        ],
    )?;
    let mut next_state = state.clone();
    next_state.generations[1].fenced = Some(serde_json::from_value(
        serde_json::json!({"at":now, "reason":"ended"}),
    )?);
    let mut generation = next_state.generations[1].clone();
    generation.n = 3;
    generation.fenced = None;
    let bound = generation.bound.as_mut().unwrap();
    bound.session_id = "new-session".into();
    bound.session_file = next_file;
    next_state.generations.push(generation);
    next_state.binding.generation = 3;
    let read_next = |max| {
        rollup::state(
            &ctx,
            rollup::Source {
                key: "fixture",
                state: &next_state,
                ended: false,
                profile: Some("fixture".into()),
            },
            cutoff,
            now,
            max,
        )
    };
    let advanced = read_next(100)?;
    // Advancing a generation scans just the newly closed file and the new live file.
    assert_eq!(advanced.budget.coverage.scanned_rows, 6);
    assert_eq!(total(&advanced), 15.0);
    assert_eq!(
        total(&advanced),
        total(&full(&ctx, &next_state, cutoff, now)?)
    );

    // Disposable caches are never authority: malformed files force a source rebuild.
    for entry in std::fs::read_dir(ctx.store.cache_root()?.join("usage-v1"))? {
        std::fs::write(entry?.path(), b"broken cache")?;
    }
    let rebuilt = read_next(100)?;
    assert_eq!(rebuilt.budget.coverage.scanned_rows, 10);
    assert_eq!(total(&rebuilt), total(&advanced));
    assert!(!rebuilt.budget.coverage.partial);
    Ok(())
}
