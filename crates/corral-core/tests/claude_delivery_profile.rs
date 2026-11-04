use corral_core::{Config, ProfileChoice};

#[test]
fn claude_profile_owns_one_structured_settings_argument() {
    let mut config = Config::default_for(std::path::Path::new("/workspace"));
    let choice = ProfileChoice {
        profile: Some("claude".into()),
        ..Default::default()
    };
    let resolved = config.resolve_profile(&choice, None, None).unwrap();
    assert_eq!(
        resolved.delivery_args().unwrap(),
        ["--settings", r#"{"crossSessionInbound":"accept"}"#]
    );
    for command in [
        "claude {model_args}",
        "claude '{delivery_args}'",
        "claude {delivery_args} {delivery_args}",
        "claude --settings '{}' {delivery_args}",
        "claude --settings='{}' {delivery_args}",
    ] {
        config.harnesses.get_mut("claude").unwrap().command = command.into();
        assert!(
            config.resolve_profile(&choice, None, None).is_err(),
            "{command}"
        );
    }
    config.harnesses.get_mut("claude").unwrap().command =
        "claude {delivery_args} {model_args}".into();
    config.profiles.get_mut("claude").unwrap().args = vec!["--settings={}".into()];
    assert!(config.resolve_profile(&choice, None, None).is_err());
}
