#![cfg(unix)] // This fixture asserts POSIX signal status and uses a shebang engine.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, Stdio};

use corral_core::{HerdrRef, Run, Status, Store, StuckProcess};
use corral_ops::{ProcessControl, SystemProcesses};
use serde_json::json;

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Err(err) = self.0.kill() {
            eprintln!("test cleanup: {err}");
        }
        if let Err(err) = self.0.wait() {
            eprintln!("test wait: {err}");
        }
    }
}

#[test]
fn cli_confirmation_controls_termination_and_reply_recovery() {
    // Given a real child in a fake herdr foreground group, with a pending reply.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut child = ChildGuard(
        Command::new("python3")
            .env("CORRAL_ROOT", tmp.path())
            .env_remove("CORRAL_GATE_ATTEMPT")
            .env_remove("CORRAL_CANDIDATE_HEAD")
            .env_remove("CORRAL_INTEGRATION_BASE")
            .env_remove("HERDR_PANE_ID")
            .env_remove("HERDR_WORKSPACE_ID")
            .env_remove("HERDR_TAB_ID")
            .args(["-c", "import sys; print(123,flush=True); sys.stdin.read()"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    // Do not sample /proc during exec: some kernels briefly expose an empty cmdline.
    let stdout = child.0.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut ready = String::new();
        let result = BufReader::new(stdout).read_line(&mut ready);
        if let Err(err) = tx.send(result.map(|_| ready)) {
            eprintln!("test readiness receiver closed: {err}");
        }
    });
    // Readiness is event-driven; this generous timeout only guards against a hung child.
    assert_eq!(
        rx.recv_timeout(std::time::Duration::from_secs(120))
            .expect("child readiness hang guard expired or reader disconnected")
            .unwrap()
            .trim(),
        "123"
    );
    let pid = child.0.id();
    let local = SystemProcesses.inspect(pid).unwrap().unwrap();
    let stuck = StuckProcess {
        pid,
        argv: local.argv.clone(),
        started: local.started,
    };
    let run: Run = serde_json::from_value(json!({
        "id": "R1", "agent": "omo", "harness": "omo", "worktree": tmp.path(),
        "branch": "corral/T-001", "base": "main", "base_sha": "abc",
        "started": "2026-10-15T09:00:00Z"
    }))
    .unwrap();
    let task = store
        .create("Stuck CLI", None, Status::Running, "")
        .unwrap();
    store
        .update(&task.id, |t| {
            let mut r = run.clone();
            r.herdr = Some(HerdrRef {
                workspace_id: "w1".into(),
                pane_id: "w1:p1".into(),
                socket: None,
            });
            r.stuck = Some(stuck.clone());
            r.pending_reply = Some("continue".into());
            t.runs.push(r);
            Ok(())
        })
        .unwrap();
    let herdr = tmp.path().join("herdr");
    let info = json!({"result": {"process_info": {
        "shell_pid": 1, "foreground_processes": [{"pid": pid, "argv": local.argv}]
    }}});
    std::fs::write(
        &herdr,
        format!(
            "#!/bin/sh\nif [ \"$1\" = --session ]; then shift 2; fi\ncase \"$1 $2\" in\n\
         'pane process-info') printf '%s\\n' '{info}' ;;\n\
         'agent prompt') printf '%s\\n' \"$4\" >> '{}'; printf '%s\\n' '{{\"result\":{{}}}}' ;;\n\
         *) exit 1 ;;\nesac\n",
            tmp.path().join("replies").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&herdr, std::fs::Permissions::from_mode(0o755)).unwrap();
    let command = || {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_corral"));
        for (name, _) in std::env::vars_os() {
            let text = name.to_string_lossy();
            if text.starts_with("CORRAL_") || text.starts_with("HERDR_") {
                cmd.env_remove(name);
            }
        }
        cmd.args(["--root", tmp.path().to_str().unwrap(), "unstick", "T-001"])
            .env("CORRAL_ROOT", tmp.path())
            .env("HERDR_BIN_PATH", &herdr)
            .env("CORRAL_HERDR_SESSION", "fixture")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    };
    // When confirmation is declined, then no process or reply is touched.
    let mut declined = command().spawn().unwrap();
    declined.stdin.take().unwrap().write_all(b"n\n").unwrap();
    assert!(declined.wait_with_output().unwrap().status.success());
    assert!(child.0.try_wait().unwrap().is_none());
    let declined_task = store.get("T-001").unwrap();
    assert!(declined_task.current_run().unwrap().stuck.is_some());
    assert!(!tmp.path().join("replies").exists());
    // When the same process is confirmed, then SIGTERM ends it and the reply reaches the fake harness.
    let mut confirmed = command().spawn().unwrap();
    confirmed.stdin.take().unwrap().write_all(b"y\n").unwrap();
    let out = confirmed.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(child.0.wait().unwrap().signal(), Some(15));
    let confirmed_task = store.get("T-001").unwrap();
    assert!(confirmed_task.current_run().unwrap().stuck.is_none());
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("replies")).unwrap(),
        "continue\n"
    );
}
