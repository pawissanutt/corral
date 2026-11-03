#![cfg(target_os = "linux")]
use super::*;
use std::process::{Child, Command, Stdio};

struct Sleeper(Child);
impl Drop for Sleeper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn cancellation_stops_only_the_gate_attempts_bound_processes() {
    let dir = tempfile::tempdir().unwrap();
    let token = dir.path().to_str().unwrap();
    let spawn = |attempt: &str| {
        Sleeper(
            Command::new("sleep")
                .arg("60")
                .env("CORRAL_GATE_ATTEMPT", "A1")
                .env("CORRAL_GATE_PROCESS_OWNER", attempt)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    };
    let mut gate = spawn(token);
    let mut worker = spawn("not-this-gate");
    stop_descendants(std::path::Path::new(token)).unwrap();
    assert!(gate.0.wait().unwrap().code().is_none());
    assert!(
        worker.0.try_wait().unwrap().is_none(),
        "the worker is not a gate descendant"
    );
}
