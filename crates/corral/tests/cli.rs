use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

struct Ws {
    _tmp: TempDir,
    root: PathBuf,
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn corral(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_corral"))
        .env_remove("CORRAL_ROOT")
        .env_remove("CORRAL_HERDR_SESSION")
        .env("HERDR_BIN_PATH", "/nonexistent/herdr")
        .arg("--root")
        .arg(root)
        .args(args)
        .current_dir(root)
        .output()
        .expect("corral runs")
}

fn ok(root: &Path, args: &[&str]) -> String {
    let out = corral(root, args);
    assert!(
        out.status.success(),
        "corral {args:?} failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn workspace() -> Ws {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().canonicalize().expect("canonical");
    let app = root.join("app");
    std::fs::create_dir_all(&app).expect("mkdir");
    git(&app, &["init", "-q", "-b", "main"]);
    std::fs::write(app.join("README.md"), "hi\n").expect("write");
    git(&app, &["add", "."]);
    git(&app, &["commit", "-q", "-m", "init"]);
    Ws { _tmp: tmp, root }
}

fn init_ws() -> Ws {
    let ws = workspace();
    ok(&ws.root, &["init"]);
    ws
}

#[test]
fn init_lists_projects() {
    let ws = workspace();
    let out = ok(&ws.root, &["init"]);
    assert!(out.contains("app"), "{out}");
    assert!(ws.root.join(".corral/corral.toml").is_file());
}

#[test]
fn new_list_show_move() {
    let ws = init_ws();
    let id = ok(&ws.root, &["new", "Add", "login", "-p", "app"])
        .trim()
        .to_string();
    assert_eq!(id, "T-001");
    let idea = ok(
        &ws.root,
        &[
            "new",
            "Maybe caching",
            "--idea",
            "-p",
            "app",
            "--body",
            "think",
        ],
    );
    assert_eq!(idea.trim(), "T-002");

    let list: serde_json::Value =
        serde_json::from_str(&ok(&ws.root, &["list", "--json"])).expect("json");
    let arr = list.as_array().expect("array");
    assert_eq!(arr.len(), 2);
    assert_eq!(arr[0]["status"], "backlog");
    assert_eq!(arr[1]["status"], "idea");

    let show: serde_json::Value =
        serde_json::from_str(&ok(&ws.root, &["show", "T-001", "--json"])).expect("json");
    assert_eq!(show["title"], "Add login");
    assert_eq!(show["project"], "app");

    ok(&ws.root, &["move", "T-001", "ready"]);
    let show: serde_json::Value =
        serde_json::from_str(&ok(&ws.root, &["show", "1", "--json"])).expect("json");
    assert_eq!(show["status"], "ready");

    let bad = corral(&ws.root, &["move", "T-001", "review"]);
    assert!(!bad.status.success());

    ok(&ws.root, &["move", "T-002", "dropped"]);
    let list: serde_json::Value =
        serde_json::from_str(&ok(&ws.root, &["list", "--json"])).expect("json");
    assert_eq!(list.as_array().expect("array").len(), 1);
    let all: serde_json::Value =
        serde_json::from_str(&ok(&ws.root, &["list", "--all", "--json"])).expect("json");
    assert_eq!(all.as_array().expect("array").len(), 2);
}

#[test]
fn show_displays_gate_result() {
    use corral_core::{GateRecord, GateResult, Run, RunMode, Store};
    let ws = init_ws();
    ok(&ws.root, &["new", "Gated", "-p", "app"]);
    let store = Store::open(&ws.root).expect("store");
    store
        .update("T-001", |t| {
            t.runs.push(Run {
                id: "R1".into(),
                agent: "omo".into(),
                harness: None,
                model: None,
                effort: None,
                mode: RunMode::Interactive,
                worktree: ws.root.join("gone"),
                branch: "corral/T-001".into(),
                base: "main".into(),
                base_sha: "0".repeat(40),
                herdr: None,
                started: jiff::Timestamp::now(),
                ended: None,
                outcome: None,
                summary: None,
                blocked_excerpt: None,
                agent_state: None,
                seen_working: false,
                gate: Some(GateRecord {
                    result: GateResult::Fail,
                    failed_cmd: Some("cargo test --workspace".into()),
                    at: jiff::Timestamp::now(),
                    sha: "abcdef1234567890".into(),
                    pane: None,
                }),
            });
            Ok(())
        })
        .expect("update");
    let out = ok(&ws.root, &["show", "T-001"]);
    let line = out
        .lines()
        .find(|l| l.trim_start().starts_with("gate:"))
        .unwrap_or_else(|| panic!("no gate line in {out}"));
    assert!(line.contains("gate: fail at abcdef1"), "{line}");
    assert!(line.ends_with(": cargo test --workspace"), "{line}");
    let json: serde_json::Value =
        serde_json::from_str(&ok(&ws.root, &["show", "T-001", "--json"])).expect("json");
    assert_eq!(json["runs"][0]["gate"]["result"], "fail");
    assert_eq!(json["runs"][0]["gate"]["sha"], "abcdef1234567890");
}

#[test]
fn artifact_add_and_list() {
    let ws = init_ws();
    ok(&ws.root, &["new", "Task", "-p", "app"]);
    let file = ws.root.join("notes.md");
    std::fs::write(&file, "evidence\n").expect("write");
    ok(
        &ws.root,
        &[
            "artifact",
            "add",
            "T-001",
            file.to_str().expect("utf8"),
            "--kind",
            "report",
        ],
    );
    let rows: serde_json::Value = serde_json::from_str(&ok(
        &ws.root,
        &["artifact", "list", "--task", "T-001", "--json"],
    ))
    .expect("json");
    let rows = rows.as_array().expect("array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["artifact"]["kind"], "report");
}

#[test]
fn plugin_install_print_writes_manifest() {
    let ws = init_ws();
    let out = ok(&ws.root, &["plugin", "install", "--print"]);
    assert!(
        out.contains("prefix+k") && out.contains("corral.board"),
        "{out}"
    );
    let manifest = ws.root.join(".corral/state/herdr-plugin/herdr-plugin.toml");
    let text = std::fs::read_to_string(&manifest).expect("manifest exists");
    let v: toml::Value = toml::from_str(&text).expect("valid toml");
    assert_eq!(v["id"].as_str(), Some("corral"));
    assert_eq!(v["panes"].as_array().map(|a| a.len()), Some(3));
    assert_eq!(v["actions"].as_array().map(|a| a.len()), Some(4));
    let exe = v["events"][0]["command"][0].as_str().expect("exe");
    assert!(Path::new(exe).is_absolute(), "{exe}");
    assert_eq!(v["events"][0]["command"][3].as_str(), Some("hook"));
}

#[test]
fn hook_exits_zero_without_herdr() {
    let ws = init_ws();
    let out = corral(&ws.root, &["hook"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let empty = tempfile::tempdir().expect("tempdir");
    let out = corral(empty.path(), &["hook"]);
    assert!(out.status.success());
}

#[test]
fn skill_install_writes_is_idempotent_and_protects_foreign_files() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    let dir = root.join("skills");
    let dir_s = dir.to_string_lossy().into_owned();
    let printed = ok(root, &["skill"]);
    assert!(printed.starts_with("---\nname: corral\n"));

    assert!(ok(root, &["skill", "install", "--dir", &dir_s]).contains("wrote"));
    let file = dir.join("corral").join("SKILL.md");
    assert_eq!(
        std::fs::read_to_string(&file).expect("skill written"),
        printed
    );
    assert!(ok(root, &["skill", "install", "--dir", &dir_s]).contains("up to date"));

    std::fs::write(&file, "my own skill").expect("write foreign");
    assert!(
        !corral(root, &["skill", "install", "--dir", &dir_s])
            .status
            .success()
    );
    assert_eq!(
        std::fs::read_to_string(&file).expect("kept"),
        "my own skill"
    );
    ok(root, &["skill", "install", "--dir", &dir_s, "--force"]);
    assert_eq!(std::fs::read_to_string(&file).expect("replaced"), printed);
}
