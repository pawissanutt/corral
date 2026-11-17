use super::*;

#[test]
fn same_model_choices_retain_billing_labels_and_select_the_exact_profile() {
    let mut feed = Feed::default();
    let snapshot: SettingsSnapshot = serde_json::from_value(serde_json::json!({
        "id":"settings", "workspace":"fixture", "store":"fixture", "host":"fixture",
        "mode":"assist", "grants":[], "assistant_profile":"subscription", "summary_profile":null,
        "listener":null, "config_hash":"fixture", "rev":1,
        "assistant_models":[
            {"name":"subscription", "model":"claude-sonnet-4-6", "effort":null, "note":"Anthropic · subscription"},
            {"name":"api", "model":"claude-sonnet-4-6", "effort":null, "note":"Anthropic · API (metered)"}
        ]
    })).unwrap();
    feed.settings.insert("settings".into(), snapshot);
    let list = model_list(&feed, "A-1", Some("subscription"));
    assert_eq!(list.options.len(), 2);
    assert_eq!(list.options[0].model, list.options[1].model);
    assert_ne!(list.options[0].note, list.options[1].note);
    assert!(list.options[0].selected);
    assert!(!list.options[1].selected);
    assert_eq!(
        list.options[1].action,
        Action::PickProfile {
            thread: "A-1".into(),
            profile: "api".into(),
        }
    );
}
