use super::*;
use corral_types::{GateAttemptPage, GateAttemptState};
use std::{
    process::{Child, Stdio},
    thread,
    time::{Duration, Instant},
};

const ROOT: &str = "CORRAL_TEST_GATE_ATTEMPT_ROOT";
const WAIT: &str = "printf 'started-%s\\n' \"$MARKER\"; if test \"$PAUSE\" = yes; then i=0; until test -f \"$BARRIER\"; do i=$((i+1)); test \"$i\" -lt 200 || exit 1; sleep .05; done; printf 'released-%s\\n' \"$MARKER\"; fi";
const OUTPUT: &str =
    "head -c 12000 /dev/zero | tr '\\000' x; printf '\\nfinished-%s\\n' \"$MARKER\"";

struct Runner(Child);

impl Runner {
    fn start(root: &Path, task: &str, marker: &str, pause: bool) -> Self {
        Self(Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lifecycle_tests::gate_attempts_tests::quiet_gate_retry_and_runner_exit_are_observable", "--nocapture"])
            .env(ROOT, root)
            .env("CORRAL_TEST_GATE_ATTEMPT_TASK", task)
            .env("CORRAL_TEST_GATE_ATTEMPT_MARKER", marker)
            .env("CORRAL_TEST_GATE_ATTEMPT_PAUSE", if pause { "yes" } else { "no" })
            .stdin(Stdio::null())
            .spawn().unwrap())
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn until<T>(mut read: impl FnMut() -> Option<T>) -> T {
    let start = Instant::now();
    loop {
        if let Some(value) = read() {
            return value;
        }
        assert!(
            start.elapsed() < Duration::from_secs(8),
            "gate observation timed out"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn quiet_gate_retry_and_runner_exit_are_observable() {
    if let Some(root) = std::env::var_os(ROOT) {
        let id = std::env::var("CORRAL_TEST_GATE_ATTEMPT_TASK").unwrap();
        let marker = std::env::var("CORRAL_TEST_GATE_ATTEMPT_MARKER").unwrap();
        let pause = std::env::var("CORRAL_TEST_GATE_ATTEMPT_PAUSE").unwrap();
        let mut engine = Engine::open(&root).unwrap();
        engine.config.projects.insert(
            "project".into(),
            Project {
                path: "project".into(),
                profile: Some("generic".into()),
                gate: vec![WAIT.into(), OUTPUT.into()],
                env: [
                    ("MARKER".into(), marker.clone()),
                    ("PAUSE".into(), pause),
                    (
                        "BARRIER".into(),
                        PathBuf::from(root).join(marker).to_str().unwrap().into(),
                    ),
                ]
                .into(),
                ..Default::default()
            },
        );
        engine.pipeline = PipelineMode::Off;
        assert_eq!(engine.gate(&id).unwrap().result, CheckResult::Pass);
        return;
    }

    let cx = Fixture::new(&[]);
    let root = &cx.engine.root;
    let task = cx.backlog("observe a controlled gate");
    let run = cx.dispatch(&task);
    let sha = git(&run.worktree, &["rev-parse", "HEAD"]);
    let page = |id: &str, before| {
        cx.engine
            .gate_attempt_page(&task, id, before)
            .unwrap()
            .unwrap()
    };
    let wait_for_start = |marker: &str| -> GateAttemptPage {
        until(|| {
            let attempt = cx.engine.latest_gate_attempt(&task).unwrap()?;
            let page = page(&attempt.id, None);
            (page.text.contains(&format!("started-{marker}"))).then_some(page)
        })
    };

    let mut runner = Runner::start(root, &task, "first", true);
    let first = wait_for_start("first");
    let seq = cx.engine.store.latest_seq().unwrap();
    assert_eq!(first.attempt.state, GateAttemptState::Running);
    assert_eq!(first.attempt.command.as_deref(), Some(WAIT));
    assert_eq!((first.attempt.step, first.attempt.steps), (Some(1), 2));
    assert_eq!(
        (&first.attempt.sha, &first.attempt.run, first.attempt.round),
        (&sha, &run.id, 0)
    );
    let quiet = until(|| {
        let current = page(&first.attempt.id, None);
        (current.attempt.heartbeat > first.attempt.heartbeat).then_some(current)
    });
    assert_eq!(quiet.attempt.state, GateAttemptState::Running);
    assert_eq!(quiet.attempt.last_output, first.attempt.last_output);
    assert_eq!(quiet.total, first.total);
    assert_eq!(cx.engine.store.latest_seq().unwrap(), seq);
    assert!(quiet.attempt.check.is_none());

    fs::write(root.join("first"), "resume").unwrap();
    assert!(runner.0.wait().unwrap().success());
    let finished = page(&first.attempt.id, None);
    assert_eq!(finished.attempt.state, GateAttemptState::Passed);
    assert_eq!(finished.attempt.step, Some(2));
    assert_eq!(finished.end, finished.total);
    assert_eq!(finished.end - finished.start, 8 * 1024);
    assert!(finished.text.ends_with("finished-first\n"));
    let earlier = page(&first.attempt.id, Some(finished.start));
    assert_eq!(earlier.end, finished.start);
    assert!(earlier.text.contains("started-first"));
    let log = finished.attempt.check.unwrap().log.unwrap();
    let retained = cx.engine.show_artifact(&log).unwrap();
    assert_eq!(retained.len() as u64, finished.total);
    assert!(retained.ends_with(b"finished-first\n"));

    assert!(
        Runner::start(root, &task, "retry", false)
            .0
            .wait()
            .unwrap()
            .success()
    );
    let retry = cx.engine.latest_gate_attempt(&task).unwrap().unwrap();
    assert_eq!(retry.sha, sha);
    assert_ne!(retry.id, first.attempt.id);
    assert!(page(&retry.id, None).text.ends_with("finished-retry\n"));
    assert!(
        page(&first.attempt.id, None)
            .text
            .ends_with("finished-first\n")
    );
    assert!(
        cx.engine
            .gate_attempt_page("T-999", &retry.id, None)
            .unwrap()
            .is_none()
    );

    let mut runner = Runner::start(root, &task, "interrupted", true);
    let active = wait_for_start("interrupted");
    runner.0.kill().unwrap();
    assert!(!runner.0.wait().unwrap().success());
    // The shell may still hold inherited stdout/stderr. Only the runner owns the lease.
    let interrupted = page(&active.attempt.id, None);
    let premature_retry = cx.engine.gate(&task);
    fs::write(root.join("interrupted"), "release orphaned shell").unwrap();
    assert!(as_refusal(&premature_retry.unwrap_err()).is_some());
    assert_eq!(interrupted.attempt.state, GateAttemptState::Interrupted);
    assert!(interrupted.attempt.finished.is_some());
    assert!(interrupted.attempt.check.is_none());
    let raw = cx
        .engine
        .state_dir
        .join("runs")
        .join(&task)
        .join("gates")
        .join(&active.attempt.id)
        .join("output.log");
    until(|| {
        fs::read_to_string(&raw)
            .ok()
            .filter(|log| log.contains("released-interrupted"))
    });
    assert_eq!(page(&active.attempt.id, None), interrupted);
    assert_eq!(cx.events(&task, EventKind::Gate).len(), 2);
}
