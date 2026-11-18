use std::{fs, path::Path, process::Command};

use corral_engine::{Engine, EventKind, Priority, Status};
use serde_json::{Value, json};

fn write_task(dir: &Path, id: &str, status: &str, metadata: &str) {
    fs::write(
        dir.join(format!("{id}.md")),
        format!(
            "---\nschema_version: 2\ntask:\n  id: {id}\n  title: Task {id}\n  status: {status}\n  created: 2026-01-01T00:00:00Z\n  updated: 2026-01-02T00:00:00Z\n{metadata}---\n# Goal\n\nTest the import.\n"
        ),
    )
    .unwrap();
}

fn import(root: &Path, dir: &Path, flags: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_corral-next"))
        .arg("--root")
        .arg(root)
        .args(["--json", "import"])
        .arg(dir)
        .args(flags)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn previews_and_imports_a_legacy_export_without_replacing_tasks_or_reusing_ids() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let export = root.join("export");
    fs::create_dir(&export).unwrap();
    let engine = Engine::init(root, None).unwrap();
    let config = engine.state_dir.join("corral.toml");
    let config_text = "[projects.configured]\npath = '.'\n";
    fs::write(&config, config_text).unwrap();
    write_task(
        &export,
        "T-010",
        "idea",
        "  project: missing\n  priority: P1\n  kind: test\n  size: xs\n  parent: T-012\n  plan: T-099@P1\n  depends_on: [T-012, T-021, T-051, T-060]\n",
    );
    write_task(&export, "T-011", "ready", "  project: configured\n");
    write_task(
        &export,
        "T-012",
        "backlog",
        "  tracker: true\n  children: [T-010, T-011]\n",
    );
    write_task(&export, "T-021", "done", "");
    write_task(&export, "T-051", "dropped", "  plan: T-012@P1\n");
    fs::write(export.join("T-054.md"), "---\nnot YAML frontmatter").unwrap();

    let before = engine.store.data_version().unwrap();
    let preview = import(root, &export, &["--dry-run"]);
    assert_eq!(engine.store.data_version().unwrap(), before);
    assert_eq!(engine.store.next_task_number().unwrap(), 1);
    assert_eq!(engine.store.latest_seq().unwrap(), 0);
    assert_eq!(fs::read_to_string(config).unwrap(), config_text);
    assert!(engine.store.get_task("T-010").unwrap().is_none());
    assert_eq!(
        preview["imported"],
        json!({"idea": 1, "backlog": 1, "ready": 1, "done": 0, "dropped": 0})
    );
    assert_eq!(preview["next_id"], 55);
    assert_eq!(preview["skipped_closed"], 2);
    assert_eq!(preview["unconfigured_projects"], json!(["missing"]));
    assert_eq!(preview["dependencies_already_done"], 1);
    assert_eq!(
        preview["dependencies_needing_attention"],
        json!({"T-051": ["T-010"], "T-060": ["T-010"]})
    );
    assert_eq!(preview["unparsable"].as_array().unwrap().len(), 1);
    assert_eq!(preview["unparsable"][0]["file"], "T-054.md");
    assert!(
        preview["unparsable"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("frontmatter")
    );

    assert_eq!(import(root, &export, &[]), preview);
    let task = engine.store.require_task("T-010").unwrap();
    assert_eq!(task.title, "Task T-010");
    assert_eq!(task.body, "# Goal\n\nTest the import.\n");
    assert_eq!(task.project.as_deref(), Some("missing"));
    assert_eq!(task.priority, Priority::P1);
    assert_eq!(task.kind.as_deref(), Some("test"));
    assert_eq!(task.size.as_deref(), Some("xs"));
    assert_eq!(task.parent.as_deref(), Some("T-012"));
    assert_eq!(task.depends_on, ["T-012", "T-051", "T-060"]);
    assert_eq!(task.created.to_string(), "2026-01-01T00:00:00Z");
    assert_eq!(task.updated.to_string(), "2026-01-02T00:00:00Z");
    assert_eq!(
        engine
            .store
            .require_task("T-011")
            .unwrap()
            .parent
            .as_deref(),
        Some("T-012")
    );
    let events = engine.store.list_events("T-010").unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, EventKind::Created);
    assert_eq!(events[0].detail["imported"], true);

    let before = engine.store.data_version().unwrap();
    let repeat = import(root, &export, &[]);
    assert_eq!(repeat["skipped_existing"], 3);
    assert_eq!(
        repeat["imported"],
        json!({"idea": 0, "backlog": 0, "ready": 0, "done": 0, "dropped": 0})
    );
    assert_eq!(repeat["dependencies_already_done"], 0);
    assert_eq!(repeat["next_id"], 55);
    assert_eq!(engine.store.data_version().unwrap(), before);

    assert_eq!(
        engine
            .store
            .transaction(|store| store.next_task_id())
            .unwrap(),
        "T-055"
    );
    write_task(
        &export,
        "T-020",
        "backlog",
        "  depends_on: [T-021, T-051, T-060]\n",
    );
    let all = import(root, &export, &["--all"]);
    assert_eq!(
        all["imported"],
        json!({"idea": 0, "backlog": 1, "ready": 0, "done": 1, "dropped": 1})
    );
    assert_eq!(all["skipped_closed"], 0);
    assert_eq!(all["dependencies_already_done"], 0);
    assert_eq!(
        all["dependencies_needing_attention"],
        json!({"T-060": ["T-020"]})
    );
    assert_eq!(
        engine.store.require_task("T-020").unwrap().depends_on,
        ["T-021", "T-051", "T-060"]
    );
    assert_eq!(all["next_id"], 56);
    assert_eq!(engine.store.next_task_number().unwrap(), 56);
    assert_eq!(
        engine.store.require_task("T-021").unwrap().status,
        Status::Done
    );
    let dropped = engine.store.require_task("T-051").unwrap();
    assert_eq!(dropped.status, Status::Dropped);
    assert_eq!(dropped.parent.as_deref(), Some("T-012"));
}
