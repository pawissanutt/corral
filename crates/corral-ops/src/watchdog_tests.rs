use std::io::{BufRead, BufReader};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Stdio};

use crate::test_support as support;

use super::*;

struct ChildGuard {
    process: Child,
    _root: tempfile::TempDir,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Err(err) = self.process.kill() {
            eprintln!("test child cleanup: {err}");
        }
        if let Err(err) = self.process.wait() {
            eprintln!("test child wait: {err}");
        }
    }
}

fn child(ignore_term: bool) -> (ChildGuard, StuckProcess) {
    let script = format!(
        "import signal,sys\n{}\nprint('READY',flush=True)\nsys.stdin.read()",
        if ignore_term {
            "signal.signal(signal.SIGTERM, signal.SIG_IGN)"
        } else {
            ""
        }
    );
    let root = tempfile::tempdir().unwrap();
    let mut child = ChildGuard {
        process: support::command("python3", root.path())
            .args(["-c", &script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap(),
        _root: root,
    };
    let stdout = child.process.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut ready = String::new();
        let result = BufReader::new(stdout).read_line(&mut ready);
        tx.send(result.map(|_| ready)).unwrap();
    });
    let ready = rx
        // The line confirms signal-handler installation; time only guards a hang.
        .recv_timeout(std::time::Duration::from_secs(120))
        .unwrap()
        .unwrap();
    assert_eq!(ready.trim(), "READY");
    let pid = child.process.id();
    let snapshot = SystemProcesses.inspect(pid).unwrap().unwrap();
    (
        child,
        StuckProcess {
            pid,
            argv: snapshot.argv,
            started: snapshot.started,
        },
    )
}

#[test]
fn system_killer_terminates_only_the_selected_identity() {
    // Given two real, ready children.
    let (mut target, process) = child(false);
    let (mut other, _) = child(false);
    // When terminating the selected pidfd.
    SystemProcesses.terminate(&process).unwrap();
    // Then SIGTERM ended that child while its peer remains alive.
    assert_eq!(target.process.wait().unwrap().signal(), Some(15));
    assert!(other.process.try_wait().unwrap().is_none());
}

#[test]
fn system_killer_escalates_when_sigterm_is_ignored() {
    // Given readiness emitted only after SIGTERM is ignored.
    let (mut target, process) = child(true);
    // When the production three-second termination deadline elapses.
    SystemProcesses.terminate(&process).unwrap();
    // Then escalation ended the child with SIGKILL.
    assert_eq!(target.process.wait().unwrap().signal(), Some(9));
}

#[test]
fn system_killer_rejects_a_changed_identity_without_signalling() {
    let (mut target, mut process) = child(false);
    process.started.push('0');
    assert!(SystemProcesses.terminate(&process).is_err());
    assert!(target.process.try_wait().unwrap().is_none());
}
