use std::path::Path;
use std::process::{Command, Output, Stdio};

use corral_core::{Status, Store};
use serde_json::Value;

const PLAN: &str = "---\ngoal: CLI fixture\nnon_goals: []\ndecisions: []\naffected_paths: [widget.rs]\ncontracts: []\ndependencies: []\nacceptance:\n  - scenario: widget --spin\n    expected: clockwise\nverification: [cargo test]\nowner_decisions: []\n---\n";

fn cli(root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("CORRAL_")
            || name.to_string_lossy().starts_with("HERDR_")
        {
            command.env_remove(name);
        }
    }
    command
        .env("CORRAL_ROOT", root)
        .env("HERDR_BIN_PATH", "/nonexistent/herdr")
        .arg("--root")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn json(root: &Path, args: &[&str]) -> Value {
    let out = cli(root, args);
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn tracker_progress_cli_reads_shared_projection_and_fails_on_broken_graphs() {
    // Given an imported Plan with a dependency and an unmapped acceptance criterion.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let path = tmp.path().join("plan.md");
    std::fs::write(&path, "---\ntitle: Plan\napproved_scope:\n  goal: Feature\n  acceptance: [{criterion: C1, scenario: Combined check, expected: Pass}]\ntasks:\n  - {key: a, title: A}\n  - {key: b, title: B, depends: [a]}\n---\n").unwrap();
    let imported = json(tmp.path(), &["import", path.to_str().unwrap(), "--json"]);
    let id = imported["tracker"]["id"].as_str().unwrap();
    let children = imported["tasks"].as_array().unwrap();
    // When the real CLI reads the Plan twice, including its human-readable form.
    let progress = json(tmp.path(), &["plan", "progress", id, "--json"]);
    let human = cli(tmp.path(), &["plan", "progress", id]);
    // Then the machine contract includes every bucket, prerequisite target, and coverage gap.
    assert!(human.status.success());
    assert_eq!(progress["task"], id);
    assert_eq!(
        progress["counts"],
        serde_json::json!({
            "accepted":0, "running":0, "review":0, "waiting":0, "missing":2,
        })
    );
    assert_eq!(
        progress["work"][1]["prerequisites"][0]["task"],
        children[0]["id"]
    );
    assert_eq!(progress["work"][1]["prerequisites"][0]["target"], "main");
    assert_eq!(progress["unmapped_criteria"][0]["criterion"], "C1");
    assert_eq!(store.get(id).unwrap().status, Status::Backlog);

    // A malformed graph must fail at the same real CLI boundary, even without a grant.
    store
        .update(children[0]["id"].as_str().unwrap(), |task| {
            task.depends_on
                .push(children[1]["id"].as_str().unwrap().into());
            Ok(())
        })
        .unwrap();
    let cycle = cli(tmp.path(), &["plan", "progress", id, "--json"]);
    assert!(!cycle.status.success());
    assert!(String::from_utf8_lossy(&cycle.stderr).contains("cycle"));
    std::fs::remove_file(&store.get(children[1]["id"].as_str().unwrap()).unwrap().path).unwrap();
    let missing = cli(tmp.path(), &["plan", "progress", id, "--json"]);
    assert!(!missing.status.success());
}

#[test]
fn plan_cli_preserves_history_and_never_accepts_implementation() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store
        .create("Plan CLI fixture", None, Status::Ready, "Widget")
        .unwrap();
    let path = tmp.path().join("plan.md");
    let notes = tmp.path().join("notes.md");
    std::fs::write(&path, PLAN).unwrap();
    std::fs::write(&notes, "Manual owner approval; no remaining decisions").unwrap();
    let required = json(tmp.path(), &["plan", "policy", &task.id, "required"]);
    assert_eq!(required["plan_required"], true);
    let plan = json(
        tmp.path(),
        &["plan", "submit", &task.id, path.to_str().unwrap(), "--json"],
    );
    assert_eq!(plan["id"], "P1");
    let result = json(
        tmp.path(),
        &[
            "plan",
            "verdict",
            &task.id,
            "approve",
            "--owner",
            "--plan",
            "P1",
            "--notes-file",
            notes.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(result["verdict"], "approve");
    let shown = json(tmp.path(), &["plan", "show", &task.id, "--json"]);
    assert_eq!(
        shown["plans"][0]["artifact"]["sha256"],
        plan["artifact"]["sha256"]
    );
    assert_eq!(shown["plans"][0]["reviews"][0]["reviewer"], "owner");
    assert_eq!(store.get(&task.id).unwrap().status, Status::Ready);
    assert!(store.get(&task.id).unwrap().runs.is_empty());
    assert!(
        !cli(
            tmp.path(),
            &[
                "verdict",
                &task.id,
                "approve",
                "--notes-file",
                notes.to_str().unwrap()
            ]
        )
        .status
        .success()
    );
    assert!(!cli(tmp.path(), &["approve", &task.id]).status.success());
    assert_eq!(store.list().unwrap().len(), 1);
}

#[test]
fn import_amends_existing_plan_at_exact_revision_without_refiling_children() {
    // Given an imported Plan and a separate owner-approved scope amendment.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let path = tmp.path().join("import.md");
    std::fs::write(&path, "---\ntitle: Plan\napproved_scope:\n  goal: Original scope\n  acceptance: [{scenario: Check, expected: Pass}]\ntasks:\n  - {key: a, title: A}\n  - {key: b, title: B, depends: [a]}\n---\nOriginal body\n").unwrap();
    let imported = json(tmp.path(), &["import", path.to_str().unwrap(), "--json"]);
    let id = imported["tracker"]["id"].as_str().unwrap();
    let before = store.get(id).unwrap();
    let amendment = tmp.path().join("scope.md");
    std::fs::write(&amendment, "---\napproved_scope:\n  goal: Amended scope\n  acceptance: [{scenario: Combined check, expected: Pass together}]\n  delivery_target: {kind: integration, branch: feature/plan}\n---\n").unwrap();
    // When the real CLI amends the current scope revision.
    let after = json(
        tmp.path(),
        &[
            "import",
            amendment.to_str().unwrap(),
            "--amend",
            id,
            "--expected-revision",
            "0",
            "--json",
        ],
    );
    // Then one revision retains the original Plan, membership and previous approval.
    assert_eq!(after["id"], id);
    assert_eq!(after["approved_scope_revision"], 1);
    assert_eq!(after["approved_scope"]["goal"], "Amended scope");
    assert_eq!(after["approved_scope_history"][0]["revision"], 0);
    assert_eq!(
        after["approved_scope_history"][0]["approved_scope"]["goal"],
        "Original scope"
    );
    assert_eq!(after["children"], imported["tracker"]["children"]);
    assert_eq!(store.list().unwrap().len(), 3);
    assert_eq!(store.get(id).unwrap().body, before.body);
    let bytes = std::fs::read(&before.path).unwrap();
    let events = std::fs::read(store.dir().join("events.jsonl")).unwrap();
    // A later stale amendment is rejected by the same normal CLI path.
    let stale = cli(
        tmp.path(),
        &[
            "import",
            amendment.to_str().unwrap(),
            "--amend",
            id,
            "--expected-revision",
            "0",
        ],
    );
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("stale Plan scope amendment"));
    assert_eq!(std::fs::read(&before.path).unwrap(), bytes);
    assert_eq!(
        std::fs::read(store.dir().join("events.jsonl")).unwrap(),
        events
    );
    std::fs::write(&amendment, format!("---\napproved_scope:\n  goal: Invalid scope\n  acceptance: [{{scenario: Check, expected: Pass}}]\n  approved_design: {id}@P0\n---\n")).unwrap();
    let invalid = cli(
        tmp.path(),
        &[
            "import",
            amendment.to_str().unwrap(),
            "--amend",
            id,
            "--expected-revision",
            "1",
        ],
    );
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("stale approved design revision"));
    assert_eq!(std::fs::read(&before.path).unwrap(), bytes);
    assert_eq!(
        std::fs::read(store.dir().join("events.jsonl")).unwrap(),
        events
    );
}

#[test]
fn plan_verdict_requires_explicit_target_and_rejects_replaced_target() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store
        .create("Plan identity", None, Status::Ready, "")
        .unwrap();
    let path = tmp.path().join("plan.md");
    let notes = tmp.path().join("notes.md");
    std::fs::write(&path, PLAN).unwrap();
    std::fs::write(&notes, "Old verdict").unwrap();
    for _ in 0..2 {
        json(
            tmp.path(),
            &["plan", "submit", &task.id, path.to_str().unwrap(), "--json"],
        );
    }
    assert!(
        !cli(
            tmp.path(),
            &[
                "plan",
                "verdict",
                &task.id,
                "approve",
                "--notes-file",
                notes.to_str().unwrap()
            ]
        )
        .status
        .success()
    );
    let stale = cli(
        tmp.path(),
        &[
            "plan",
            "verdict",
            &task.id,
            "approve",
            "--owner",
            "--plan",
            "P1",
            "--notes-file",
            notes.to_str().unwrap(),
        ],
    );
    assert!(!stale.status.success());
    assert!(String::from_utf8_lossy(&stale.stderr).contains("replaced"));
    let shown = json(tmp.path(), &["plan", "show", &task.id, "--json"]);
    assert_eq!(shown["plans"].as_array().unwrap().len(), 2);
    assert!(shown["plans"][1].get("reviews").is_none());
}
