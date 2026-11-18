use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use super::{FakeRunner, LaunchSpec, Origin, ProcessRunner, Runner, SessionRef, SessionStatus};
use crate::error::Refusal;
#[cfg(unix)]
use crate::{Next, Op, as_refusal};

pub(super) fn spec(root: &Path, command: &str) -> LaunchSpec {
    LaunchSpec {
        label: "T-042 runner test".to_owned(),
        cwd: root.to_owned(),
        command: command.to_owned(),
        env: Vec::new(),
        dir: root.join("T-042").join("run with ' quotes"),
        tokens: vec![
            ("task".to_owned(), "T-042".to_owned()),
            ("run".to_owned(), "R7".to_owned()),
        ],
    }
}

#[test]
fn session_contract_round_trips_with_snake_case_variants() {
    let sessions = [
        SessionRef::Herdr {
            workspace: "workspace".to_owned(),
            pane: "pane".to_owned(),
        },
        SessionRef::Process {
            pid: 42,
            dir: "/tmp/run".into(),
        },
    ];
    for session in sessions {
        let value = serde_json::to_value(&session).unwrap();
        assert!(value.get("herdr").is_some() || value.get("process").is_some());
        assert_eq!(
            serde_json::from_value::<SessionRef>(value).unwrap(),
            session
        );
    }
    for status in [
        SessionStatus::Working,
        SessionStatus::Blocked {
            excerpt: "question".to_owned(),
        },
        SessionStatus::Idle,
        SessionStatus::Gone,
    ] {
        let encoded = serde_json::to_string(&status).unwrap();
        assert_eq!(
            serde_json::from_str::<SessionStatus>(&encoded).unwrap(),
            status
        );
    }
    assert_eq!(
        serde_json::to_string(&SessionStatus::Working).unwrap(),
        "\"working\""
    );
    let launch = spec(Path::new("/tmp"), "true");
    let encoded = serde_json::to_vec(&launch).unwrap();
    assert_eq!(
        serde_json::from_slice::<LaunchSpec>(&encoded).unwrap(),
        launch
    );
}

#[test]
fn fake_shares_history_scripts_status_and_runs_reentrant_agent_hook() {
    let temp = crate::tests::tempdir();
    let runner = FakeRunner::new();
    let shared = runner.clone();
    let hook_runner = runner.clone();
    runner.set_start_hook(move |launch| {
        assert_eq!(hook_runner.starts().len(), 1);
        fs::write(launch.cwd.join("agent-done"), b"done")?;
        Ok(())
    });
    let launch = spec(temp.path(), "agent");
    let session = runner.start(&launch).unwrap();
    runner.clear_start_hook();
    assert!(temp.path().join("agent-done").exists());
    assert_eq!(shared.starts(), vec![launch]);
    shared.set_default_status(SessionStatus::Idle);
    shared.queue_status(&session, SessionStatus::Working);
    shared.queue_status(
        &session,
        SessionStatus::Blocked {
            excerpt: "help".to_owned(),
        },
    );
    assert_eq!(runner.status(&session).unwrap(), SessionStatus::Working);
    assert_eq!(
        runner.status(&session).unwrap(),
        SessionStatus::Blocked {
            excerpt: "help".to_owned()
        }
    );
    assert_eq!(runner.status(&session).unwrap(), SessionStatus::Idle);
    runner.send(&session, "continue", Origin::Owner).unwrap();
    runner.focus(&session).unwrap();
    assert_eq!(shared.sends()[0].text, "continue");
    assert_eq!(shared.sends()[0].session, session);
    assert_eq!(shared.focuses(), vec![session.clone()]);
    runner.set_status(&session, SessionStatus::Working);
    runner.stop(&session).unwrap();
    assert_eq!(shared.stops(), vec![session.clone()]);
    assert_eq!(shared.status(&session).unwrap(), SessionStatus::Gone);
    assert!(
        runner
            .send(&session, "too late", Origin::Auto)
            .unwrap_err()
            .is::<Refusal>()
    );
}

#[cfg(unix)]
struct Running(SessionRef);

#[cfg(unix)]
impl Drop for Running {
    fn drop(&mut self) {
        let _ = ProcessRunner.stop(&self.0);
    }
}

#[cfg(unix)]
fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "session did not reach the expected state"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
#[test]
fn process_appends_logs_inherits_env_and_records_exit_without_stdin() {
    let temp = crate::tests::tempdir();
    let mut launch = spec(
        temp.path(),
        r#"
if read -r line; then exit 99; fi
printf '%s\n' "$AGENT_MARKER"
printf 'stderr\n' >&2
while [ ! -e "$RELEASE" ]; do sleep 0.01; done
exit 7
"#,
    );
    let release = temp.path().join("release");
    launch.env = vec![
        ("AGENT_MARKER".to_owned(), "agent output".to_owned()),
        ("RELEASE".to_owned(), release.to_string_lossy().into_owned()),
    ];
    fs::create_dir_all(&launch.dir).unwrap();
    fs::write(launch.dir.join("session.log"), b"previous output\n").unwrap();
    let running = Running(ProcessRunner.start(&launch).unwrap());
    assert_eq!(
        ProcessRunner.status(&running.0).unwrap(),
        SessionStatus::Working
    );
    let error = ProcessRunner
        .send(&running.0, "input", Origin::Auto)
        .unwrap_err();
    assert_eq!(
        as_refusal(&error).unwrap().next,
        vec![Next {
            op: Op::Retry {
                id: "T-042".to_owned(),
                profile: None,
                fresh: false,
                note: Some("input".to_owned()),
            },
            force: false,
        }]
    );
    assert!(ProcessRunner.focus(&running.0).unwrap_err().is::<Refusal>());
    fs::write(release, b"go").unwrap();
    wait_until(|| ProcessRunner.status(&running.0).unwrap() == SessionStatus::Gone);
    assert_eq!(
        fs::read_to_string(launch.dir.join("exit_code")).unwrap(),
        "7\n"
    );
    let log = fs::read_to_string(launch.dir.join("session.log")).unwrap();
    assert_eq!(log, "previous output\nagent output\nstderr\n");
    ProcessRunner.stop(&running.0).unwrap();
    assert!(ProcessRunner.start(&launch).unwrap_err().is::<Refusal>());
}

#[cfg(unix)]
#[test]
fn process_stop_escalates_for_term_ignoring_group_and_records_completion() {
    let temp = crate::tests::tempdir();
    let mut launch = spec(
        temp.path(),
        r#"
trap '' TERM
printf 'ready\n' > "$READY"
exec sleep 10
"#,
    );
    let ready = temp.path().join("ready");
    launch
        .env
        .push(("READY".to_owned(), ready.to_string_lossy().into_owned()));
    let running = Running(ProcessRunner.start(&launch).unwrap());
    wait_until(|| ready.exists());
    ProcessRunner.stop(&running.0).unwrap();
    assert_eq!(
        ProcessRunner.status(&running.0).unwrap(),
        SessionStatus::Gone
    );
    assert!(launch.dir.join("exit_code").exists());
    ProcessRunner.stop(&running.0).unwrap();
    let invalid = SessionRef::Process {
        pid: 0,
        dir: temp.path().join("not-owned"),
    };
    assert!(ProcessRunner.stop(&invalid).is_err());
}
