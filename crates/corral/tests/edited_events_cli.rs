#![cfg(unix)]

use std::process::{Command, Stdio};

use corral_core::{Origin, Status, Store};
use serde_json::json;
use sha2::{Digest, Sha256};

#[test]
fn edit_command_records_one_title_and_body_diff_without_body_text() {
    // Given a real task and a noninteractive editor replacing its scratch file.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let task = store
        .create_with_origin(
            "Before",
            None,
            Status::Backlog,
            "old secret\n",
            Origin::Owner,
        )
        .unwrap();
    let mut replacement = task.clone();
    replacement.title = "After".into();
    replacement.body = "new secret\nsecond line\n".into();
    let replacement_path = dir.path().join("replacement.md");
    std::fs::write(&replacement_path, corral_core::render_task(&replacement)).unwrap();
    let editor = dir.path().join("editor.sh");
    std::fs::write(&editor, "cp \"$CORRAL_EDIT_REPLACEMENT\" \"$1\"\n").unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_corral"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("CORRAL_")
            || key.to_string_lossy().starts_with("HERDR_")
        {
            cmd.env_remove(key);
        }
    }

    // When the actual CLI edits title and body in one approved operation.
    let out = cmd
        .current_dir(dir.path())
        .env("CORRAL_ROOT", dir.path())
        .env("HERDR_BIN_PATH", dir.path().join("no-herdr"))
        .env("EDITOR", format!("sh {}", editor.display()))
        .env("CORRAL_EDIT_REPLACEMENT", &replacement_path)
        .args(["edit", &task.id, "--owner-ok"])
        .stdin(Stdio::null())
        .output()
        .unwrap();

    // Then the task is saved and one diff contains hashes, never full body text.
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let saved = store.get(&task.id).unwrap();
    assert_eq!(saved.title, replacement.title);
    assert_eq!(saved.body, replacement.body);
    let events: Vec<_> = store
        .events()
        .unwrap()
        .into_iter()
        .filter(|event| event.kind == "edited")
        .collect();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].detail,
        json!({"fields": {
            "title": {"from": "Before", "to": "After"},
            "body": {
                "lines_from": 1, "lines_to": 2,
                "sha_from": format!("{:x}", Sha256::digest(task.body.as_bytes()))[..12],
                "sha_to": format!("{:x}", Sha256::digest(saved.body.as_bytes()))[..12],
            }
        }})
    );
    let log = std::fs::read_to_string(store.dir().join("events.jsonl")).unwrap();
    assert!(!log.contains("old secret"));
    assert!(!log.contains("new secret"));
}
