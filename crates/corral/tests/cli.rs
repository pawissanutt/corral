use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

#[path = "cli/acceptance.rs"]
mod acceptance;
#[path = "cli/log.rs"]
mod log;
#[path = "resource_support/mod.rs"]
mod resource_support;
#[path = "cli/rework.rs"]
mod rework;
#[path = "cli/routing.rs"]
mod routing;
#[path = "cli/routing_override.rs"]
mod routing_override;
#[path = "cli/sync_gate.rs"]
mod sync_gate;

struct Ws {
    _tmp: TempDir,
    root: PathBuf,
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .env("CORRAL_ROOT", dir)
        .env_remove("CORRAL_GATE_ATTEMPT")
        .env_remove("CORRAL_CANDIDATE_HEAD")
        .env_remove("CORRAL_INTEGRATION_BASE")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_WORKSPACE_ID")
        .env_remove("HERDR_TAB_ID")
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
        .arg("--root")
        .arg(root)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
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
    let mut store = corral_core::Store::open(&ws.root).unwrap();
    resource_support::configure(&mut store.config);
    store.save_config().unwrap();
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

    ok(&ws.root, &["move", "T-002", "dropped", "--owner-ok"]);
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
                execution: None,
                delivery: corral_core::inbox::DeliveryMode::Legacy,
                inbox: Default::default(),
                completion: Default::default(),
                id: "R1".into(),
                progress: None,
                checkpoints: vec![],
                checkpoint_required: false,
                agent: "omo".into(),
                profile: None,
                harness: None,
                model: None,
                effort: None,
                route: Some(corral_core::RouteSource::Rule {
                    index: 2,
                    name: Some("backend".into()),
                }),
                vendor: None,
                failures: 0,
                switches: vec![],
                retry: None,
                mode: RunMode::Interactive,
                worktree: ws.root.join("gone"),
                branch: "corral/T-001".into(),
                base: "main".into(),
                base_sha: "0".repeat(40),
                plan_target: None,
                herdr: None,
                started: jiff::Timestamp::now(),
                ended: None,
                outcome: None,
                summary: None,
                blocked_excerpt: None,
                agent_state: None,
                seen_pane: false,
                seen_working: false,
                gate: Some(GateRecord {
                    socket: None,
                    identity: None,
                    commands: vec![],
                    log_sha256: None,
                    result: GateResult::Fail,
                    process_lost: false,
                    failed_cmd: Some("cargo test --workspace".into()),
                    at: jiff::Timestamp::now(),
                    sha: "abcdef1234567890".into(),
                    pane: None,
                }),
                idle_since: None,
                idle_no_report: false,
                herdr_idle_since: None,
                stuck: None,
                pending_reply: None,
                obligations: Default::default(),
                review: None,
                review_runs: Vec::new(),
                usage: None,
                usage_note: None,
                autopilot: None,
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
    assert!(out.contains("route: routing rule 2 (backend)"), "{out}");
    let json: serde_json::Value =
        serde_json::from_str(&ok(&ws.root, &["show", "T-001", "--json"])).expect("json");
    assert_eq!(json["runs"][0]["gate"]["result"], "fail");
    assert_eq!(json["runs"][0]["gate"]["sha"], "abcdef1234567890");
    assert_eq!(json["runs"][0]["route"]["index"], 2);
}

/// A T-001 in Review whose run has a real worktree; `review` is the run's starting review record.
fn review_task(
    ws: &Ws,
    review: Option<corral_core::ReviewRecord>,
) -> (corral_core::Store, PathBuf, String) {
    use corral_core::{Run, RunMode, Status, Store};
    ok(&ws.root, &["new", "Reviewed", "-p", "app"]);
    let app = ws.root.join("app");
    let worktree = app.join(".worktrees").join("T-001");
    let wt = worktree.to_str().expect("utf8");
    git(
        &app,
        &["worktree", "add", "-q", "-b", "corral/T-001", wt, "main"],
    );
    std::fs::write(worktree.join("feature.txt"), "done\n").expect("write");
    git(&worktree, &["add", "-A"]);
    git(&worktree, &["commit", "-q", "-m", "feature"]);
    let head = String::from_utf8(
        Command::new("git")
            .env("CORRAL_ROOT", &ws.root)
            .env_remove("CORRAL_GATE_ATTEMPT")
            .env_remove("CORRAL_CANDIDATE_HEAD")
            .env_remove("CORRAL_INTEGRATION_BASE")
            .env_remove("HERDR_PANE_ID")
            .env_remove("HERDR_WORKSPACE_ID")
            .env_remove("HERDR_TAB_ID")
            .arg("-C")
            .arg(&worktree)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("git runs")
            .stdout,
    )
    .expect("utf8")
    .trim()
    .to_string();
    let store = Store::open(&ws.root).expect("store");
    let review = review.map(|mut r| {
        r.sha = head.clone();
        r
    });
    store
        .update("T-001", |t| {
            t.runs.push(Run {
                execution: None,
                delivery: corral_core::inbox::DeliveryMode::Legacy,
                inbox: Default::default(),
                completion: Default::default(),
                id: "R1".into(),
                progress: None,
                checkpoints: vec![],
                checkpoint_required: false,
                agent: "fake".into(),
                profile: None,
                harness: None,
                model: None,
                effort: None,
                route: None,
                vendor: None,
                failures: 0,
                switches: vec![],
                retry: None,
                mode: RunMode::Interactive,
                worktree: worktree.clone(),
                branch: "corral/T-001".into(),
                base: "main".into(),
                base_sha: head.clone(),
                plan_target: None,
                herdr: None,
                started: jiff::Timestamp::now(),
                ended: None,
                outcome: None,
                summary: Some("feature added".into()),
                blocked_excerpt: None,
                agent_state: None,
                seen_pane: false,
                seen_working: false,
                gate: None,
                idle_since: None,
                idle_no_report: false,
                herdr_idle_since: None,
                stuck: None,
                pending_reply: None,
                obligations: Default::default(),
                review: review.clone(),
                review_runs: Vec::new(),
                usage: None,
                usage_note: None,
                autopilot: None,
            });
            t.status = Status::Review;
            Ok(())
        })
        .expect("update");
    if review.is_some() {
        use sha2::{Digest, Sha256};
        // Capture through the same hermetic CLI boundary used by the verdict.
        // The parent may run inside a real corral gate pane with different inputs.
        let audit: serde_json::Value =
            serde_json::from_str(&ok(&ws.root, &["done", "T-001", "--check", "--json"])).unwrap();
        let identity = serde_json::from_value(audit["identity"].clone()).unwrap();
        store
            .update("T-001", |task| {
                let review = task.current_run_mut().unwrap().review.as_mut().unwrap();
                review.identity = identity;
                review.evidence_digest = Some(format!("{:x}", Sha256::digest(b"[]")));
                Ok(())
            })
            .unwrap();
    }
    (store, worktree, head)
}

fn started_review(reviewer: &str) -> corral_core::ReviewRecord {
    corral_core::ReviewRecord {
        identity: None,
        evidence_digest: None,
        verdict: None,
        process_lost: false,
        reviewer: reviewer.into(),
        notes: None,
        sha: String::new(),
        at: jiff::Timestamp::now(),
        pane: Some("w1:p2".into()),
    }
}

fn show_json(root: &Path) -> serde_json::Value {
    serde_json::from_str(&ok(root, &["show", "T-001", "--json"])).expect("json")
}

fn approve_cli_case(flags: &[&str], branch_kept: bool, worktree_kept: bool) {
    // Given a real completed run in an isolated temporary repository, without herdr panes.
    let ws = init_ws();
    let (store, worktree, _) = review_task(&ws, None);
    let mut args = vec!["approve", "T-001"];
    args.extend_from_slice(flags);
    // When the CLI approves it with the requested flags.
    let output = ok(&ws.root, &args);
    // Then the CLI wiring preserves the requested branch/worktree lifecycle and records it.
    let branch = Command::new("git")
        .env("CORRAL_ROOT", &ws.root)
        .env_remove("CORRAL_GATE_ATTEMPT")
        .env_remove("CORRAL_CANDIDATE_HEAD")
        .env_remove("CORRAL_INTEGRATION_BASE")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_WORKSPACE_ID")
        .env_remove("HERDR_TAB_ID")
        .arg("-C")
        .arg(ws.root.join("app"))
        .args(["show-ref", "--verify", "--quiet", "refs/heads/corral/T-001"])
        .output()
        .expect("git runs");
    assert_eq!(branch.status.success(), branch_kept);
    assert_eq!(worktree.exists(), worktree_kept);
    assert!(ws.root.join("app/feature.txt").is_file());
    let approved = store
        .events()
        .expect("events")
        .into_iter()
        .find(|e| e.kind == "approved")
        .expect("approved event");
    assert_eq!(approved.detail["branch_deleted"], !branch_kept);
    println!(
        "CLI {args:?}: {}branch_exists={branch_kept}, worktree_exists={worktree_kept}, approved={}",
        output, approved.detail
    );
}

#[test]
fn approve_cli_deletes_merged_branch() {
    approve_cli_case(&[], false, false);
}

#[test]
fn approve_cli_keep_worktree_keeps_branch() {
    approve_cli_case(&["--keep-worktree"], true, true);
}

#[test]
fn approve_cli_keep_branch_keeps_only_branch() {
    approve_cli_case(&["--keep-branch"], true, false);
}

#[test]
fn verdict_records_review_notes_artifact_and_event() {
    let ws = init_ws();
    let (store, _worktree, head) = review_task(&ws, Some(started_review("opus-rev")));
    let notes = ws.root.join("notes.md");
    std::fs::write(&notes, "Looks right.\n\n- tests cover it\n").expect("write");
    let out = ok(
        &ws.root,
        &[
            "verdict",
            "T-001",
            "approve",
            "--notes-file",
            notes.to_str().expect("utf8"),
        ],
    );
    assert!(out.contains("approve by opus-rev"), "{out}");

    let review = &show_json(&ws.root)["runs"][0]["review"];
    assert_eq!(review["verdict"], "approve");
    assert_eq!(review["reviewer"], "opus-rev");
    assert_eq!(review["sha"], head.as_str());
    assert_eq!(review["notes"], "Looks right.\n\n- tests cover it\n");
    assert!(review["at"].is_string());

    let task = store.get("T-001").expect("task");
    let report = task
        .artifacts
        .iter()
        .find(|a| a.title == "Review (opus-rev)")
        .expect("review report artifact");
    assert_eq!(report.kind, corral_core::ArtifactKind::Report);
    let saved = std::fs::read_to_string(store.dir().join(&report.path)).expect("snapshot");
    assert!(saved.contains("tests cover it"), "{saved}");
    let verdicts: Vec<_> = store
        .events()
        .expect("events")
        .into_iter()
        .filter(|e| e.kind == "verdict")
        .collect();
    assert_eq!(verdicts.len(), 1);
    assert_eq!(verdicts[0].detail["verdict"], "approve");

    let shown = ok(&ws.root, &["show", "T-001"]);
    assert!(
        shown.contains("review: approve by opus-rev") && shown.contains("Looks right."),
        "{shown}"
    );
}

#[test]
fn autopilot_on_off_status() {
    let ws = init_ws();
    let toml = ws.root.join(".corral/corral.toml");
    let text = format!(
        "# keep me\n{}",
        std::fs::read_to_string(&toml).expect("read")
    );
    std::fs::write(&toml, text).expect("write");

    assert!(ok(&ws.root, &["autopilot"]).starts_with("Mode: Assist"));
    let out = ok(&ws.root, &["autopilot", "off"]);
    assert!(out.contains("Mode: Assist (paused)"), "{out}");
    assert!(
        corral_core::Config::load(&toml)
            .expect("config")
            .automation
            .paused
    );
    let saved = std::fs::read_to_string(&toml).expect("read");
    assert!(
        saved.starts_with("# keep me\n") && saved.contains("paused = true"),
        "{saved}"
    );
    assert!(ok(&ws.root, &["autopilot", "status"]).starts_with("Mode: Assist (paused)"));

    let (store, _worktree, _head) = review_task(&ws, None);
    store
        .update("T-001", |t| {
            if let Some(r) = t.current_run_mut() {
                r.autopilot = Some(corral_core::AutopilotState {
                    stage: corral_core::AutoStage::Blocked,
                    reason: Some("main checkout has uncommitted changes".into()),
                    sha: None,
                    at: jiff::Timestamp::now(),
                });
            }
            Ok(())
        })
        .expect("update");
    let out = ok(&ws.root, &["autopilot", "on"]);
    assert!(
        out.contains("Mode: Assist (paused)") && out.contains("No grant written"),
        "{out}"
    );
    let config = corral_core::Config::load(&toml).expect("config");
    assert!(config.automation.paused);
    assert!(config.automation.authorization.is_none());
    let shown = ok(&ws.root, &["show", "T-001"]);
    assert!(
        shown.contains("autopilot: blocked: main checkout has uncommitted changes"),
        "{shown}"
    );
}

#[test]
fn verdict_without_review_run_is_the_owners() {
    let ws = init_ws();
    review_task(&ws, None);
    ok(
        &ws.root,
        &["verdict", "T-001", "changes", "--notes", "rename foo"],
    );
    let review = &show_json(&ws.root)["runs"][0]["review"];
    assert_eq!(review["verdict"], "changes");
    assert_eq!(review["reviewer"], "owner");

    let empty = corral(&ws.root, &["verdict", "T-001", "approve", "--notes", " "]);
    assert!(!empty.status.success());
    let bogus = corral(&ws.root, &["verdict", "T-001", "maybe", "--notes", "x"]);
    assert!(!bogus.status.success());
}

#[test]
fn verdict_is_refused_when_head_moved_since_the_review_started() {
    let ws = init_ws();
    let (_store, worktree, _head) = review_task(&ws, Some(started_review("opus-rev")));
    std::fs::write(worktree.join("later.txt"), "more\n").expect("write");
    git(&worktree, &["add", "-A"]);
    git(&worktree, &["commit", "-q", "-m", "later"]);

    let out = corral(&ws.root, &["verdict", "T-001", "approve", "--notes", "ok"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("stale review"), "{err}");
    let review = &show_json(&ws.root)["runs"][0]["review"];
    assert!(review["verdict"].is_null(), "{review}");
}

#[test]
fn verdict_on_a_dirty_worktree_records_escalate_instead() {
    let ws = init_ws();
    let (_store, worktree, _head) = review_task(&ws, Some(started_review("opus-rev")));
    std::fs::write(worktree.join("feature.txt"), "edited by reviewer\n").expect("write");

    let out = corral(
        &ws.root,
        &["verdict", "T-001", "approve", "--notes", "fine"],
    );
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("reviewer must not edit files"), "{err}");
    let review = &show_json(&ws.root)["runs"][0]["review"];
    assert_eq!(review["verdict"], "escalate");
    assert_eq!(review["reviewer"], "opus-rev");
    let notes = review["notes"].as_str().expect("notes");
    assert!(
        notes.starts_with("escalate: reviewer modified the worktree"),
        "{notes}"
    );
    assert!(
        notes.contains("feature.txt") && notes.contains("fine"),
        "{notes}"
    );
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
    // The board key goes back to the running board instead of opening a second one.
    assert_eq!(v["actions"][0]["id"].as_str(), Some("board"));
    assert_eq!(v["actions"][0]["command"][3].as_str(), Some("focus-board"));
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

#[test]
fn config_get_set_validate() {
    let ws = init_ws();
    let path = ws.root.join(".corral/corral.toml");
    assert_eq!(
        ok(&ws.root, &["config", "path"]).trim(),
        path.to_str().unwrap()
    );
    assert_eq!(
        ok(&ws.root, &["config", "get", "limits.running"]).trim(),
        "3"
    );

    let original = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, format!("# keep me\n{original}")).unwrap();
    let out = ok(&ws.root, &["config", "set", "limits.running", "5"]);
    assert_eq!(out.trim(), "limits.running = 5");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("# keep me\n"), "{text}");
    assert_eq!(
        ok(&ws.root, &["config", "get", "limits.running"]).trim(),
        "5"
    );
    ok(&ws.root, &["config", "set", "defaults.profile", "claude"]);
    assert_eq!(
        ok(&ws.root, &["config", "get", "defaults.profile"]).trim(),
        "claude"
    );
    ok(&ws.root, &["config", "validate"]);

    let before = std::fs::read_to_string(&path).unwrap();
    let rejected = corral(&ws.root, &["config", "set", "defaults.profile", "nobody"]);
    assert!(!rejected.status.success());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

    std::fs::write(
        &path,
        format!("{before}\n[projects.ghost]\npath = \"gone\"\ndefault_profile = \"nobody\"\n"),
    )
    .unwrap();
    let out = corral(&ws.root, &["config", "validate"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("invalid config (2 problems):"), "{err}");
    assert!(
        err.contains(&format!(
            "\n  - {}: projects.ghost.default_profile: unknown profile 'nobody'",
            path.display()
        )),
        "{err}"
    );
    assert!(
        err.contains(&format!("\n  - {}: projects.ghost.path: ", path.display())),
        "{err}"
    );

    std::fs::write(&path, "[limits\nrunning = 3\n").unwrap();
    let out = corral(&ws.root, &["config", "validate"]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("line 1"), "{err}");
}

#[test]
fn list_done_since_keeps_recent_finishes_newest_first() {
    use corral_core::{Status, Store};
    use std::io::Write;
    // Given T-001 approved three days ago, T-002 forced to done now, T-003 open and T-004 dropped.
    let ws = init_ws();
    for title in ["Old landing", "Fresh landing", "Still open", "Dropped idea"] {
        ok(&ws.root, &["new", title, "-p", "app"]);
    }
    let store = Store::open(&ws.root).expect("store");
    store
        .update("T-001", |t| {
            t.status = Status::Done;
            Ok(())
        })
        .expect("update");
    let three_days_ago = jiff::Timestamp::now() - jiff::SignedDuration::from_hours(72);
    let approved = serde_json::json!({
        "ts": three_days_ago.to_string(),
        "task": "T-001",
        "kind": "approved",
        "detail": {"run": "R1", "sha": "1a2b3c4d5e6f7a8b", "strategy": "rebase"},
    });
    let mut events = std::fs::OpenOptions::new()
        .append(true)
        .open(ws.root.join(".corral/events.jsonl"))
        .expect("events");
    writeln!(events, "{approved}").expect("append");
    ok(&ws.root, &["move", "T-002", "done", "--force"]);
    ok(&ws.root, &["move", "T-004", "dropped", "--owner-ok"]);
    let ids = |v: &serde_json::Value| -> Vec<String> {
        v.as_array()
            .expect("array")
            .iter()
            .map(|t| t["id"].as_str().expect("id").to_string())
            .collect()
    };

    // When listing done tasks of the last day, and all done tasks.
    let recent: serde_json::Value = serde_json::from_str(&ok(
        &ws.root,
        &["list", "--done", "--since", "1d", "--json"],
    ))
    .expect("json");
    let all: serde_json::Value =
        serde_json::from_str(&ok(&ws.root, &["list", "--done", "--json"])).expect("json");
    let text = ok(&ws.root, &["list", "--done"]);

    // Then the window drops the older landing, the order is newest first, and the landing is reported.
    assert_eq!(ids(&recent), ["T-002"]);
    assert_eq!(
        ids(&all),
        ["T-002", "T-001"],
        "dropped and open tasks excluded"
    );
    assert_eq!(all[1]["landed"]["sha"], "1a2b3c4d5e6f7a8b");
    assert_eq!(all[1]["landed"]["strategy"], "rebase");
    assert_eq!(all[1]["finished"], three_days_ago.to_string());
    assert!(all[0]["landed"].is_null(), "a forced move landed nothing");
    let old = text
        .lines()
        .find(|l| l.starts_with("T-001"))
        .unwrap_or_else(|| panic!("{text}"));
    assert!(
        old.contains("1a2b3c4 rebase") && old.contains("Old landing"),
        "{old}"
    );
    assert!(
        !corral(&ws.root, &["list", "--since", "1d"])
            .status
            .success()
    );
    assert!(
        !corral(&ws.root, &["list", "--done", "--since", "1x"])
            .status
            .success()
    );
}
