//! Real CLI round trips with a generic fixture worker and an isolated fake herdr.
#![cfg(unix)] // Fixtures execute a POSIX-shebang engine.
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;

use corral_core::{Run, Status, Store};
use serde_json::{Value, json};

#[path = "resource_support/mod.rs"]
mod resource_support;

static EXECUTABLE_FIXTURE: Mutex<()> = Mutex::new(());

struct Fixture {
    _root: tempfile::TempDir,
    store: Store,
    herdr: PathBuf,
    log: std::cell::RefCell<String>,
}

fn command(program: &str, dir: &Path, args: &[&str]) {
    let output = Command::new(program)
        .env("CORRAL_ROOT", dir)
        .env_remove("CORRAL_GATE_ATTEMPT")
        .env_remove("CORRAL_CANDIDATE_HEAD")
        .env_remove("CORRAL_INTEGRATION_BASE")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_WORKSPACE_ID")
        .env_remove("HERDR_TAB_ID")
        .current_dir(dir)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let worktree = root.path().join("app");
        std::fs::create_dir(&worktree).unwrap();
        command("git", &worktree, &["init", "-q", "-b", "main"]);
        command(
            "git",
            &worktree,
            &[
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "fixture",
            ],
        );
        let mut store = Store::init(root.path()).unwrap();
        resource_support::configure(&mut store.config);
        store.config.herdr.session = Some("corral-test".into());
        store.config.review.gate = Some(vec!["printf 'gate-ready\\n'".into()]);
        store.config.review.autopilot = Some(false);
        store.save_config().unwrap();
        let task = store
            .create("Portable obligations", Some("app"), Status::Running, "")
            .unwrap();
        let run: Run = serde_json::from_value(json!({
            "id":"R1","agent":"generic","harness":"generic","worktree":worktree,
            "branch":"main","base":"main","base_sha":"fixture","started":"2026-10-15T09:00:00Z",
            "herdr":{"workspace_id":"w1","pane_id":"w1:p1",
                "socket":corral_herdr::Herdr::new(Some("corral-test".into())).socket_path().unwrap()},
            "seen_working":true
        }))
        .unwrap();
        store
            .update(&task.id, |t| {
                t.runs.push(run.clone());
                Ok(())
            })
            .unwrap();
        let herdr = root.path().join("herdr");
        std::fs::write(&herdr, r#"#!/bin/sh
if [ "$1" = --session ]; then
  [ "$2" = corral-test ] || exit 91
  shift 2
fi
case "$1 $2" in
  "workspace list") printf '%s\n' '{"result":{"workspaces":[{"workspace_id":"w1","label":"fixture"}]}}' ;;
  "pane list") printf '%s\n' '{"result":{"panes":[{"pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1","agent":"generic","agent_status":"idle"},{"pane_id":"w1:p2","workspace_id":"w1","tab_id":"w1:t2","agent_status":"working"}]}}' ;;
  "tab create") printf '%s\n' '{"result":{"root_pane":{"workspace_id":"w1","tab_id":"w1:t2","pane_id":"w1:p2"}}}' ;;
  "pane get") printf '%s\n' '{"result":{"pane":{"pane_id":"w1:p2","workspace_id":"w1","tab_id":"w1:t2"}}}' ;;
  "pane run"|"pane report-metadata"|"notification show") printf '%s\n' '{"result":{}}' ;;
  "pane process-info") printf '%s\n' '{"result":{"process_info":{"shell_pid":10,"foreground_processes":[]}}}' ;;
  "agent prompt") printf '%s\n' "$4" >> "$(dirname "$0")/replies"; printf '%s\n' '{"result":{}}' ;;
  *) printf '{"error":{"code":"unexpected","message":"%s"}}\n' "$*"; exit 1 ;;
esac
"#).unwrap();
        std::fs::set_permissions(&herdr, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            _root: root,
            store,
            herdr,
            log: Default::default(),
        }
    }

    fn cli(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
        for (name, _) in std::env::vars_os() {
            let text = name.to_string_lossy();
            if text.starts_with("CORRAL_") || text.starts_with("HERDR_") {
                command.env_remove(name);
            }
        }
        let out = command
            .env("CORRAL_ROOT", self.store.root())
            .args(["--root", self.store.root().to_str().unwrap()])
            .args(args)
            .env("HERDR_BIN_PATH", &self.herdr)
            .env("CORRAL_HERDR_SESSION", "corral-test")
            .stdin(Stdio::null())
            .output()
            .unwrap();
        self.log.borrow_mut().push_str(&format!(
            "$ corral {}\nexit={}\n{}{}\n",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ));
        out
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.cli(args);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn submit(&self, verb: &str, name: &str, value: Value) -> Value {
        let file = self.store.root().join(name);
        std::fs::write(&file, value.to_string()).unwrap();
        serde_json::from_str(&self.ok(&["run", verb, "T-001", "--file", file.to_str().unwrap()]))
            .unwrap()
    }

    fn show(&self) -> Value {
        serde_json::from_str(&self.ok(&["show", "T-001", "--json"])).unwrap()
    }

    fn evidence(&self, name: &str) {
        if let Ok(dir) = std::env::var("CORRAL_T057_EVIDENCE") {
            let dir = PathBuf::from(dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join(format!("{name}.log")),
                self.log.borrow().as_bytes(),
            )
            .unwrap();
            std::fs::write(
                dir.join(format!("{name}-events.json")),
                serde_json::to_vec_pretty(&self.store.events().unwrap()).unwrap(),
            )
            .unwrap();
        }
    }
}

fn channel(id: &str) -> Value {
    json!({"version":1,"id":id,"run":"R1","condition":"External CI completed","owner":"fixture","source":"worker",
        "deadline":"2099-01-01T00:00:00Z","handle":{"kind":"channel","channel_id":"fixture:ci","capability":"wait_v1"}})
}

#[test]
fn generic_worker_gate_wait_collects_once_across_cli_process_restarts() {
    // Given the real gate CLI and a generic worker declaring its actual invocation.
    let _lock = EXECUTABLE_FIXTURE.lock().unwrap();
    let fx = Fixture::new();
    fx.ok(&["gate", "T-001"]);
    let task = fx.show();
    let gate = &task["runs"][0]["gate"];
    fx.submit("wait", "wait.json", json!({
        "version":1,"id":"gate","run":"R1","condition":"Review gate result available","owner":"fixture","source":"worker",
        "deadline":"2099-01-01T00:00:00Z","handle":{"kind":"gate","sha":gate["sha"],"pane":gate["pane"]}
    }));
    fx.ok(&["sync"]);
    let pending = fx.show();
    assert_eq!(pending["status"], "running");
    assert_eq!(
        pending["runs"][0]["obligations"]["waits"][0]["observation"]["state"],
        "pending"
    );
    assert_eq!(
        pending["runs"][0]["obligations"]["waits"][0]["request"]["condition"],
        "Review gate result available"
    );
    // When readiness is injected by executing the real script, and two fresh sync processes collect.
    let ctx = corral_ops::resources::FixedResourceProbe::ctx(fx.store.clone());
    let script = corral_ops::gate_paths(&ctx, "T-001", "R1").script;
    command("bash", fx.store.root(), &[script.to_str().unwrap()]);
    fx.ok(&["sync"]);
    let first = fx.show();
    fx.ok(&["sync"]);
    let second = fx.show();
    // Then one real gate result and one immutable wait receipt survive restarts.
    assert_eq!(
        first["runs"][0]["obligations"],
        second["runs"][0]["obligations"]
    );
    assert_eq!(
        second["runs"][0]["obligations"]["waits"][0]["receipt"]["outcome"],
        "ready"
    );
    assert_eq!(
        second["runs"][0]["obligations"]["waits"][0]["receipt"]["delivery"],
        "unknown"
    );
    assert_eq!(
        fx.store
            .events()
            .unwrap()
            .iter()
            .filter(|e| e.kind == "gate")
            .count(),
        1
    );
    assert!(!fx.store.root().join("replies").exists());
    fx.evidence("cli-gate");
}

#[test]
fn idle_owner_question_answer_and_expired_wait_are_visible_via_real_cli() {
    // Given a generic worker registering an owner question while fake herdr reports idle.
    let _lock = EXECUTABLE_FIXTURE.lock().unwrap();
    let fx = Fixture::new();
    fx.submit(
        "decision",
        "decision.json",
        json!({
            "version":1,"id":"ship","run":"R1","question":"Which migration should ship?",
            "requested_action":"Choose migration A or B","owner":"owner","source":"worker"
        }),
    );
    fx.ok(&["sync"]);
    let question = fx.show();
    assert_eq!(question["status"], "needs_me");
    assert_eq!(
        question["runs"][0]["obligations"]["decisions"][0]["request"]["question"],
        "Which migration should ship?"
    );
    // This round trip tests delivery, not wall-clock rollback. Pin registration
    // to the fixture's fixed run start so a host clock adjustment cannot invert it.
    fx.store
        .update("T-001", |task| {
            let run = task.current_run_mut().unwrap();
            run.obligations.decisions[0].registered_at = run.started;
            Ok(())
        })
        .unwrap();
    // When the normal verified reply answers it, then an already-expired external wait is registered.
    fx.ok(&["reply", "T-001", "Migration B"]);
    let mut expired = channel("expired");
    expired["deadline"] = json!("1970-01-01T00:00:00Z");
    fx.submit("wait", "expired.json", expired);
    fx.ok(&["sync"]);
    let task = fx.show();
    // Then the exact answer is retained and expiration is not an unanswered question.
    assert_eq!(task["status"], "running");
    assert_eq!(
        task["runs"][0]["obligations"]["decisions"][0]["receipt"]["answer"],
        "Migration B"
    );
    assert_eq!(
        task["runs"][0]["obligations"]["waits"][0]["observation"]["state"],
        "expired"
    );
    assert_eq!(
        std::fs::read_to_string(fx.store.root().join("replies")).unwrap(),
        "Migration B\n"
    );
    fx.evidence("cli-decision-expired");
}

#[test]
fn unknown_delivery_duplicate_and_malformed_receipts_have_explicit_cli_results() {
    // Given a worker's pending channel with an unknown-delivery completion.
    let _lock = EXECUTABLE_FIXTURE.lock().unwrap();
    let fx = Fixture::new();
    let task = fx.submit("wait", "channel.json", channel("ci"));
    let registered = task["runs"][0]["obligations"]["waits"][0]["registered_at"].clone();
    let file = fx.store.root().join("receipt.json");
    std::fs::write(
        &file,
        json!({"version":1,"id":"ci-ready","run":"R1","outcome":"ready",
        "delivery":"unknown","observed_at":registered,"data":{"exit_code":0}})
        .to_string(),
    )
    .unwrap();
    // When the same receipt is repeated across fresh CLI processes, followed by malformed input.
    for _ in 0..2 {
        fx.ok(&[
            "run",
            "resume",
            "T-001",
            "--wait",
            "ci",
            "--file",
            file.to_str().unwrap(),
        ]);
    }
    fx.ok(&["sync"]);
    let before = fx.show();
    std::fs::write(&file, r#"{"version":99}"#).unwrap();
    let rejected = fx.cli(&[
        "run",
        "resume",
        "T-001",
        "--wait",
        "ci",
        "--file",
        file.to_str().unwrap(),
    ]);
    // Then uncertainty persists, malformed input is rejected and no work is resent.
    assert!(!rejected.status.success());
    assert_eq!(
        before["runs"][0]["obligations"],
        fx.show()["runs"][0]["obligations"]
    );
    assert_eq!(
        before["runs"][0]["obligations"]["waits"][0]["observation"]["state"],
        "delivery_unknown"
    );
    assert_eq!(
        fx.store
            .events()
            .unwrap()
            .iter()
            .filter(|e| e.kind == "run_resumed")
            .count(),
        1
    );
    assert!(!fx.store.root().join("replies").exists());
    fx.evidence("cli-receipts");
}
