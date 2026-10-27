//! The task commands on the database store (T-320 store-tasks-db): concurrent creation, the
//! revision-checked editor round trip, task summaries in list output and the Markdown export.

use std::path::Path;
use std::process::{Child, Command, Output, Stdio};

use corral_core::{Status, Store};
use serde_json::Value;

fn command(root: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
    for (name, _) in std::env::vars_os() {
        let text = name.to_string_lossy();
        if text.starts_with("CORRAL_") || text.starts_with("HERDR_") {
            command.env_remove(name);
        }
    }
    command
        .env("CORRAL_ROOT", root)
        .env("HERDR_BIN_PATH", "/nonexistent/herdr")
        .env("CORRAL_HERDR_SESSION", "corral-test")
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .arg("--root")
        .arg(root)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn invoke(root: &Path, args: &[&str]) -> Output {
    command(root, args).output().unwrap()
}

fn success(root: &Path, args: &[&str]) -> Output {
    let out = invoke(root, args);
    assert!(out.status.success(), "{args:?}: {out:?}");
    out
}

fn workspace() -> (tempfile::TempDir, Store) {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    (tmp, store)
}

fn editor(root: &Path, body: &str) -> std::path::PathBuf {
    let path = root.join("editor.sh");
    std::fs::write(&path, format!("#!/bin/sh\nset -e\n{body}\n")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn corral_new_from_eight_processes_allocates_unique_ids() {
    let (dir, _store) = workspace();
    let children: Vec<Child> = (0..8)
        .map(|n| {
            command(dir.path(), &["new", &format!("Concurrent {n}")])
                .spawn()
                .unwrap()
        })
        .collect();
    let mut ids: Vec<String> = children
        .into_iter()
        .map(|child| {
            let out = child.wait_with_output().unwrap();
            assert!(out.status.success(), "{out:?}");
            let text = String::from_utf8(out.stdout).unwrap();
            text.split_whitespace()
                .find(|word| word.starts_with("T-"))
                .unwrap()
                .to_owned()
        })
        .collect();
    ids.sort();
    let expected: Vec<String> = (1..=8).map(|n| format!("T-{n:03}")).collect();
    assert_eq!(ids, expected);
    let locks = dir.path().join(".corral/lock");
    let next_id = std::fs::read_dir(&locks)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| entry.file_name().to_string_lossy().starts_with("next-id"));
    assert!(!next_id, "lock/next-id is gone");
}

#[test]
fn edit_stores_the_edited_markdown_when_the_task_did_not_change() {
    let (dir, store) = workspace();
    let task = store
        .create("Original", None, Status::Backlog, "Body\n")
        .unwrap();
    let script = editor(
        dir.path(),
        "grep -q '^# corral edit T-001: task revision' \"$1\"\nsed -i 's/^  title: .*/  title: Edited/' \"$1\"",
    );
    let mut edit = command(dir.path(), &["edit", &task.id]);
    edit.env("EDITOR", &script);
    let out = edit.output().unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(store.get(&task.id).unwrap().title, "Edited");
}

#[test]
fn edit_refuses_a_task_changed_meanwhile_and_keeps_the_edited_text() {
    // Given an editor during which another process comments on the task.
    let (dir, store) = workspace();
    let task = store
        .create("Original", None, Status::Backlog, "Body\n")
        .unwrap();
    let script = editor(
        dir.path(),
        &format!(
            "sed -i 's/^  title: .*/  title: Edited/' \"$1\"\n\"{}\" --root \"{}\" comment {} meanwhile </dev/null",
            env!("CARGO_BIN_EXE_corral"),
            dir.path().display(),
            task.id
        ),
    );
    // When the edit is stored.
    let mut edit = command(dir.path(), &["edit", &task.id]);
    edit.env("EDITOR", &script);
    let out = edit.output().unwrap();
    // Then it is refused, names the kept file, and the concurrent change survives.
    assert!(!out.status.success(), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("task changed while editor was open"),
        "{stderr}"
    );
    let kept = dir.path().join(".corral/state/tmp/edit-T-001.md");
    assert!(stderr.contains(&kept.display().to_string()), "{stderr}");
    assert!(
        std::fs::read_to_string(&kept)
            .unwrap()
            .contains("title: Edited")
    );
    let stored = store.get(&task.id).unwrap();
    assert_eq!(stored.title, "Original");
    assert_eq!(stored.comments.last().unwrap().text, "meanwhile");
    let reopened = Store::open_unobserved(dir.path()).unwrap();
    let draft = reopened
        .record::<Value>("edit-drafts", &task.id)
        .unwrap()
        .unwrap();
    assert_eq!(draft["doc_version"], 1);
    assert_eq!(
        Store::parse_task_markdown(draft["value"]["text"].as_str().unwrap())
            .unwrap()
            .title,
        "Edited"
    );
}

#[test]
fn edit_resumes_failed_editor_draft_after_restart_without_scratch() {
    // Given an editor which changes the draft but exits without applying it.
    let (dir, store) = workspace();
    let task = store
        .create("Original", None, Status::Backlog, "Body\n")
        .unwrap();
    let script = editor(
        dir.path(),
        "sed -i 's/^  title: .*/  title: Recovered/' \"$1\"\nexit 1",
    );
    let out = command(dir.path(), &["edit", &task.id])
        .env("EDITOR", script)
        .output()
        .unwrap();
    assert!(!out.status.success(), "{out:?}");
    std::fs::remove_file(dir.path().join(".corral/state/tmp/edit-T-001.md")).unwrap();
    let script = editor(dir.path(), "exit 0");

    // When another CLI process resumes the edit with no scratch copy.
    let out = command(dir.path(), &["edit", &task.id])
        .env("EDITOR", script)
        .output()
        .unwrap();

    // Then the canonical draft is recovered and removed after successful application.
    assert!(out.status.success(), "{out:?}");
    assert_eq!(store.get(&task.id).unwrap().title, "Recovered");
    assert!(store.record_row("edit-drafts", &task.id).unwrap().is_none());
    assert!(!dir.path().join(".corral/state/edit-T-001.md").exists());
}

#[test]
fn list_returns_summaries_show_the_full_task_and_export_changes_nothing() {
    let (dir, store) = workspace();
    let open = store
        .create("Open task", Some("app"), Status::Backlog, "Open body\n")
        .unwrap();
    let done = store
        .create("Finished task", None, Status::Done, "Done body\n")
        .unwrap();

    let listed: Vec<Value> =
        serde_json::from_slice(&success(dir.path(), &["list", "--json"]).stdout).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["id"], open.id.as_str());
    assert!(listed[0].get("path").is_none(), "{}", listed[0]);
    assert!(listed[0].get("runs").is_none(), "{}", listed[0]);
    let all: Vec<Value> =
        serde_json::from_slice(&success(dir.path(), &["list", "--all", "--json"]).stdout).unwrap();
    let ids: Vec<&str> = all.iter().map(|t| t["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [open.id.as_str(), done.id.as_str()]);

    let shown: Value =
        serde_json::from_slice(&success(dir.path(), &["show", &open.id, "--json"]).stdout).unwrap();
    let stored = serde_json::to_value(store.get(&open.id).unwrap()).unwrap();
    for (field, value) in stored.as_object().unwrap() {
        assert_eq!(&shown[field], value, "{field}");
    }
    assert_eq!(shown["body"], "Open body\n");

    let seq = store.view().unwrap().seq();
    let out_dir = dir.path().join("exported");
    success(dir.path(), &["export", "tasks", out_dir.to_str().unwrap()]);
    assert_eq!(store.view().unwrap().seq(), seq, "export changes nothing");
    for task in [&open, &done] {
        let text = std::fs::read_to_string(out_dir.join(format!("{}.md", task.id))).unwrap();
        assert_eq!(
            Store::parse_task_markdown(&text).unwrap(),
            store.get(&task.id).unwrap()
        );
    }
}

#[test]
fn export_events_streams_stored_records_across_pages_without_mutation() {
    // Given more stored events than one export page, including multiline details.
    let (dir, store) = workspace();
    for index in 0..1001 {
        store
            .log_event(
                Some("T-001"),
                "fixture",
                serde_json::json!({"index": index, "text": "first\nsecond", "run": "R1"}),
            )
            .unwrap();
    }
    let expected = store.events().unwrap();
    let seq = store.view().unwrap().seq();

    // When the real CLI exports the complete event history.
    let output = success(dir.path(), &["export", "events", "--since", "0", "--json"]);

    // Then JSONL preserves every stored event and sequence in ascending order.
    let text = String::from_utf8(output.stdout).unwrap();
    let actual: Vec<corral_core::Event> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(actual, expected);
    assert!(actual.len() > 1000);
    assert!(actual.windows(2).all(|pair| pair[0].seq < pair[1].seq));
    assert_eq!(store.view().unwrap().seq(), seq);
}

#[test]
fn export_events_since_excludes_the_cursor_event() {
    // Given a sequence checkpoint followed by one more stored event.
    let (dir, store) = workspace();
    store
        .log_event(None, "before", serde_json::json!({}))
        .unwrap();
    let seq = store.events_since(None).unwrap().cursor().seq;
    store
        .log_event(None, "after", serde_json::json!({"text":"kept"}))
        .unwrap();
    let expected = store.events().unwrap().pop().unwrap();

    // When export resumes from that checkpoint.
    let output = success(
        dir.path(),
        &["export", "events", "--since", &seq.to_string(), "--json"],
    );

    // Then only the later stored event is emitted, without a wrapper or summary.
    let event: corral_core::Event = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(event, expected);
    assert_eq!(String::from_utf8(output.stdout).unwrap().lines().count(), 1);
}

#[test]
fn export_events_at_tail_emits_nothing() {
    // Given a checkpoint at the current tail.
    let (dir, store) = workspace();
    let seq = store.events_since(None).unwrap().cursor().seq;

    // When there are no events beyond it.
    let output = success(
        dir.path(),
        &["export", "events", "--since", &seq.to_string(), "--json"],
    );

    // Then export succeeds with empty JSONL.
    assert!(output.stdout.is_empty());
}
