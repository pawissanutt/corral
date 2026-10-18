use std::fs;
use std::sync::mpsc;

use corral_core::{
    ConfigSource, Event, Store, actionable_kind, capture_messages, format_event_line,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::tempdir;

fn changed(store: &Store) -> Vec<Event> {
    store
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "config_changed")
        .collect()
}

#[test]
fn operating_changes_are_policy_relevant_without_granting_authority() {
    // Given a legacy workspace without an automation receipt.
    let root = tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    // When an explicit narrowing control changes.
    store.pause_operating_policy(true).unwrap();
    // Then policy consumers can observe it, but no grant was created.
    let events = changed(&store);
    assert_eq!(events.len(), 1);
    assert_eq!(change(&events[0], "automation.paused")["pm_relevant"], true);
    assert_eq!(
        change(&events[0], "automation.paused")["applies"],
        "next_sync"
    );
    assert!(store.operating_state().unwrap().approval.is_none());
}

fn change<'a>(event: &'a Event, key: &str) -> &'a Value {
    event.detail["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["key"] == key)
        .unwrap()
}

fn external(store: &Store, key: &str, value: &str) -> String {
    let mut doc = corral_core::ConfigDoc::load(&store.config_path()).unwrap();
    doc.set_str(key, value).unwrap();
    let text = doc.to_string();
    fs::write(store.config_path(), &text).unwrap();
    text
}

#[test]
fn file_reload_records_semantics_hash_binding_and_deduplicates_observers() {
    let root = tempdir().unwrap();
    let mut board = Store::init(root.path()).unwrap();
    let mut worker = board.clone();
    let modified = fs::metadata(board.config_path())
        .unwrap()
        .modified()
        .unwrap();
    let text = external(&board, "limits.running", "8");
    // Same-size writes can retain the same filesystem timestamp; content must still win.
    // Windows requires write access to set a retained file handle's timestamps.
    fs::OpenOptions::new()
        .write(true)
        .open(board.config_path())
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    assert!(board.reload_config_if_changed().unwrap());
    assert!(worker.reload_config_if_changed().unwrap());
    assert!(!board.reload_config_if_changed().unwrap());
    let events = changed(&board);
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event.detail["source"], "file_edit");
    assert_eq!(
        event.detail["hash"],
        format!("{:x}", Sha256::digest(text.as_bytes()))
    );
    assert_eq!(
        change(event, "limits.running"),
        &json!({
            "key": "limits.running", "old": 3, "new": 8,
            "pm_relevant": true, "applies": "next_sync"
        })
    );
    assert_eq!(actionable_kind(event), Some("config_changed"));
    assert!(format_event_line(event).contains("limits.running"));
    assert!(board.config.limits.running == 8 && worker.config.limits.running == 8);
}

#[test]
fn comments_defaults_and_key_order_do_not_emit_semantic_changes() {
    let root = tempdir().unwrap();
    let mut store = Store::init(root.path()).unwrap();
    let text = fs::read_to_string(store.config_path()).unwrap();
    fs::write(store.config_path(), format!("# owner note\n{text}\n")).unwrap();
    assert!(store.reload_config_if_changed().unwrap());
    assert!(changed(&store).is_empty());
    // An explicit default deserializes to the same effective setting.
    external(&store, "sync.idle_after_secs", "120");
    store.reload_config_if_changed().unwrap();
    assert!(changed(&store).is_empty());
}

#[test]
fn cli_settings_and_new_process_observe_each_revision_once() {
    let root = tempdir().unwrap();
    let mut store = Store::init(root.path()).unwrap();
    store
        .edit_config(|doc| doc.set_str("limits.running", "7"))
        .unwrap();
    store.reload_config_if_changed().unwrap();
    store
        .edit_config_from(ConfigSource::SettingsUi, |doc| {
            doc.set_str("review.accept", "auto")?;
            doc.set_str("defaults.profile", "claude")
        })
        .unwrap();
    store.reload_config_if_changed().unwrap();
    external(&store, "limits.running", "9");
    let opened = Store::open(root.path()).unwrap();
    Store::open(root.path()).unwrap();
    let events = changed(&opened);
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].detail["source"], "cli");
    assert_eq!(events[1].detail["source"], "settings_ui");
    assert_eq!(change(&events[1], "review.accept")["applies"], "next_gate");
    assert_eq!(
        change(&events[1], "defaults.profile")["applies"],
        "new_dispatches"
    );
    assert_eq!(events[2].detail["source"], "file_edit");
}

#[test]
fn profile_add_remove_and_secret_only_changes_are_visible_but_redacted() {
    let root = tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    store
        .edit_config(|doc| {
            doc.set_str("profiles.fast.harness", "generic")?;
            doc.set_str("profiles.fast.args", r#"["--api-key", "secret-one"]"#)?;
            doc.set_str(
                "harnesses.generic.command",
                "runner secret-command {model_args}",
            )?;
            doc.set_str("review.gate", r#"["TOKEN=secret-gate cargo test"]"#)?;
            doc.set_str("review.env.CONNECTION", "secret-connection")?;
            doc.set_str(
                "routing.rules",
                r#"[{ profile = "omo", body_regex = "secret-pattern" }]"#,
            )
        })
        .unwrap();
    store
        .edit_config(|doc| doc.set_str("profiles.fast.args", r#"["secret-two"]"#))
        .unwrap();
    store
        .edit_config(|doc| {
            doc.remove_entry("profiles", "fast")?;
            Ok(())
        })
        .unwrap();
    let events = changed(&store);
    assert_eq!(events.len(), 3);
    assert_eq!(
        change(&events[0], "profiles.fast.harness")["old"],
        Value::Null
    );
    assert_eq!(
        change(&events[2], "profiles.fast.harness")["new"],
        Value::Null
    );
    assert_eq!(
        change(&events[1], "profiles.fast.args")["old"],
        "[redacted]"
    );
    assert_eq!(
        change(&events[1], "profiles.fast.args")["new"],
        "[redacted]"
    );
    let log = fs::read_to_string(store.dir().join("events.jsonl")).unwrap();
    let state = fs::read_to_string(store.state_dir().join("config-observation.json")).unwrap();
    for secret in [
        "secret-one",
        "secret-two",
        "secret-command",
        "secret-gate",
        "secret-pattern",
        "secret-connection",
    ] {
        assert!(!log.contains(secret), "secret in event log");
        assert!(!state.contains(secret), "secret in observation state");
    }
}

#[test]
fn invalid_syntax_schema_and_validation_keep_previous_and_use_sink() {
    let root = tempdir().unwrap();
    let mut board = Store::init(root.path()).unwrap();
    let mut worker = board.clone();
    let valid = fs::read_to_string(board.config_path()).unwrap();
    let previous = board.config.clone();
    let (tx, rx) = mpsc::channel();
    let _sink = capture_messages(tx);
    for text in [
        "api_key = \"secret-syntax\n".to_owned(),
        "[limits]\nrunning = 'secret-schema'\n".to_owned(),
        valid.replace("running = 3", "running = 0"),
    ] {
        fs::write(board.config_path(), &text).unwrap();
        assert!(!board.reload_config_if_changed().unwrap());
        assert!(!worker.reload_config_if_changed().unwrap());
        assert_eq!(board.config, previous);
        assert_eq!(worker.config, previous);
        let first = rx.try_recv().unwrap();
        let second = rx.try_recv().unwrap();
        assert_eq!(first.source, "Config not reloaded");
        assert_eq!(first.text, second.text);
        assert!(first.text.contains("keeping previous config"));
        assert!(!first.text.contains("secret"));
        assert!(!board.reload_config_if_changed().unwrap());
        assert!(rx.try_recv().is_err());
    }
    let invalid = board
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "config_invalid")
        .count();
    assert_eq!(invalid, 3);
    assert!(changed(&board).is_empty());
    fs::write(
        board.config_path(),
        valid.replace("running = 3", "running = 10"),
    )
    .unwrap();
    assert!(board.reload_config_if_changed().unwrap());
    assert_eq!(change(&changed(&board)[0], "limits.running")["old"], 3);
    assert_eq!(board.config.limits.running, 10);
    let log = fs::read_to_string(board.dir().join("events.jsonl")).unwrap();
    assert!(!log.contains("secret-syntax") && !log.contains("secret-schema"));
}

#[test]
fn non_pm_config_still_emits_for_wait_but_is_not_pm_actionable() {
    let root = tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    store
        .edit_config(|doc| doc.set_str("ui.mouse", "false"))
        .unwrap();
    let events = changed(&store);
    assert_eq!(events[0].detail["pm_relevant"], false);
    assert_eq!(actionable_kind(&events[0]), None);
}

#[test]
fn invalid_config_open_records_safe_event_without_overwriting_last_good_snapshot() {
    let root = tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    fs::write(store.config_path(), "[limits]\nrunning = 'secret'").unwrap();
    assert!(Store::open(root.path()).is_err());
    assert!(Store::open(root.path()).is_err());
    let events = store.events().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "config_invalid");
    assert!(!events[0].detail.to_string().contains("secret"));
}

#[test]
fn settings_repair_observes_pending_invalid_file_without_logging_it_as_valid() {
    let root = tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let (tx, rx) = mpsc::channel();
    let _sink = capture_messages(tx);
    external(&store, "limits.running", "0");
    store
        .edit_config_from(ConfigSource::SettingsUi, |doc| {
            doc.set_str("limits.running", "11")
        })
        .unwrap();
    let events = store.events().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind, "config_invalid");
    assert_eq!(events[1].kind, "config_changed");
    assert_eq!(events[1].detail["source"], "settings_ui");
    assert_eq!(change(&events[1], "limits.running")["old"], 3);
    assert_eq!(change(&events[1], "limits.running")["new"], 11);
    assert!(
        rx.try_recv()
            .unwrap()
            .text
            .contains("keeping previous config")
    );
    assert_eq!(store.config.limits.running, 3);
}
