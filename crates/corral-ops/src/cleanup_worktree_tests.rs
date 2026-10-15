use std::io::{BufRead, BufReader};

use super::*;

#[test]
fn current_process_and_parent_are_protected() {
    let pids = protected_pids().unwrap();
    let own = std::process::id();
    let parent = SystemProcesses.inspect(own).unwrap().unwrap().parent_pid;
    assert!(pids.contains(&own));
    assert!(pids.contains(&parent));
    assert!(pids.contains(&1));
}

#[test]
fn pidfd_termination_refuses_changed_identity_without_signalling() {
    let mut child = Command::new("python3")
        .args([
            "-c",
            "import signal; print('ready', flush=True); signal.pause()",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(line.trim(), "ready");
    let snapshot = SystemProcesses.inspect(child.id()).unwrap().unwrap();
    let mut identity = StuckProcess {
        pid: child.id(),
        started: format!("{}-stale", snapshot.started),
        argv: snapshot.argv,
    };
    assert!(SystemProcesses.terminate(&identity).is_err());
    assert!(child.try_wait().unwrap().is_none());
    identity.started = snapshot.started;
    identity.argv.push("changed-command".into());
    assert!(SystemProcesses.terminate(&identity).is_err());
    assert!(child.try_wait().unwrap().is_none());
    child.kill().unwrap();
    child.wait().unwrap();
}
