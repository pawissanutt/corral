use crate::session_host::SessionSend;

#[test]
fn absent_attachments_preserve_canonical_text_only_payload() {
    let original = serde_json::json!({"id":"quick", "cmd_id":"c", "text":"typed words",
        "policy":"steer", "typed_at":"2026-11-14T00:00:00Z"});
    let request: SessionSend = serde_json::from_value(original.clone()).unwrap();
    assert!(request.attachments.is_empty());
    assert_eq!(serde_json::to_value(request).unwrap(), original);
}
