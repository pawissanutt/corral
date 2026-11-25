use crate::{attachment::is_attachment_label, session_host::SessionSend};

#[test]
fn attachment_labels_allow_legacy_and_larger_indices() {
    for label in ["a1", "a4", "a5", "a42"] {
        assert!(is_attachment_label(label), "{label}");
    }
    for label in ["a", "a0", "a01", "a+1", "a١", "a1x", "b1"] {
        assert!(!is_attachment_label(label), "{label}");
    }
}

#[test]
fn absent_attachments_preserve_canonical_text_only_payload() {
    let original = serde_json::json!({"id":"quick", "cmd_id":"c", "text":"typed words",
        "policy":"steer", "typed_at":"2026-11-14T00:00:00Z"});
    let request: SessionSend = serde_json::from_value(original.clone()).unwrap();
    assert!(request.attachments.is_empty());
    assert_eq!(serde_json::to_value(request).unwrap(), original);
}
