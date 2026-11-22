use std::{
    collections::BTreeMap,
    ffi::CString,
    fs::OpenOptions,
    io::{self, Read},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::Path,
    time::{Duration, Instant},
};

use serde_json::{Map, Value};

use super::{CommandContext, run};

fn large_request() -> Map<String, Value> {
    Map::from_iter([("data".into(), Value::String("x".repeat(512 * 1024)))])
}

fn context<'a>(cwd: &'a Path, env: &'a BTreeMap<String, String>) -> CommandContext<'a> {
    CommandContext {
        root: cwd,
        cwd,
        project: None,
        extension: "fixture",
        env,
    }
}

#[test]
fn deadline_kills_sleeping_descendant_after_shell_exit() {
    let cwd = tempfile::tempdir().unwrap();
    let fifo = cwd.path().join("observer");
    let fifo_c = CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: fifo_c is a valid path in the test's private temporary directory.
    assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
    let mut observer = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(fifo)
        .unwrap();
    // The shell exits but sleep holds stdout, stderr, and a copy of stdin. A
    // large request also checks the deadline while stdin cannot make progress.
    let started = Instant::now();
    let error = run(
        "exec 4<&0; (exec 3>observer; printf ready >&3; exec sleep 4) &",
        large_request(),
        &context(cwd.path(), &BTreeMap::new()),
        Duration::from_millis(300),
        1024,
    )
    .unwrap_err();
    assert!(error.to_string().contains("timed out"), "{error:#}");
    assert!(started.elapsed() < Duration::from_secs(2));

    let mut ready = [0; 5];
    observer.read_exact(&mut ready).unwrap();
    assert_eq!(&ready, b"ready", "descendant started before the deadline");
    // The writer's FIFO fd is released only when sleep dies. Its PID might
    // remain a zombie, and kill(0) would then be an unreliable assertion.
    let mut byte = [0];
    match observer.read(&mut byte) {
        Ok(0) => {}
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
            let mut fd = libc::pollfd {
                fd: observer.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: fd is a live pollfd borrowing the open observer.
            assert!(unsafe { libc::poll(&mut fd, 1, 1000) } > 0);
            assert_eq!(observer.read(&mut byte).unwrap(), 0);
        }
        result => panic!("unexpected observation: {result:?}"),
    }
}

#[test]
fn deadline_still_applies_when_shell_closed_all_pipes() {
    let cwd = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let error = run(
        "cat >/dev/null; exec >/dev/null 2>&1; exec sleep 4",
        Map::new(),
        &context(cwd.path(), &BTreeMap::new()),
        Duration::from_millis(100),
        1024,
    )
    .unwrap_err();
    assert!(error.to_string().contains("timed out"), "{error:#}");
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn closed_stdin_does_not_hide_success_or_change_stdout() {
    let cwd = tempfile::tempdir().unwrap();
    let output = run(
        "exec </dev/null; printf '\\000\\377'",
        large_request(),
        &context(cwd.path(), &BTreeMap::new()),
        Duration::from_secs(2),
        2,
    )
    .unwrap();
    assert_eq!(output, [0, 255]);
}

#[test]
fn stdout_over_ceiling_fails() {
    let cwd = tempfile::tempdir().unwrap();
    let error = run(
        "cat >/dev/null; printf 0123456789",
        Map::new(),
        &context(cwd.path(), &BTreeMap::new()),
        Duration::from_secs(2),
        9,
    )
    .unwrap_err();
    assert!(error.to_string().contains("stdout exceeded"), "{error:#}");
}

#[test]
fn nonzero_exit_reports_only_last_four_kib_of_stderr() {
    let cwd = tempfile::tempdir().unwrap();
    let error = run(
        "exec </dev/null; printf 'discard-me%04096d' 0 >&2; printf 'stdout-must-not-leak'; exit 7",
        Map::new(),
        &context(cwd.path(), &BTreeMap::new()),
        Duration::from_secs(2),
        1024,
    )
    .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("status 7"));
    assert!(!message.contains("discard-me"));
    assert!(!message.contains("stdout-must-not-leak"));
    assert_eq!(message.lines().last().unwrap(), "0".repeat(4096));
}
