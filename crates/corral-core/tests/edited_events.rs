use corral_core::{Actor, Event, Run, Status, Store, TaskSize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn edits(store: &Store) -> Vec<Event> {
    store
        .events()
        .unwrap()
        .into_iter()
        .filter(|event| event.kind == "edited")
        .collect()
}

fn short_sha(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))[..12].into()
}

#[test]
fn spec_fields_are_logged_together_by_every_save_entry_point() {
    for entry in ["save", "update", "try_update", "dispatch_update"] {
        // Given an existing task and process attribution.
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        store.set_attribution(Actor::Owner, "cli").unwrap();
        let mut task = store
            .create("Before", None, Status::Backlog, "old body\n")
            .unwrap();
        let change = |task: &mut corral_core::Task| {
            task.title = "After".into();
            task.body = "new body\nsecond line\n".into();
            task.project = Some("corral".into());
            task.priority = "P1".into();
            task.depends_on = vec!["T-002".into()];
            task.labels = vec!["history".into()];
            task.kind = Some("backend".into());
            task.size = Some(TaskSize::M);
            task.profile = Some("strong".into());
            task.design = true;
            Ok(())
        };

        // When one write changes every spec field.
        match entry {
            "save" => {
                change(&mut task).unwrap();
                store.save(&task).unwrap();
            }
            "update" => {
                store.update(&task.id, change).unwrap();
            }
            "try_update" => {
                store
                    .try_update(&task.id, |task| {
                        change(task)?;
                        Ok(true)
                    })
                    .unwrap();
            }
            "dispatch_update" => {
                store
                    .dispatch_update(&task.id, |_, task| change(task))
                    .unwrap();
            }
            _ => unreachable!(),
        }

        // Then exactly one attributed event contains the complete bounded diff.
        let events = edits(&store);
        assert_eq!(events.len(), 1, "{entry}");
        let event = &events[0];
        assert_eq!(event.task.as_deref(), Some(task.id.as_str()));
        assert_eq!(event.actor, Some(Actor::Owner));
        assert_eq!(event.via.as_deref(), Some("cli"));
        assert_eq!(
            event.detail,
            json!({"fields": {
                "title": {"from": "Before", "to": "After"},
                "project": {"from": null, "to": "corral"},
                "priority": {"from": "P2", "to": "P1"},
                "depends": {"from": [], "to": ["T-002"]},
                "labels": {"from": [], "to": ["history"]},
                "kind": {"from": null, "to": "backend"},
                "size": {"from": null, "to": "m"},
                "profile": {"from": null, "to": "strong"},
                "design": {"from": false, "to": true},
                "body": {
                    "lines_from": 1, "lines_to": 2,
                    "sha_from": short_sha("old body\n"),
                    "sha_to": short_sha("new body\nsecond line\n"),
                },
            }})
        );
    }
}

#[test]
fn runtime_writes_and_unchanged_saves_do_not_log_edits() {
    // Given a task created with a body lacking a final newline and an existing run.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let mut task = store
        .create("Runtime", None, Status::Backlog, "body without newline")
        .unwrap();
    let run: Run = serde_json::from_value(json!({
        "id": "R1", "agent": "claude", "worktree": "/unused",
        "branch": "corral/T-001", "base": "main", "base_sha": "abc",
        "started": "2026-10-18T00:00:00Z"
    }))
    .unwrap();
    task.runs.push(run);
    store.save(&task).unwrap();

    // When lifecycle, run, artifact and timestamp-only writes persist.
    store.save(&task).unwrap();
    store.update(&task.id, |_| Ok(())).unwrap();
    store
        .update(&task.id, |task| {
            task.status = Status::Review;
            task.queued = true;
            task.runs[0].summary = Some("finished".into());
            task.artifacts.push(serde_json::from_value(json!({
                "run": "R1", "kind": "report", "title": "Report",
                "path": "artifacts/T-001/R1/report.md", "source": "/unused/report.md",
                "sha256": "a".repeat(64), "bytes": 10,
                "added": "2026-10-18T00:00:00Z"
            }))?);
            Ok(())
        })
        .unwrap();
    store.try_update(&task.id, |_| Ok(true)).unwrap();
    store.dispatch_update(&task.id, |_, _| Ok(())).unwrap();

    // Then the creation is retained, but none of these writes is a spec edit.
    assert!(edits(&store).is_empty());
    assert_eq!(store.events().unwrap()[0].kind, "created");
    let saved = store.get(&task.id).unwrap();
    assert_eq!(saved.status, Status::Review);
    assert_eq!(saved.runs[0].summary.as_deref(), Some("finished"));
    assert_eq!(saved.artifacts.len(), 1);
}

#[test]
fn long_scalars_and_list_values_are_bounded_without_losing_suffix_edits() {
    // Given scalar and list values whose only differences will follow character 80.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let prefix = "\u{754c}".repeat(80);
    let task = store
        .create_initialized(
            &format!("{prefix}before"),
            None,
            Status::Backlog,
            "private original body\n",
            |task| {
                task.profile = Some(format!("{prefix}before"));
                task.labels = vec![format!("{prefix}before")];
                Ok(())
            },
        )
        .unwrap();

    // When the hidden suffix and private body text change.
    store
        .update(&task.id, |task| {
            task.title = format!("{prefix}after");
            task.profile = Some(format!("{prefix}after"));
            task.labels = vec![format!("{prefix}after")];
            task.body = "private replacement body\n\nlast line".into();
            Ok(())
        })
        .unwrap();

    // Then every changed field remains present, with no scalar over 80 characters.
    let events = edits(&store);
    assert_eq!(events.len(), 1);
    let fields = &events[0].detail["fields"];
    for field in ["title", "profile"] {
        assert_eq!(fields[field], json!({"from": prefix, "to": prefix}));
        assert_eq!(fields[field]["to"].as_str().unwrap().chars().count(), 80);
    }
    assert_eq!(fields["labels"], json!({"from": [prefix], "to": [prefix]}));
    assert_eq!(
        fields["body"],
        json!({
            "lines_from": 1, "lines_to": 3,
            "sha_from": short_sha("private original body\n"),
            "sha_to": short_sha("private replacement body\n\nlast line\n"),
        })
    );
    let log = std::fs::read_to_string(store.dir().join("events.jsonl")).unwrap();
    assert!(!log.contains("private original body"));
    assert!(!log.contains("private replacement body"));
}

#[test]
fn rejected_or_skipped_writes_do_not_log_edits() {
    // Given an owner task without permission to replace its spec.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let task = store
        .create_with_origin(
            "Protected",
            None,
            Status::Idea,
            "original\n",
            corral_core::Origin::Owner,
        )
        .unwrap();

    // When a protected edit is rejected or a compare-and-swap declines to save.
    let rejected = store.update(&task.id, |task| {
        task.title = "Unauthorized".into();
        Ok(())
    });
    let skipped = store
        .try_update(&task.id, |task| {
            task.title = "Skipped".into();
            Ok(false)
        })
        .unwrap();

    // Then neither attempt becomes history or modifies the task.
    assert!(rejected.is_err());
    assert!(skipped.is_none());
    assert!(edits(&store).is_empty());
    assert_eq!(store.get(&task.id).unwrap(), task);
}

#[test]
fn direct_file_edits_have_no_recoverable_before_state_after_reopen() {
    // Given an unobserved direct task-file replacement.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let mut task = store
        .create("Original", None, Status::Backlog, "before\n")
        .unwrap();
    task.title = "Hand edited".into();
    task.body = "after\n".into();
    std::fs::write(&task.path, corral_core::render_task(&task)).unwrap();

    // When a new process opens the already-edited file and makes a runtime update.
    let reopened = Store::open(dir.path()).unwrap();
    reopened
        .update(&task.id, |task| {
            task.status = Status::Running;
            Ok(())
        })
        .unwrap();

    // Then it sees the edit but cannot fabricate a before-state or a field diff.
    assert_eq!(reopened.get(&task.id).unwrap().title, "Hand edited");
    assert!(edits(&reopened).is_empty());
    assert_eq!(
        reopened.events().unwrap()[0].detail["title"],
        Value::from("Original")
    );
}
