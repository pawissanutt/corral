use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

const FIRST: &str = "2026-10-14T10:00:00Z";
const LAST: &str = "2026-10-15T10:00:00Z";

fn changed(ts: &str, source: &str, old: Value, new: Value) -> Value {
    json!({
        "ts": ts,
        "kind": "config_changed",
        "detail": {
            "source": source,
            "hash": "a".repeat(64),
            "pm_relevant": true,
            "changes": [{
                "key": "limits.running", "old": old, "new": new,
                "pm_relevant": true, "applies": "new_dispatches"
            }],
            "message": "Existing running agents keep their dispatch settings."
        }
    })
}

fn invalid() -> Value {
    json!({
        "ts": LAST, "kind": "config_invalid",
        "detail": {
            "source": "file_edit", "hash": "b".repeat(64),
            "error": "limits.running must be above 0",
            "message": "Existing running agents keep their dispatch settings."
        }
    })
}

fn workspace(events: &[Value]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join(".corral");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("corral.toml"), "[invalid toml").unwrap();
    let log: String = events.iter().map(|event| format!("{event}\n")).collect();
    std::fs::write(dir.join("events.jsonl"), log).unwrap();
    root
}

fn corral(root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
    for (name, _) in std::env::vars_os() {
        let text = name.to_string_lossy();
        if text.starts_with("CORRAL_") || text.starts_with("HERDR_") {
            command.env_remove(name);
        }
    }
    command
        .env("CORRAL_ROOT", root)
        .current_dir(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn ok(root: &Path, args: &[&str]) -> String {
    let out = corral(root, args);
    assert!(
        out.status.success(),
        "corral {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn diff_defaults_to_last_changed_record_despite_invalid_current_config() {
    // Given multiple changes followed by an invalid edit and an unrelated event.
    let latest = changed(LAST, "settings_ui", json!(3), json!(5));
    let root = workspace(&[
        changed(FIRST, "cli", json!(2), json!(3)),
        latest.clone(),
        invalid(),
        json!({"ts": LAST, "kind": "created"}),
    ]);
    let log = std::fs::read(root.path().join(".corral/events.jsonl")).unwrap();

    // When reading the default diff.
    let output = ok(root.path(), &["config", "diff", "--json"]);

    // Then only the last changed event is returned, without touching config or the log.
    assert_eq!(
        serde_json::from_str::<Value>(&output).unwrap(),
        json!([latest])
    );
    assert_eq!(
        std::fs::read(root.path().join(".corral/events.jsonl")).unwrap(),
        log
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join(".corral/corral.toml")).unwrap(),
        "[invalid toml"
    );
}

#[test]
fn diff_since_is_inclusive_and_compares_timestamp_offsets() {
    // Given changes before, at and after the cutoff, plus an invalid event.
    let at = changed(FIRST, "file_edit", json!(null), json!({"nested": true}));
    let after = changed(LAST, "cli", json!(["a"]), json!(null));
    let root = workspace(&[
        changed("2026-10-14T09:59:59Z", "cli", json!(1), json!(2)),
        at.clone(),
        invalid(),
        after.clone(),
    ]);

    // When the cutoff names the same instant with a different offset.
    let output = ok(
        root.path(),
        &[
            "config",
            "diff",
            "--since",
            "2026-10-14T12:00:00+02:00",
            "--json",
        ],
    );

    // Then both matching changed records retain their complete semantic JSON.
    assert_eq!(
        serde_json::from_str::<Value>(&output).unwrap(),
        json!([at, after])
    );
}

#[test]
fn diff_text_shows_semantic_values_timestamp_and_source() {
    // Given a recorded semantic change.
    let root = workspace(&[changed(LAST, "settings_ui", json!(3), json!(5))]);

    // When reading the human-readable diff.
    let output = ok(root.path(), &["config", "diff"]);

    // Then the output identifies the change and when it applies.
    for field in [
        LAST,
        "source=settings_ui",
        "limits.running: 3 -> 5",
        "applies=new_dispatches",
        "pm_relevant=true",
    ] {
        assert!(output.contains(field), "{output}");
    }
}

#[test]
fn history_json_includes_changes_and_invalid_diagnostics_in_log_order() {
    // Given config and non-config records.
    let change = changed(FIRST, "file_edit", json!(2), json!(3));
    let diagnostic = invalid();
    let root = workspace(&[
        change.clone(),
        json!({"ts": FIRST, "kind": "created"}),
        diagnostic.clone(),
    ]);

    // When reading history as JSON with invalid current TOML.
    let output = ok(root.path(), &["config", "history", "--json"]);

    // Then config records include their original hashes and diagnostic details.
    assert_eq!(
        serde_json::from_str::<Value>(&output).unwrap(),
        json!([change, diagnostic])
    );
}

#[test]
fn history_text_includes_invalid_error_and_hash() {
    // Given an invalid edit.
    let root = workspace(&[invalid()]);

    // When reading human-readable history.
    let output = ok(root.path(), &["config", "history"]);

    // Then timestamp, kind, source, hash and diagnostic are visible.
    for field in [
        LAST,
        "config_invalid",
        "source=file_edit",
        &"b".repeat(64),
        "limits.running must be above 0",
    ] {
        assert!(output.contains(field), "{output}");
    }
}

#[test]
fn readers_return_empty_arrays_when_log_is_missing_or_filter_has_no_matches() {
    // Given no event log and invalid TOML.
    let root = workspace(&[]);
    std::fs::remove_file(root.path().join(".corral/events.jsonl")).unwrap();

    // When reading either command or a future cutoff.
    for args in [
        vec!["config", "diff", "--json"],
        vec!["config", "history", "--json"],
        vec!["config", "diff", "--since", LAST, "--json"],
    ] {
        let output = ok(root.path(), &args);

        // Then absence is a successful empty array, not a config parsing failure.
        assert_eq!(serde_json::from_str::<Value>(&output).unwrap(), json!([]));
    }
}

#[test]
fn diff_since_rejects_non_timestamp_input() {
    // Given a workspace.
    let root = workspace(&[]);

    // When the cutoff is not RFC3339.
    let output = corral(root.path(), &["config", "diff", "--since", "yesterday"]);

    // Then clap rejects it before reading events.
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn history_drains_past_reader_budget_and_ignores_incomplete_tail() {
    // Given oversized malformed data before a valid event and an incomplete tail.
    let event = changed(LAST, "cli", json!(false), json!(true));
    let root = workspace(&[]);
    std::fs::write(
        root.path().join(".corral/events.jsonl"),
        format!("{}\n{event}\n{{\"ts\":", "x".repeat(128 * 1024)),
    )
    .unwrap();

    // When reading history.
    let output = ok(root.path(), &["config", "history", "--json"]);

    // Then budget exhaustion is not mistaken for EOF.
    assert_eq!(
        serde_json::from_str::<Value>(&output).unwrap(),
        json!([event])
    );
}
