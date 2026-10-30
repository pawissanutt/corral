use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use corral_core::{Status, Store};
use serde_json::{Value, json};

struct Fixture {
    dir: tempfile::TempDir,
    store: Store,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        Self { dir, store }
    }

    fn run(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("CORRAL_")
                || key.to_string_lossy().starts_with("HERDR_")
                || key.to_string_lossy().starts_with("XDG_")
            {
                command.env_remove(key);
            }
        }
        for key in [
            "HOME",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
            "XDG_CACHE_HOME",
            "XDG_RUNTIME_DIR",
        ] {
            command.env(key, self.dir.path().join("private"));
        }
        command
            .env("HERDR_BIN_PATH", self.dir.path().join("no-herdr"))
            .env("CORRAL_ROOT", self.dir.path())
            .current_dir(self.dir.path())
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        serde_json::from_str(&self.ok(args)).unwrap()
    }
}

fn ids(board: &Value) -> BTreeSet<&str> {
    board["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| task["task"].as_str().unwrap())
        .collect()
}

fn snapshot(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(path, path, &mut files);
    files
}

#[test]
fn default_and_list_emit_flattened_version_one_assessments() {
    // Given an unfinished task with a canonical status time.
    let fx = Fixture::new();
    let task = fx
        .store
        .create("owner choice", None, Status::NeedsMe, "")
        .unwrap();
    // When invoking both forms through the compiled binary.
    let default = fx.json(&["attention", "--json"]);
    let explicit = fx.json(&["attention", "list", "--json"]);
    // Then both forms expose the same task and stable condition identity.
    assert_eq!(default["version"], 1);
    assert_eq!(explicit["version"], 1);
    assert_eq!(ids(&default), BTreeSet::from([task.id.as_str()]));
    assert_eq!(ids(&default), ids(&explicit));
    let row = &default["tasks"][0];
    assert_eq!(row["title"], "owner choice");
    assert!(row.get("project").is_some());
    assert!(row.get("status").is_some());
    assert!(row.get("assessment").is_none());
    assert!(row.get("progress").is_some());
    let condition = &row["conditions"][0];
    assert_eq!(row["primary"], *condition);
    for field in [
        "key",
        "kind",
        "class",
        "reason_code",
        "reason",
        "responsible",
        "since",
        "since_source",
        "age_secs",
        "resume",
        "next_action",
    ] {
        assert!(condition.get(field).is_some(), "missing {field}");
    }
    assert_eq!(condition["since_source"], "canonical");
    assert!(condition["since"].is_string());
    assert!(condition["age_secs"].as_i64().is_some_and(|age| age >= 0));
    assert_eq!(
        condition["key"],
        explicit["tasks"][0]["conditions"][0]["key"]
    );
    for check in condition["resume"].as_array().unwrap() {
        assert!(check["machine_checkable"].is_boolean());
        assert!(check.get("satisfied").is_some());
    }
    assert!(condition["next_action"].get("command").is_some());
    assert!(condition["next_action"]["description"].is_string());
}

#[test]
fn project_is_exact_and_all_includes_both_terminal_statuses() {
    // Given two similarly named projects and all terminal statuses.
    let fx = Fixture::new();
    let active = fx
        .store
        .create("active", Some("alpha"), Status::NeedsMe, "")
        .unwrap();
    fx.store
        .create("other", Some("alphabet"), Status::NeedsMe, "")
        .unwrap();
    fx.store
        .create("unassigned", None, Status::NeedsMe, "")
        .unwrap();
    let done = fx
        .store
        .create("finished", Some("alpha"), Status::Done, "")
        .unwrap();
    let dropped = fx
        .store
        .create("dropped", Some("alpha"), Status::Dropped, "")
        .unwrap();
    // When filtering with flags on either side of the optional list command.
    let normal = fx.json(&["attention", "--project", "alpha", "list", "--json"]);
    let all = fx.json(&["attention", "list", "--json", "--all", "--project", "alpha"]);
    let unfiltered = fx.json(&["attention", "--json"]);
    // Then project matching is exact and only --all exposes terminal tasks.
    assert_eq!(ids(&normal), BTreeSet::from([active.id.as_str()]));
    assert_eq!(
        ids(&all),
        BTreeSet::from([active.id.as_str(), done.id.as_str(), dropped.id.as_str()])
    );
    assert_eq!(ids(&unfiltered).len(), 3);
    assert!(!ids(&unfiltered).contains(done.id.as_str()));
    assert!(!ids(&unfiltered).contains(dropped.id.as_str()));
}

#[test]
fn human_output_has_one_row_per_condition_bearing_task() {
    // Given one task with overlapping delivery and stuck-process conditions.
    let fx = Fixture::new();
    let task = fx
        .store
        .create("waiting owner", None, Status::NeedsMe, "")
        .unwrap();
    fx.store
        .update(&task.id, |task| {
            task.runs.push(serde_json::from_value(json!({
                "id": "R1", "agent": "worker", "worktree": "/not-read",
                "branch": "unused", "base": "main", "base_sha": "unused",
                "started": "2026-01-01T00:00:00Z",
                "pending_reply": "retained owner reply",
                "stuck": {"pid": 123, "argv": ["foreign"], "started": "100"}
            }))?);
            Ok(())
        })
        .unwrap();
    let progressing = fx
        .store
        .create("live worker", None, Status::Running, "")
        .unwrap();
    fx.store
        .update(&progressing.id, |task| {
            task.runs.push(serde_json::from_value(json!({
                "id": "R1", "agent": "worker", "worktree": "/not-read",
                "branch": "unused", "base": "main", "base_sha": "unused",
                "started": "2026-01-01T00:00:00Z", "agent_state": "working"
            }))?);
            Ok(())
        })
        .unwrap();
    let finished = fx.store.create("done", None, Status::Done, "").unwrap();
    // When requesting both machine and human projections.
    let board = fx.json(&["attention", "--json"]);
    let human = fx.ok(&["attention", "list"]);
    // Then all keys survive JSON while human output has one line for their task.
    let rows = board["tasks"].as_array().unwrap();
    let waiting = rows.iter().find(|row| row["task"] == task.id).unwrap();
    let progress = rows
        .iter()
        .find(|row| row["task"] == progressing.id)
        .unwrap();
    assert!(progress["progress"].is_object());
    assert!(progress["conditions"].as_array().unwrap().is_empty());
    let conditions = waiting["conditions"].as_array().unwrap();
    assert!(conditions.len() >= 2, "{conditions:?}");
    let primary: corral_core::attention::Condition =
        serde_json::from_value(waiting["primary"].clone()).unwrap();
    assert!(human.contains(primary.class.label()));
    assert!(human.contains(primary.responsible.label()));
    assert!(human.contains(primary.since_source.label()));
    assert!(
        human.contains(
            &primary
                .reason
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        )
    );
    if let Some(command) = primary.next_action.command {
        assert!(human.contains(&command));
    }
    let keys: BTreeSet<_> = conditions
        .iter()
        .filter_map(|condition| condition["key"].as_str())
        .collect();
    assert_eq!(keys.len(), conditions.len());
    assert_eq!(
        human
            .lines()
            .filter(|line| line.starts_with(&task.id))
            .count(),
        1
    );
    assert!(!human.contains(&finished.id));
    assert!(!human.contains(&progressing.id));
    assert_eq!(human.lines().count(), 1);
}

#[test]
fn attention_does_not_mutate_tasks_config_events_or_inbox_state() {
    // Given a store with tasks and a changed but valid workspace configuration.
    let fx = Fixture::new();
    fx.store
        .create("waiting", None, Status::NeedsMe, "")
        .unwrap();
    let config = fx.store.config_path();
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str("\n# attention must not record configuration observation\n");
    std::fs::write(&config, text).unwrap();
    let state = fx.dir.path().join(".corral");
    let before = snapshot(&state);
    // When listing all assessments without a terminal server.
    fx.ok(&["attention", "list", "--json", "--all"]);
    // Then the entire durable store remains byte-for-byte unchanged.
    assert_eq!(before, snapshot(&state));
}

#[test]
fn read_failure_exits_nonzero_without_success_output() {
    // Given invalid workspace configuration.
    let fx = Fixture::new();
    std::fs::write(fx.store.config_path(), "[invalid").unwrap();
    // When requesting the human assessment.
    let output = fx.run(&["attention"]);
    // Then failure is not presented as a successful empty assessment.
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

#[test]
fn legacy_missing_status_time_stays_unknown_without_a_durable_discovery() {
    // Given a legacy task with no canonical status timestamp or attention record.
    let fx = Fixture::new();
    let task = fx
        .store
        .create("legacy choice", None, Status::NeedsMe, "")
        .unwrap();
    let text = std::fs::read_to_string(&task.path).unwrap();
    let text = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("status_since:"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&task.path, text).unwrap();
    assert!(
        Store::open_unobserved(fx.dir.path())
            .unwrap()
            .get(&task.id)
            .unwrap()
            .status_since
            .is_none()
    );
    let before = snapshot(fx.store.dir());
    // When reading the legacy assessment twice.
    let first = fx.json(&["attention", "--json"]);
    let second = fx.json(&["attention", "list", "--json"]);
    // Then neither invocation invents an age base or writes first discovery.
    for board in [&first, &second] {
        let condition = &board["tasks"][0]["primary"];
        assert_eq!(condition["since_source"], "discovered");
        assert!(condition["since"].is_null());
        assert!(condition["age_secs"].is_null());
    }
    assert_eq!(before, snapshot(fx.store.dir()));
}
