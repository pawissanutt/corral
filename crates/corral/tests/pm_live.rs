//! Real CLI acceptance check, confined to the isolated corral-test herdr server.
#![cfg(unix)] // The agent fixture uses Unix datagrams.

use std::io::Write;
use std::os::unix::net::UnixDatagram;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use corral_core::{AgentCmd, Store};
use corral_herdr::{AgentStatus, ENTER_ATTEMPTS, Herdr};
use serde_json::Value;

#[path = "resource_support/mod.rs"]
mod resource_support;

const SESSION: &str = "corral-test";
const FAKE_PM: &str = r#"import socket, sys
s = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
s.connect(sys.argv[1])
print("> ", end="", flush=True)
s.send(b"READY")
for line in sys.stdin:
    print("> ", end="", flush=True)
    if line.startswith("[corral]"):
        s.send(line.strip().encode())
"#;

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn cli(root: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_corral"));
    cmd.env("CORRAL_ROOT", root)
        .env_remove("CORRAL_GATE_ATTEMPT")
        .env_remove("CORRAL_CANDIDATE_HEAD")
        .env_remove("CORRAL_INTEGRATION_BASE")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_WORKSPACE_ID")
        .env_remove("HERDR_TAB_ID")
        .env_remove("HERDR_BIN_PATH")
        .env("CORRAL_HERDR_SESSION", SESSION)
        .arg("--root")
        .arg(root)
        .args(args)
        .stdin(Stdio::null());
    cmd
}

fn ok(root: &Path, args: &[&str]) -> Output {
    let out = cli(root, args).output().unwrap();
    assert!(
        out.status.success(),
        "corral {args:?}: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .env("CORRAL_ROOT", root)
        .env_remove("CORRAL_GATE_ATTEMPT")
        .env_remove("CORRAL_CANDIDATE_HEAD")
        .env_remove("CORRAL_INTEGRATION_BASE")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_WORKSPACE_ID")
        .env_remove("HERDR_TAB_ID")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

struct Panes {
    herdr: Herdr,
    ids: Vec<String>,
}

impl Drop for Panes {
    fn drop(&mut self) {
        for id in &self.ids {
            if let Err(err) = self.herdr.pane_close(id) {
                eprintln!("closing isolated pane {id}: {err:#}");
            }
        }
    }
}

#[test]
fn done_wakes_background_wait_and_pushes_to_live_pm() {
    // Given a real isolated server, a fake PM that acknowledges terminal input, and a run.
    let herdr = Herdr::with_bin("herdr".into(), Some(SESSION.into()));
    if !herdr.available() {
        println!("SKIP: herdr session {SESSION} unavailable");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let app = root.join("app");
    std::fs::create_dir(&app).unwrap();
    git(&app, &["init", "-q", "-b", "main"]);
    git(&app, &["config", "user.name", "corral live test"]);
    git(&app, &["config", "user.email", "corral@test.invalid"]);
    git(&app, &["config", "commit.gpgsign", "false"]);
    std::fs::write(app.join("README.md"), "initial\n").unwrap();
    git(&app, &["add", "."]);
    git(&app, &["commit", "-q", "-m", "init"]);
    let socket_path = root.join("pm.sock");
    let socket = UnixDatagram::bind(&socket_path).unwrap();
    // READY and the PM's acknowledgement are the signals; time only bounds a hang.
    socket
        .set_read_timeout(Some(Duration::from_secs(120)))
        .unwrap();
    let mut store = Store::init(root).unwrap();
    resource_support::configure(&mut store.config);
    store.config.agents.insert(
        "fake-pm".into(),
        AgentCmd {
            command: format!(
                "python3 -u -c {} {}",
                quote(FAKE_PM),
                quote(&socket_path.display().to_string())
            ),
        },
    );
    store.config.agents.insert(
        "fake-worker".into(),
        AgentCmd {
            command: "true".into(),
        },
    );
    store.config.pm.agent = "fake-pm".into();
    store.config.pm.push_events = true;
    store.config.pm.push_kinds = vec!["done".into()];
    store.config.herdr.session = Some(SESSION.into());
    store.save_config().unwrap();
    let pm_out = ok(root, &["pm"]);
    let pm = String::from_utf8(pm_out.stdout)
        .unwrap()
        .trim()
        .strip_prefix("PM pane ")
        .unwrap()
        .to_string();
    let mut panes = Panes {
        herdr,
        ids: vec![pm.clone()],
    };
    let mut received = [0; 4096];
    let n = socket.recv(&mut received).unwrap();
    assert_eq!(&received[..n], b"READY");
    // Label it like an OmO pane, exercising agent_prompt's not-ready terminal fallback.
    panes
        .herdr
        .pane_report_agent(&pm, "omo", AgentStatus::Idle, None)
        .unwrap();
    let out = ok(root, &["new", "Live wake", "-p", "app", "--ready"]);
    let id = String::from_utf8(out.stdout).unwrap().trim().to_string();
    ok(root, &["dispatch", &id, "--profile", "fake-worker"]);
    let task = store.get(&id).unwrap();
    let run = task.current_run().unwrap();
    panes.ids.push(run.herdr.as_ref().unwrap().pane_id.clone());
    std::fs::write(run.worktree.join("wake.txt"), "verified\n").unwrap();
    git(&run.worktree, &["add", "."]);
    git(&run.worktree, &["commit", "-qm", "Record wake fixture"]);
    let identity: Value =
        serde_json::from_slice(&ok(root, &["run", "checkpoint", &id, "--json"]).stdout).unwrap();
    let checkpoint = serde_json::json!({
        "task": id, "run": run.id, "spec_revision": identity["spec_revision"],
        "head": identity["head"], "base": identity["base"], "tree": identity["tree"],
        "expected_revision": 0, "progress_revision": 0,
        "criterion_ids": [], "evidence_refs": [],
        "next_action": "Submit the verified wake event", "decisions": [], "waits": [], "resources": []
    });
    let checkpoint_file = root.join("checkpoint.json");
    std::fs::write(&checkpoint_file, serde_json::to_vec(&checkpoint).unwrap()).unwrap();
    ok(
        root,
        &[
            "run",
            "checkpoint",
            &id,
            "--file",
            checkpoint_file.to_str().unwrap(),
        ],
    );
    // Subscribe before triggering done; an explicit cursor makes startup scheduling irrelevant.
    let offset = std::fs::metadata(store.dir().join("events.jsonl"))
        .unwrap()
        .len();
    let waiter = cli(
        root,
        &[
            "wait",
            "--after-offset",
            &offset.to_string(),
            "--task",
            &id,
            "--for",
            "done",
            "--timeout",
            "120s",
            "--json",
            "--print-cursor",
        ],
    )
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
    // When the real done command appends its event and sync delivers it.
    ok(root, &["done", &id, "--summary", "Live wake acceptance"]);
    ok(root, &["sync"]);
    // Then both the background waiter and the actual PM process observe the same event.
    let out = waiter.wait_with_output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let event: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(event["task"], id);
    assert_eq!(event["kind"], "done");
    let n = socket.recv(&mut received).unwrap();
    let pushed = std::str::from_utf8(&received[..n]).unwrap();
    assert!(
        pushed.starts_with(&format!("[corral] {id} done R1 ")),
        "{pushed}"
    );
    let snapshot = panes.herdr.pane_read(&pm, 25).unwrap();
    assert!(
        snapshot.contains(&format!("[corral] {id} done")),
        "{snapshot}"
    );
    let events = store.events().unwrap();
    let delivery = events
        .iter()
        .find(|e| e.kind == "pm_event_delivery" && e.task.as_deref() == Some(&id))
        .unwrap();
    assert_eq!(delivery.detail["via"], "send_text");
    assert_eq!(delivery.detail["submitted"], true);
    assert!(
        (1..=u64::from(ENTER_ATTEMPTS)).contains(&delivery.detail["attempts"].as_u64().unwrap()),
        "{delivery:?}"
    );
    println!("LIVE session={SESSION} PM={pm}; corral done + sync: exit 0");
    println!(
        "LIVE background corral wait: {}",
        String::from_utf8_lossy(&out.stdout).trim()
    );
    println!("LIVE PM process acknowledged: {pushed}");
    println!("LIVE PM terminal snapshot:\n{snapshot}");
    std::io::stdout().flush().unwrap();
}
