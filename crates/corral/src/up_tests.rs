use super::wait_for_server;
use std::process::{Child, Command, Stdio};

fn exited_engine(code: i32) -> Child {
    let mut child = Command::new("sh")
        .args(["-c", &format!("exit {code}")])
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    // Wait for the exact process-exit event, not a scheduler-dependent delay.
    assert_eq!(child.wait().unwrap().code(), Some(code));
    child
}

#[test]
fn server_ready_between_probe_and_child_exit_is_accepted() {
    // Given a server that became ready and exited after a stale negative probe.
    let child = exited_engine(0);
    let mut probes = [false, true].into_iter();
    // When startup observes the stale probe followed by the child's exit.
    let result = wait_for_server(child, || probes.next().unwrap());
    // Then it refreshes readiness rather than reporting a successful exit as death.
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(probes.next(), None);
}

#[test]
fn exited_engine_without_readiness_still_fails() {
    // Given an engine that exited without ever serving requests.
    for code in [0, 1] {
        let child = exited_engine(code);
        // When startup checks readiness.
        let error = wait_for_server(child, || false).unwrap_err();
        // Then the exit remains an actionable failure, even for exit status zero.
        assert!(
            error.to_string().contains("terminal engine exited"),
            "{error}"
        );
    }
}
