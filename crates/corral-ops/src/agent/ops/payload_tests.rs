use super::*;
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    #[serde(rename = "limit")]
    _limit: u32,
}

fn envelope(payload: serde_json::Value) -> Envelope {
    serde_json::from_value(json!({
        "version": 1, "op": "read_list", "request_id": "rq-0123456789abcdef0123456789abcdef",
        "payload_sha256": "a".repeat(64),
        "identity": {"store":"st-test", "project":"demo", "role":"pm",
            "session_id":"session", "generation":1},
        "auth": {"host_cap":"h".repeat(64)}, "payload":payload,
    }))
    .unwrap()
}

#[test]
fn malformed_payload_names_known_field_without_echoing_input() {
    for input in [json!({}), json!({"limit":"SECRET"}), json!({"limit":-1})] {
        let error = payload::<Input>(&envelope(input), "limit").unwrap_err();
        let rejection = error.downcast_ref::<Rejection>().unwrap();
        assert_eq!(rejection.code, RejectCode::BadRequest);
        let detail = rejection.detail.as_ref().unwrap();
        assert_eq!(detail["field"], "limit");
        assert_eq!(detail["expected_fields"], "limit");
        assert!(!format!("{rejection:?}").contains("SECRET"));
    }
    let error = payload::<Input>(&envelope(json!({"SECRET":0, "limit":1})), "limit").unwrap_err();
    let rejection = error.downcast_ref::<Rejection>().unwrap();
    assert!(!format!("{rejection:?}").contains("SECRET"));
    assert!(rejection.detail.as_ref().unwrap()["field"].is_null());
    assert!(payload::<Input>(&envelope(json!({"limit":4294967295u64})), "limit").is_ok());
}
