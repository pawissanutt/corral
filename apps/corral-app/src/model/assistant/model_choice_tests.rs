use super::*;

#[test]
fn only_the_chat_hosts_profiles_are_models_and_the_exact_one_is_selected() {
    let mut feed = Feed::default();
    let snapshot: SettingsSnapshot = serde_json::from_value(serde_json::json!({
        "config": {"profiles": {
            "api": {"harness": "pi", "model": "claude-sonnet-4-6"},
            "subscription": {"harness": "pi", "model": "claude-sonnet-4-6", "effort": "high"},
            "worker": {"harness": "omp", "model": "opus"}
        }},
        "listener": null,
        "devices": []
    }))
    .unwrap();
    feed.settings = Some(snapshot);
    let list = model_list(&feed, "A-1", Some("subscription"));
    let names: Vec<_> = list.options.iter().map(|option| option.name.as_str()).collect();
    assert_eq!(names, ["api", "subscription"]);
    assert!(!list.options[0].selected);
    assert!(list.options[1].selected);
    assert_eq!(
        list.options[0].action,
        Action::PickProfile {
            thread: "A-1".into(),
            profile: "api".into(),
        }
    );
}
