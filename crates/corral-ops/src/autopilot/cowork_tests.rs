use super::*;

#[test]
fn repair_destination_keeps_open_assistant_inbox_and_falls_back_for_gone_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(tmp.path()).unwrap();
    let ctx = Ctx::from_store(store);
    ctx.store
        .create_assistant("fake", None, None, None)
        .unwrap();
    // Offline is resumable, not a missing worker. An ended/missing session belongs to the owner.
    assert_eq!(destination(&ctx, "A-1"), (true, false));
    let me = crate::agent::effects::executor().unwrap();
    ctx.store.update_assistant_container("A-1", |container| {
        let now = ctx.store.now();
        container.state.generations.push(serde_json::from_value(json!({
            "n": 1, "bind_nonce_sha256": "nonce", "pane": "pane", "created": now,
            "bound": {"host_cap_sha256":"host", "ctl_cap_sha256":"ctl", "session_id":"sid",
                "session_file":tmp.path().join("session.jsonl"), "pid":me.pid, "start_time":me.start_time,
                "request_id":"hello", "at":now}
        }))?);
        container.state.binding.generation = 1;
        Ok(())
    }).unwrap();
    assert_eq!(destination(&ctx, "A-1"), (true, true));
    ctx.store
        .update_assistant("A-1", |s| {
            s.ended = Some(ctx.store.now());
            Ok(())
        })
        .unwrap();
    assert_eq!(destination(&ctx, "A-1"), (false, false));
    assert_eq!(destination(&ctx, "A-2"), (false, false));
}
