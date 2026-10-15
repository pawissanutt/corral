use corral_core::{Config, ConfigDoc, Store};
use tempfile::tempdir;

#[test]
fn old_configs_keep_push_disabled_and_default_kinds() {
    // Given an old config with either no PM table or only its agent.
    for text in ["", "[pm]\nagent = \"omo\"\n"] {
        // When it is parsed.
        let cfg = ConfigDoc::parse(text).unwrap().config().unwrap();
        // Then push is opt-in and all actionable kinds are selected.
        assert!(!cfg.pm.push_events);
        assert_eq!(
            cfg.pm.push_kinds,
            [
                "done",
                "needs_me",
                "stuck",
                "blocked",
                "escalated",
                "config_changed",
                "lessons_due"
            ]
        );
    }
    let root = tempdir().unwrap();
    let cfg = Config::default_for(root.path());
    assert!(!cfg.pm.push_events);
    assert_eq!(cfg.pm.push_kinds.len(), 7);
}

#[test]
fn push_options_persist_through_config_edits() {
    // Given a workspace with the default config.
    let root = tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    // When push and its selected kinds are saved.
    store
        .edit_config(|doc| {
            let mut cfg = doc.config()?;
            cfg.pm.push_events = true;
            cfg.pm.push_kinds = vec!["done".into(), "blocked".into()];
            doc.sync_from(&cfg)
        })
        .unwrap();
    // Then reopening reads the machine values unchanged.
    let cfg = Store::open(root.path()).unwrap().config;
    assert!(cfg.pm.push_events);
    assert_eq!(cfg.pm.push_kinds, ["done", "blocked"]);
}

#[test]
fn invalid_push_kinds_are_rejected_without_writing() {
    // Given a valid workspace config.
    let root = tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let before = std::fs::read_to_string(store.config_path()).unwrap();
    // When a non-actionable kind is saved, even while push is off.
    let result = store.edit_config(|doc| {
        let mut cfg = doc.config()?;
        cfg.pm.push_kinds = vec!["working".into(), "".into()];
        doc.sync_from(&cfg)
    });
    // Then validation rejects it and the file is untouched.
    assert!(result.is_err());
    assert_eq!(
        std::fs::read_to_string(store.config_path()).unwrap(),
        before
    );
}

#[test]
fn explicit_empty_push_kinds_stays_empty() {
    // Given an explicitly empty selection.
    let text = "[pm]\nagent = \"omo\"\npush_events = true\npush_kinds = []\n";
    // When parsed and serialized again.
    let cfg = ConfigDoc::parse(text).unwrap().config().unwrap();
    let saved = ConfigDoc::parse(&cfg.to_toml()).unwrap().config().unwrap();
    // Then empty does not mean the default selection.
    assert!(saved.pm.push_events);
    assert!(saved.pm.push_kinds.is_empty());
}
