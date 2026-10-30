//! Crash cuts through real processes (T-600 review F1). The executor is a `corral agent call`
//! child process armed at failpoints, where it announces itself and blocks until released or
//! SIGKILLed. The command child runs the real claim, writes attributed preparation events like
//! approval's rebase and verdict carry, then lands, fails or hangs as told, synchronizing with
//! the test through FIFOs. No step waits on time.
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use corral_core::agent::ReceiptState;
use corral_core::{Actor, Status, Store};
use serde_json::{Value, json};

use super::tests::{Session, receipt, session};
use crate::agent::bridge::fixture as f;
use crate::assistant::owner_action::{OWNER_ACTION_ENV, claim, finish_in};

const DEADLINE: Duration = Duration::from_secs(120);
const FAILURE: &str = "landing_stale: the base moved during landing";

#[test]
#[ignore = "entry point of the owner-action executor child process"]
fn owner_action_executor_child() {
    let Ok(spec) = std::env::var("CORRAL_T600_EXECUTOR") else {
        return;
    };
    let spec: Value = serde_json::from_str(&spec).unwrap();
    let ctx = f::ctx_at(Path::new(spec["root"].as_str().unwrap()));
    let program = PathBuf::from(spec["program"].as_str().unwrap());
    crate::assistant::owner_action::EXECUTABLE.with(|path| *path.borrow_mut() = Some(program));
    let envelope = spec["envelope"].as_str().unwrap().as_bytes();
    let response = crate::agent::call(&ctx, "owner_action", envelope).unwrap();
    println!("\nRESULT {}", response.to_json().unwrap());
}

#[test]
#[ignore = "entry point of the owner-action command child process"]
fn owner_action_command_child() {
    let Ok(spec) = std::env::var("CORRAL_T600_COMMAND") else {
        return;
    };
    let spec: Value = serde_json::from_str(&spec).unwrap();
    let store = Store::open_unobserved(Path::new(spec["root"].as_str().unwrap())).unwrap();
    let argv: Vec<String> = serde_json::from_value(spec["argv"].clone()).unwrap();
    let value = std::env::var(OWNER_ACTION_ENV).unwrap();
    let via = claim(&store, &value, &argv, std::os::unix::process::parent_id())
        .unwrap()
        .expect("the executor's direct child claims its reservation");
    let session = format!("assistant:{}", via.session);
    store.set_attribution(Actor::Owner, &session).unwrap();
    let authority = serde_json::to_value(&via).unwrap();
    store
        .set_attribution_detail("owner_authority", authority)
        .unwrap();
    let task = argv[1].clone();
    for kind in ["candidate_rebased", "verdict_carried"] {
        store.log_event(Some(&task), kind, json!({})).unwrap();
    }
    let land = || {
        store
            .update(&task, |task| {
                task.status = Status::Done;
                Ok(())
            })
            .unwrap();
        store.log_event(Some(&task), "approved", json!({})).unwrap();
    };
    match spec["script"].as_str().unwrap() {
        "land" => {
            land();
            finish_in(&store, &via, None).unwrap();
        }
        "land_then_die" => {
            land();
            std::process::abort();
        }
        "fail" => {
            store
                .log_event(Some(&task), "landing_stale", json!({}))
                .unwrap();
            finish_in(&store, &via, Some(FAILURE)).unwrap();
            eprintln!("Error: {FAILURE}");
            std::process::exit(1);
        }
        _ => {
            let mut ready = std::fs::OpenOptions::new()
                .write(true)
                .open(spec["ready"].as_str().unwrap())
                .unwrap();
            writeln!(ready, "{}", std::process::id()).unwrap();
            drop(ready);
            let mut go = std::fs::File::open(spec["go"].as_str().unwrap()).unwrap();
            let mut ignored = String::new();
            let _ = go.read_to_string(&mut ignored);
            std::process::abort();
        }
    }
}

struct Executor {
    child: Child,
    lines: mpsc::Receiver<String>,
}

impl Executor {
    fn expect(&self, marker: &str) -> String {
        let deadline = std::time::Instant::now() + DEADLINE;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    if let Some((_, rest)) = line.split_once(marker) {
                        return rest.trim().to_owned();
                    }
                }
                Err(error) => panic!("the executor never printed {marker}: {error}"),
            }
        }
    }

    fn kill(mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}

fn mkfifo(path: &Path) {
    let status = Command::new("mkfifo").arg(path).status().unwrap();
    assert!(status.success());
}

/// The executor for `call` with `payload`, armed at `points`; its command child follows
/// `script`.
fn spawn(s: &Session, call: &str, payload: &Value, script: &str, points: &str) -> Executor {
    let request = s.request(call);
    let envelope = crate::agent::test_support::envelope(
        &s.bound,
        "owner_action",
        &request,
        payload.clone(),
        false,
    );
    let root = s.fx.root();
    let program = root.join(format!("command-{call}.sh"));
    let test = std::env::current_exe().unwrap();
    crate::agent::test_support::write_executable(
        &program,
        &format!(
            "#!/bin/sh\nexec '{}' --exact agent::ops::owner_action::crash_tests::owner_action_command_child \
             --ignored --nocapture --test-threads 1\n",
            test.display()
        ),
    );
    let task = payload["target"].as_str().unwrap();
    let command = json!({"root": root, "argv": ["approve", task], "script": script,
        "ready": root.join("ready"), "go": root.join("go")});
    let executor = json!({"root": root, "program": program,
        "envelope": String::from_utf8(envelope).unwrap()});
    let mut child = Command::new(&test)
        .args([
            "--exact",
            "agent::ops::owner_action::crash_tests::owner_action_executor_child",
            "--ignored",
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env("CORRAL_T600_EXECUTOR", executor.to_string())
        .env("CORRAL_T600_COMMAND", command.to_string())
        .env("CORRAL_AGENT_FAILPOINT", points)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    Executor { child, lines }
}

fn approvals(s: &Session, task: &str, request: &str) -> usize {
    s.fx.ctx
        .store
        .events()
        .unwrap()
        .iter()
        .filter(|event| {
            event.kind == "approved"
                && event.task.as_deref() == Some(task)
                && event.detail["owner_authority"]["request"].as_str() == Some(request)
        })
        .count()
}

fn status(s: &Session, task: &str) -> Status {
    s.fx.ctx.store.get(task).unwrap().status
}

fn setup() -> (Session, String, Value) {
    let mut s = session();
    let task = s.task(Status::Review);
    let input = s.journal("T-5 looks good, please land it", None);
    let payload = json!({"kind": "approve", "target": task, "input": input});
    (s, task, payload)
}

#[test]
fn an_executor_killed_after_its_reservation_runs_no_command() {
    // Given an executor stopped right after it reserved the approval.
    let (s, task, payload) = setup();
    let executor = spawn(
        &s,
        "early",
        &payload,
        "land",
        "owner_action_reserved_before_run",
    );
    executor.expect("FAILPOINT owner_action_reserved_before_run");
    // When it is killed there and the host replays the call.
    executor.kill();
    let (replayed, _) = receipt(&s.act("early", &payload));
    // Then the receipt is not executed and the task never landed.
    assert_eq!(replayed.state, ReceiptState::Abandoned, "{replayed:?}");
    assert_eq!(replayed.result["outcome"], "not_executed");
    assert_eq!(status(&s, &task), Status::Review);
}

#[test]
fn a_command_killed_after_its_preparation_events_is_uncertain_never_success() {
    // Given a command child that claimed the approval and wrote its preparation events.
    let (s, task, payload) = setup();
    let ready = s.fx.root().join("ready");
    mkfifo(&ready);
    mkfifo(&s.fx.root().join("go"));
    let executor = spawn(&s, "prepared", &payload, "hang", "");
    let mut pid = String::new();
    std::fs::File::open(&ready)
        .unwrap()
        .read_to_string(&mut pid)
        .unwrap();
    // When the command is killed before its effect.
    let killed = Command::new("kill")
        .args(["-9", pid.trim()])
        .status()
        .unwrap();
    assert!(killed.success());
    // Then the executor settles it uncertain: events before the effect prove nothing.
    let result: Value = serde_json::from_str(&executor.expect("RESULT")).unwrap();
    assert_eq!(
        (
            result["state"].as_str(),
            result["result"]["outcome"].as_str()
        ),
        (Some("uncertain"), Some("outcome_unknown")),
        "{result}"
    );
    assert_eq!(status(&s, &task), Status::Review);
    // And a replay returns that receipt and runs nothing again.
    let (replayed, again) = receipt(&s.act("prepared", &payload));
    assert!(again);
    assert_eq!(replayed.state, ReceiptState::Uncertain);
}

#[test]
fn an_executor_killed_after_a_landed_command_replays_executed_once() {
    // Given a command child that landed the task and recorded its completion.
    let (s, task, payload) = setup();
    let executor = spawn(
        &s,
        "landed",
        &payload,
        "land",
        "owner_action_ran_before_settle",
    );
    executor.expect("FAILPOINT owner_action_ran_before_settle");
    // When the executor is killed before it settles the receipt, and the host replays twice.
    executor.kill();
    let (first, _) = receipt(&s.act("landed", &payload));
    let (second, replayed) = receipt(&s.act("landed", &payload));
    // Then the child's own record settles it executed, once, with one landing.
    assert_eq!(first.state, ReceiptState::Committed, "{first:?}");
    assert_eq!(first.result["reconciled"], "completion_record");
    assert!(replayed && second == first);
    assert_eq!(status(&s, &task), Status::Done);
    assert_eq!(approvals(&s, &task, &s.request("landed")), 1);
}

#[test]
fn a_failed_landing_after_its_events_settles_refused_never_success() {
    // Given a command child that rebased, then failed its landing and recorded the failure.
    let (s, task, payload) = setup();
    let executor = spawn(
        &s,
        "stale",
        &payload,
        "fail",
        "owner_action_ran_before_settle",
    );
    executor.expect("FAILPOINT owner_action_ran_before_settle");
    // When the executor is killed before it settles, and the host replays.
    executor.kill();
    let (replayed, _) = receipt(&s.act("stale", &payload));
    // Then the receipt is refused with the command's error, and nothing landed.
    assert_eq!(replayed.state, ReceiptState::Rejected, "{replayed:?}");
    assert_eq!(
        (
            replayed.result["outcome"].as_str(),
            replayed.result["reason"].as_str()
        ),
        (Some("refused"), Some(FAILURE))
    );
    assert_eq!(status(&s, &task), Status::Review);
    assert_eq!(approvals(&s, &task, &s.request("stale")), 0);
}

#[test]
fn a_command_dying_after_landing_without_a_record_settles_from_the_landed_task() {
    // Given a command child that landed the task and died before recording its completion.
    let (s, task, payload) = setup();
    let executor = spawn(&s, "died", &payload, "land_then_die", "");
    // When the executor sees it die.
    let result: Value = serde_json::from_str(&executor.expect("RESULT")).unwrap();
    // Then only the verified terminal state (Done, approved for this request) settles it.
    assert_eq!(
        (
            result["state"].as_str(),
            result["result"]["reconciled"].as_str()
        ),
        (Some("committed"), Some("target_state")),
        "{result}"
    );
    assert_eq!(approvals(&s, &task, &s.request("died")), 1);
}
