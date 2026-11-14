use crate::assistant::AssistantSend;

#[test]
fn absent_attachments_preserve_canonical_text_only_payload() {
    let original = serde_json::json!({"cmd_id":"c", "thread":"quick", "target":"assistant",
        "text":"typed words", "typed_at":"2026-11-14T00:00:00Z", "context":[]});
    let request: AssistantSend = serde_json::from_value(original.clone()).unwrap();
    assert!(request.attachments.is_empty());
    assert_eq!(serde_json::to_value(request).unwrap(), original);
}
