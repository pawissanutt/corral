//! Consumer-visible boundaries: keyed sends cannot replace words, escaped text cannot stall paging.
use super::*;
use corral_protocol::assistant::EntryKind;

fn message(session: &str, text: String) -> Message {
    Message {
        id: String::new(),
        session: session.into(),
        seq: 0,
        cmd_id: "send-1".into(),
        device: "device-1".into(),
        name: "Owner".into(),
        target: "assistant".into(),
        text,
        typed_at: "2026-11-14T00:00:00Z".into(),
        at: "2026-11-14T00:00:00Z".into(),
        state: "sent".into(),
        input: None,
        pm_item: None,
        saved_task: None,
        reply: None,
        failure: None,
        turn: None,
        generation: None,
        late: false,
        tainted: false,
        model: None,
        context: Vec::new(),
        attachments: Vec::new(),
        rev: 1,
    }
}

#[test]
fn keyed_send_preserves_original_words_and_rejects_payload_reuse() {
    let fx = crate::agent::test_support::fx();
    let head = fx.ctx.store.app_thread("quick").unwrap();
    let original = message(&head.session, "Owner's original words".into());
    let (first, replayed) = fx
        .ctx
        .store
        .append_app_message(original.clone(), "digest-1", &[])
        .unwrap();
    assert!(!replayed);
    let (again, replayed) = fx
        .ctx
        .store
        .append_app_message(original, "digest-1", &[])
        .unwrap();
    assert!(replayed);
    assert_eq!(again.id, first.id);
    assert!(
        fx.ctx
            .store
            .append_app_message(message(&head.session, "different".into()), "digest-2", &[])
            .is_err()
    );
    assert_eq!(
        fx.ctx.store.app_message(&first.id).unwrap().text,
        "Owner's original words"
    );
    assert_eq!(fx.ctx.store.app_thread_head(&head.session).unwrap().next, 2);
}

#[test]
fn escaped_and_multibyte_text_pages_progress_without_losing_bytes() {
    let fx = crate::agent::test_support::fx();
    let head = fx.ctx.store.app_thread("quick").unwrap();
    let owner = "\u{0001}".repeat(16 * 1024);
    let answer = "界".repeat(7000);
    let (saved, _) = fx
        .ctx
        .store
        .append_app_message(message(&head.session, owner.clone()), "digest", &[])
        .unwrap();
    fx.ctx
        .store
        .update_app_message(&saved.id, |m| {
            m.reply = Some(answer.clone());
            m.state = "done".into();
            Ok(())
        })
        .unwrap();
    let mut before = None;
    let mut pages = Vec::new();
    loop {
        let (rows, next, _) = thread(&fx.ctx, "quick", before, 100).unwrap();
        assert!(!rows.is_empty());
        assert!(serde_json::to_vec(&rows).unwrap().len() <= 54 * 1024 + 100);
        pages.push(rows);
        let Some(next) = next else {
            break;
        };
        assert!(before.is_none_or(|old| next < old));
        before = Some(next);
    }
    let rows: Vec<_> = pages.into_iter().rev().flatten().collect();
    let rendered = |kind| {
        rows.iter()
            .filter(|row| row.kind == kind)
            .filter_map(|row| row.text.as_deref())
            .collect::<String>()
    };
    assert_eq!(rendered(EntryKind::Message), owner);
    assert_eq!(rendered(EntryKind::Answer), answer);
}
