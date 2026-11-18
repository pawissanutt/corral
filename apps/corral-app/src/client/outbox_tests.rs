use super::*;

fn message(id: &str) -> Message {
    Message {
        request: serde_json::from_value(serde_json::json!({
            "cmd_id":id,"thread":"quick","target":"assistant","text":format!("owner words {id}"),
            "typed_at":"2026-11-15T10:00:00Z","context":[]
        }))
        .unwrap(),
        attempted: false,
        cancel: false,
        refused: false,
        error: None,
    }
}

#[test]
fn independent_outboxes_reload_each_transaction_and_never_revert_cancel() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let mut a = Outbox::load("interleaved-store", "same-device").unwrap();
    let mut b = Outbox::load("interleaved-store", "same-device").unwrap();
    a.update(|saved| {
        saved.messages.insert("a".into(), message("a"));
    })
    .unwrap();
    b.update(|saved| {
        saved.messages.insert("b".into(), message("b"));
        saved.drafts.insert("quick".into(), "new draft".into());
    })
    .unwrap();
    a.update(|saved| {
        saved.messages.get_mut("a").unwrap().cancel = true;
    })
    .unwrap();
    b.update(|saved| {
        saved.drafts.insert("named".into(), "other words".into());
    })
    .unwrap();
    let saved = Outbox::load("interleaved-store", "same-device")
        .unwrap()
        .saved;
    assert_eq!(saved.messages.len(), 2);
    assert!(saved.messages["a"].cancel);
    assert_eq!(saved.drafts.len(), 2);
    a.update(|saved| {
        saved.messages.remove("a");
        saved.cancelled.insert("a".into());
    })
    .unwrap();
    assert!(
        durable::wait(b.update_checked(|saved| {
            if saved.cancelled.contains("a") {
                return Err("withdrawn".into());
            }
            saved.messages.insert("a".into(), message("a"));
            Ok(())
        }))
        .is_err()
    );
    assert!(
        !Outbox::load("interleaved-store", "same-device")
            .unwrap()
            .saved
            .messages
            .contains_key("a")
    );
}

#[test]
fn pruning_keeps_owner_words_and_drops_the_retired_command_journal() {
    let mut records = durable::Records::new();
    records.insert(
        "namespace/old/message/a".into(),
        serde_json::to_value(message("a")).unwrap(),
    );
    records.insert(
        "namespace/old/draft/quick".into(),
        Value::String("unsent".into()),
    );
    records.insert(
        "namespace/old/command/running".into(),
        serde_json::json!({"method": "act.submit", "params": null}),
    );
    records.insert(
        "namespace/empty/meta".into(),
        serde_json::json!({"bytes":0}),
    );
    prune_settled(&mut records, "namespace/current/").unwrap();
    assert_eq!(records.len(), 2);
    assert!(records.contains_key("namespace/old/message/a"));
    assert!(records.contains_key("namespace/old/draft/quick"));
}
