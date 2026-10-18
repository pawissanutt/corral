use corral_core::{Accent, Config, ConfigDoc, Status, UiCfg};

#[test]
fn preferences_keep_existing_defaults_when_ui_is_omitted() {
    // Given an existing workspace without UI preferences.
    let config: Config = toml::from_str("").unwrap();
    // When the UI defaults are read.
    let ui = config.ui;
    // Then existing board behavior is retained and desktop alerts are opt-in.
    assert_eq!(ui.columns, Status::COLUMNS);
    assert!(ui.show_wip && ui.mouse);
    assert_eq!(ui.accent, Accent::Cyan);
    assert!(!ui.compact_cards && !ui.notify_needs_me && !ui.notify_review);
    assert!(!ui.route_warnings);
    assert_eq!(ui.canonical_key('n'), Some('n'));
    assert_eq!(ui.canonical_key('1'), Some('1'));
}

#[test]
fn route_warnings_accept_only_booleans_and_round_trip_both_values() {
    // Given an explicit route warning preference.
    for enabled in [false, true] {
        let mut doc = ConfigDoc::default();
        doc.set_str("ui.route_warnings", &enabled.to_string())
            .unwrap();
        // When the configuration is parsed and serialized again.
        let config = doc.config().unwrap();
        let saved: Config = toml::from_str(&config.to_toml()).unwrap();
        // Then either boolean survives the configuration boundary.
        assert_eq!(saved.ui.route_warnings, enabled);
    }
    for invalid in ["\"true\"", "1", "[]"] {
        let mut doc = ConfigDoc::default();
        doc.set_str("ui.route_warnings", invalid).unwrap();
        assert!(
            doc.config().is_err(),
            "route_warnings must reject {invalid}"
        );
    }
}

#[test]
fn swapped_shortcuts_resolve_once_when_both_actions_are_remapped() {
    // Given a valid swap of capture and dispatch.
    let mut ui = UiCfg::default();
    ui.keys.insert("capture".into(), "d".into());
    ui.keys.insert("dispatch".into(), "n".into());
    // When physical keys are resolved.
    let resolved = [ui.canonical_key('d'), ui.canonical_key('n')];
    // Then each maps to exactly one canonical action.
    assert_eq!(resolved, [Some('n'), Some('d')]);
}

#[test]
fn invalid_shortcuts_and_columns_are_rejected_when_config_is_validated() {
    // Given invalid keys, reserved digits, a collision, and non-board columns.
    let root = tempfile::tempdir().unwrap();
    let cases = [
        ("ui.columns", "[]"),
        ("ui.columns", "[\"idea\"]"),
        ("ui.columns", "[\"ready\", \"ready\"]"),
        ("ui.keys.capture", "\"d\""),
        ("ui.keys.capture", "\"1\""),
        ("ui.keys.capture", "\"ctrl+n\""),
        ("ui.keys.capture", "\" \""),
        ("ui.keys.missing", "\"z\""),
        ("ui.keys.open", "\"z\""),
    ];
    for (key, value) in cases {
        let mut doc = ConfigDoc::default();
        doc.set_str(key, value).unwrap();
        // When validation runs at the configuration boundary.
        let problems = doc.config().unwrap().validate(root.path()).unwrap_err();
        // Then the invalid preference blocks saving.
        assert!(
            problems.0.iter().any(|p| p.starts_with("ui.")),
            "{key}: {problems}"
        );
    }
}

#[test]
fn staged_preference_delta_preserves_concurrent_keys_when_saved() {
    // Given staged personal preferences and unrelated concurrent edits.
    let mut doc = ConfigDoc::parse("# keep this comment\n[ui]\nmouse = false\n").unwrap();
    let base = doc.config().unwrap();
    let mut draft = base.clone();
    draft.ui.compact_cards = true;
    draft.ui.notify_review = true;
    draft.ui.keys.insert("capture".into(), "z".into());
    doc.set_str("ui.accent", "magenta").unwrap();
    // When only the draft's delta is applied.
    doc.apply_config_changes(&base, &draft).unwrap();
    // Then concurrent keys and comments survive and the staged changes round trip.
    let saved = doc.config().unwrap();
    assert_eq!(saved.ui.accent, Accent::Magenta);
    assert!(!saved.ui.mouse);
    assert!(saved.ui.compact_cards && saved.ui.notify_review);
    assert_eq!(saved.ui.bound_key('n'), 'z');
    assert!(doc.to_string().starts_with("# keep this comment"));
}
