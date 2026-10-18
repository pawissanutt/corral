use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::{fs::PermissionsExt, net::UnixListener};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;

use corral_core::{Run, Status, Store};
use serde_json::{Value, json};

use super::*;

const REQUEST: &str = "call-full|native-request-1";
const ANSWER: &str = "Keep the complete first thirty characters.\nSecond line: {{literal}} \"quoted\".\n\nLast line [Pasted text #9 +4 lines]";

struct NativeState {
    received: Vec<String>,
    lose_ack: bool,
    refuse: bool,
}

struct Fixture {
    temp: tempfile::TempDir,
    ctx: Ctx,
    task: Task,
    session: PathBuf,
    state: Arc<Mutex<NativeState>>,
    stopped: Arc<AtomicBool>,
    socket: PathBuf,
    worker: Option<JoinHandle<()>>,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let cwd = temp.path().join("app");
        std::fs::create_dir(&cwd).unwrap();
        let agent = temp.path().join("omo");
        let session_dir = agent.join("sessions").join(crate::omo::encode_cwd(&cwd));
        std::fs::create_dir_all(&session_dir).unwrap();
        let session = session_dir.join("session.jsonl");
        std::fs::write(&session, initial(&cwd)).unwrap();
        let socket = temp.path().join("q.sock");
        std::fs::write(format!("{}.secret", socket.display()), [7_u8; 32]).unwrap();
        let registry = agent.join("rpc-host-daemon/endpoint");
        std::fs::create_dir_all(registry.join("generations/instance")).unwrap();
        for (path, value) in [
            (
                registry.join("endpoint.json"),
                json!({"endpoint_kind":"tui","socket":socket}),
            ),
            (registry.join("host.pid"), json!({"instance_id":"instance"})),
            (
                registry.join("generations/instance/host.pid"),
                json!({"pid":4242,"instance_id":"instance","socket":socket}),
            ),
        ] {
            std::fs::write(path, value.to_string()).unwrap();
        }
        let engine = temp.path().join("herdr");
        std::fs::write(&engine, r#"#!/bin/sh
[ "$1" != --session ] || shift 2
case "$1 $2" in
"pane process-info") printf '%s\n' '{"result":{"process_info":{"shell_pid":100,"foreground_processes":[{"pid":4242,"argv":["omo"]}]}}}' ;;
"pane get") printf '%s\n' '{"result":{"pane":{"pane_id":"w:p","workspace_id":"w","agent":"omo","agent_status":"blocked"}}}' ;;
"pane read") printf '> [Pasted text #9 +4 lines]\n' ;;
*) printf '%s\n' "$*" >> "$(dirname "$0")/unexpected-terminal-write"
   printf '%s\n' '{"error":{"code":"unexpected_terminal","message":"native question must not receive terminal input"}}' ;;
esac
"#).unwrap();
        std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut store = Store::init(temp.path()).unwrap();
        store.config.sync.omo_agent_dir = Some(agent);
        let ctx = Ctx {
            store,
            herdr: corral_herdr::Herdr::with_bin(engine, Some("private-question-fixture".into())),
            resource_probe: crate::resources::FixedResourceProbe::arc(),
        };
        let task = ctx
            .store
            .create("Native question", None, Status::NeedsMe, "")
            .unwrap();
        let run: Run = serde_json::from_value(json!({
            "id":"R1", "agent":"omo", "harness":"omo", "worktree":cwd,
            "branch":"corral/T-fixture", "base":"main", "base_sha":"0",
            "started":"2026-10-18T00:00:00Z",
            "herdr":{"workspace_id":"w", "pane_id":"w:p", "socket":ctx.herdr.socket_path().unwrap()}
        }))
        .unwrap();
        let task = ctx
            .store
            .update(&task.id, |task| {
                task.runs.push(run);
                Ok(())
            })
            .unwrap();
        let listener = UnixListener::bind(&socket).unwrap();
        let state = Arc::new(Mutex::new(NativeState {
            received: Vec::new(),
            lose_ack: false,
            refuse: false,
        }));
        let stopped = Arc::new(AtomicBool::new(false));
        let shared = state.clone();
        let stopping = stopped.clone();
        let transcript = session.clone();
        let worker = std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                if stopping.load(Ordering::SeqCst) {
                    break;
                }
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                    .unwrap();
                let mut secret = [0; 32];
                if stream.read_exact(&mut secret).is_err() {
                    continue;
                }
                assert_eq!(secret, [7; 32]);
                let mut reader = BufReader::new(stream);
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap() == 0 {
                        break;
                    }
                    let command: Value = serde_json::from_str(&line).unwrap();
                    let mut response = json!({"id":command["id"],"type":"response","command":command["type"],"success":true});
                    match command["type"].as_str().unwrap() {
                        "get_protocol_info" => {
                            response["data"] = json!({"mode":"tui","instanceId":"instance","commands":["extension_ui_response"]})
                        }
                        "list_sessions" => {
                            response["data"] = json!({"sessions":[{"sessionId":"session","sessionPath":transcript,"cwd":cwd}]})
                        }
                        "extension_ui_response" => {
                            let mut state = shared.lock().unwrap();
                            if state.refuse {
                                response["success"] = false.into();
                                response["error"] = "fixture refusal".into();
                            } else if command["uiRequestId"] != REQUEST
                                || !state.received.is_empty()
                            {
                                response["success"] = false.into();
                                response["error"] = "unknown_request".into();
                            } else {
                                let text = command["comment"].as_str().unwrap();
                                state.received.push(text.into());
                                let mut file = std::fs::OpenOptions::new()
                                    .append(true)
                                    .open(&transcript)
                                    .unwrap();
                                writeln!(file, "{}", json!({"type":"custom","customType":"ask-user:settlement","data":{"requestId":REQUEST,"status":"comment-submitted"}})).unwrap();
                                writeln!(file, "{}", json!({"type":"message","message":{"role":"user","content":[
                                    {"type":"text","text":format!("[Answer to question {REQUEST}]\nThe user responded: (see comment below)")},
                                    {"type":"text","text":format!("[The user's comment for question {REQUEST}]")},
                                    {"type":"text","text":text.trim()}
                                ]}})).unwrap();
                                if state.lose_ack {
                                    break;
                                }
                            }
                        }
                        other => panic!("unexpected native command {other}"),
                    }
                    writeln!(reader.get_mut(), "{response}").unwrap();
                }
            }
        });
        Self {
            temp,
            ctx,
            task,
            session,
            state,
            stopped,
            socket,
            worker: Some(worker),
        }
    }

    fn receipt(&self) -> Value {
        self.ctx
            .store
            .events()
            .unwrap()
            .into_iter()
            .rev()
            .find(|e| e.kind == "replied")
            .unwrap()
            .detail
    }

    fn planner(mut self) -> Self {
        let cwd = self.task.current_run().unwrap().worktree.clone();
        self.ctx.store.config.projects.insert(
            "app".into(),
            corral_core::Project {
                dispatch: Default::default(),
                path: cwd.display().to_string(),
                default_profile: None,
                base: None,
                review: Default::default(),
                artifact_retention: None,
                post_land: Vec::new(),
            },
        );
        self.ctx.store.save_config().unwrap();
        self.task = self.ctx.store.update(&self.task.id, |task| {
            task.project = Some("app".into());
            task.current_run_mut().unwrap().outcome = Some(corral_core::RunOutcome::Done);
            task.plan_runs.push(serde_json::from_value(json!({
                "id":"D1", "profile":"omo", "harness":"omo",
                "spec_revision":crate::plan_spec_revision(task).unwrap(),
                "started":"2026-10-18T00:00:00Z", "state":"planning",
                "output_dir":self.temp.path().join("planner"), "rounds":0,
                "herdr":{"workspace_id":"w","pane_id":"w:p","socket":self.ctx.herdr.socket_path().unwrap()}
            })).unwrap());
            Ok(())
        }).unwrap();
        self
    }

    fn pending(&self) -> Option<String> {
        self.ctx
            .store
            .get(&self.task.id)
            .unwrap()
            .current_run()
            .unwrap()
            .pending_reply
            .clone()
    }

    fn no_terminal_writes(&self) {
        assert!(!self.temp.path().join("unexpected-terminal-write").exists());
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        let _wake = std::os::unix::net::UnixStream::connect(&self.socket).unwrap();
        self.worker.take().unwrap().join().unwrap();
    }
}

fn initial(cwd: &std::path::Path) -> String {
    [
        json!({"type":"session","id":"session","cwd":cwd,"timestamp":"2026-10-18T00:00:01Z"}),
        json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","id":REQUEST,"name":"request_user_input","arguments":{"questions":[{"id":"choice","header":"Choice","question":"Choose?","options":[{"label":"A"},{"label":"B"}]}],"wait_for_answer":false}}]}}),
        json!({"type":"custom","customType":"ask-user:question","data":{"requestId":REQUEST,"headers":["Choice"]}}),
        json!({"type":"message","message":{"role":"toolResult","toolCallId":REQUEST,"details":{"accepted":true,"status":"pending","requestId":REQUEST},"content":[{"type":"text","text":"Question accepted"}]}}),
    ].iter().map(|line| format!("{line}\n")).collect()
}

#[test]
fn collapsed_multiline_reply_settles_exact_request_without_terminal_input() {
    // Given a real socket protocol fixture and a collapsed own-answer draft.
    let fixture = Fixture::new();
    // When the supported reply operation delivers the complete answer.
    let saved = crate::reply(&fixture.ctx, &fixture.task.id, ANSWER).unwrap();
    // Then one request gets the exact full payload, with truthful delivery metadata.
    assert_eq!(fixture.state.lock().unwrap().received, [ANSWER]);
    assert!(saved.current_run().unwrap().pending_reply.is_none());
    assert_eq!(fixture.receipt()["via"], "omo_question");
    assert_eq!(fixture.receipt()["submitted"], true);
    assert_eq!(fixture.receipt()["attempts"], 1);
    fixture.no_terminal_writes();
}

#[test]
fn lost_ack_resend_reconciles_settlement_without_second_submission() {
    // Given an accepted native answer whose acknowledgement connection was lost.
    let fixture = Fixture::new();
    fixture.state.lock().unwrap().lose_ack = true;
    assert!(crate::reply(&fixture.ctx, &fixture.task.id, ANSWER).is_err());
    assert_eq!(fixture.pending().as_deref(), Some(ANSWER));
    assert_eq!(fixture.receipt()["submitted"], false);
    assert_eq!(fixture.receipt()["attempts"], 1);
    // When the supported resend operation reconciles the durable exact-request answer.
    crate::resend_reply(&fixture.ctx, &fixture.task.id).unwrap();
    // Then it reports observed settlement, not a second mutating command.
    assert_eq!(fixture.state.lock().unwrap().received, [ANSWER]);
    assert!(fixture.pending().is_none());
    assert_eq!(fixture.receipt()["submitted"], true);
    assert_eq!(fixture.receipt()["attempts"], 0);
    fixture.no_terminal_writes();
}

#[test]
fn refused_answer_resends_only_the_bound_request() {
    // Given a refusal with the exact pending answer retained.
    let fixture = Fixture::new();
    fixture.state.lock().unwrap().refuse = true;
    assert!(crate::reply(&fixture.ctx, &fixture.task.id, ANSWER).is_err());
    fixture.state.lock().unwrap().refuse = false;
    // When resend retries the same request.
    crate::resend_reply(&fixture.ctx, &fixture.task.id).unwrap();
    // Then the native request is answered once without a duplicate paste.
    assert_eq!(fixture.state.lock().unwrap().received, [ANSWER]);
    fixture.no_terminal_writes();
}

#[test]
fn replaced_question_or_options_cannot_inherit_a_pending_answer() {
    for replace_id in [true, false] {
        // Given a prepared reply followed by a request or option-identity replacement.
        let fixture = Fixture::new();
        fixture.state.lock().unwrap().refuse = true;
        assert!(crate::reply(&fixture.ctx, &fixture.task.id, ANSWER).is_err());
        let original = std::fs::read_to_string(&fixture.session).unwrap();
        std::fs::write(
            &fixture.session,
            if replace_id {
                original.replace(REQUEST, "replacement-request")
            } else {
                original.replace("\"label\":\"B\"", "\"label\":\"C\"")
            },
        )
        .unwrap();
        fixture.state.lock().unwrap().refuse = false;
        // When resend sees the changed native identity.
        assert!(crate::resend_reply(&fixture.ctx, &fixture.task.id).is_err());
        // Then the full pending answer remains and no question is submitted.
        assert!(fixture.state.lock().unwrap().received.is_empty());
        assert_eq!(fixture.pending().as_deref(), Some(ANSWER));
        assert_eq!(fixture.receipt()["submitted"], false);
        assert_eq!(fixture.receipt()["attempts"], 0);
        fixture.no_terminal_writes();
    }
}

#[test]
fn unrelated_settlement_does_not_claim_our_answer_was_submitted() {
    // Given a refusal followed by someone else's answer to the same question.
    let fixture = Fixture::new();
    fixture.state.lock().unwrap().refuse = true;
    assert!(crate::reply(&fixture.ctx, &fixture.task.id, ANSWER).is_err());
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&fixture.session)
        .unwrap();
    writeln!(file, "{}", json!({"type":"custom","customType":"ask-user:settlement","data":{"requestId":REQUEST,"status":"comment-submitted"}})).unwrap();
    // When our pending answer is resent.
    assert!(crate::resend_reply(&fixture.ctx, &fixture.task.id).is_err());
    // Then settlement alone neither clears pending metadata nor submits unrelated input.
    assert_eq!(fixture.pending().as_deref(), Some(ANSWER));
    assert_eq!(fixture.receipt()["attempts"], 0);
    fixture.no_terminal_writes();
}

#[test]
fn menu_reply_is_delivered_as_literal_comment_not_a_guessed_selection() {
    // Given a question offering menu choices.
    let fixture = Fixture::new();
    // When corral replies with the literal label of one choice.
    crate::reply(&fixture.ctx, &fixture.task.id, "A").unwrap();
    // Then that full text reaches the native request without key-driven menu selection.
    assert_eq!(fixture.state.lock().unwrap().received, ["A"]);
    assert!(fixture.pending().is_none());
    fixture.no_terminal_writes();
}

#[test]
fn lost_ack_reconciliation_does_not_answer_a_new_pending_question() {
    // Given an accepted answer with lost ack and a subsequent native question.
    let fixture = Fixture::new();
    fixture.state.lock().unwrap().lose_ack = true;
    assert!(crate::reply(&fixture.ctx, &fixture.task.id, ANSWER).is_err());
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&fixture.session)
        .unwrap();
    writeln!(file, "{}", json!({"type":"message","message":{"role":"assistant","content":[
        {"type":"toolCall","id":"next-question","name":"request_user_input","arguments":{"questions":[{"id":"next","options":[]}],"wait_for_answer":false}}
    ]}})).unwrap();
    writeln!(file, "{}", json!({"type":"custom","customType":"ask-user:question","data":{"requestId":"next-question"}})).unwrap();
    // When resend reconciles the original binding, not the newest question.
    crate::resend_reply(&fixture.ctx, &fixture.task.id).unwrap();
    // Then only the original request has received text.
    assert_eq!(fixture.state.lock().unwrap().received, [ANSWER]);
    let snapshot = transcript::Snapshot::read(&fixture.session).unwrap();
    assert!(snapshot.pending.contains_key("next-question"));
    assert_eq!(fixture.receipt()["attempts"], 0);
    fixture.no_terminal_writes();
}

#[test]
fn blocking_question_tool_result_is_sufficient_for_lost_ack_reconciliation() {
    // Given a bound request whose blocking tool result carries the native answer.
    let fixture = Fixture::new();
    fixture.state.lock().unwrap().refuse = true;
    assert!(crate::reply(&fixture.ctx, &fixture.task.id, ANSWER).is_err());
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&fixture.session)
        .unwrap();
    writeln!(
        file,
        "{}",
        json!({"type":"message","message":{"role":"toolResult","toolCallId":REQUEST,
        "details":{"status":"comment-submitted","resolvedBy":"control_endpoint","comment":ANSWER},
        "content":[{"type":"text","text":"result"}]}})
    )
    .unwrap();
    // When resend inspects that exact-request terminal result.
    crate::resend_reply(&fixture.ctx, &fixture.task.id).unwrap();
    // Then it clears pending without requiring an async-only lifecycle settlement.
    assert!(fixture.pending().is_none());
    assert_eq!(fixture.receipt()["submitted"], true);
    assert_eq!(fixture.receipt()["attempts"], 0);
    fixture.no_terminal_writes();
}

#[test]
fn unsupported_native_question_retains_full_answer_without_terminal_fallback() {
    // Given a native question on a harness without a supported endpoint.
    let fixture = Fixture::new();
    std::fs::remove_dir_all(fixture.temp.path().join("omo/rpc-host-daemon")).unwrap();
    // When reply encounters that unsupported native surface.
    assert!(crate::reply(&fixture.ctx, &fixture.task.id, ANSWER).is_err());
    // Then it leaves a bounded manual action and the complete pending answer.
    assert_eq!(fixture.pending().as_deref(), Some(ANSWER));
    assert_eq!(fixture.receipt()["submitted"], false);
    assert_eq!(fixture.receipt()["attempts"], 0);
    assert!(fixture.state.lock().unwrap().received.is_empty());
    fixture.no_terminal_writes();
}

#[test]
fn prepared_ordinary_reply_cannot_become_a_native_question_answer_on_resend() {
    // Given an ordinary reply prepared before the current native question appeared.
    let fixture = Fixture::new();
    fixture
        .ctx
        .store
        .log_event(
            Some(&fixture.task.id),
            "reply_prepared",
            json!({
                "run":"R1","pane":"w:p","text":ANSWER,"question":null
            }),
        )
        .unwrap();
    fixture
        .ctx
        .store
        .update(&fixture.task.id, |task| {
            task.current_run_mut().unwrap().pending_reply = Some(ANSWER.into());
            Ok(())
        })
        .unwrap();
    // When that ordinary reply is resent into a now-question-bearing pane.
    assert!(crate::resend_reply(&fixture.ctx, &fixture.task.id).is_err());
    // Then it cannot submit either the unrelated composer or the new question.
    assert_eq!(fixture.pending().as_deref(), Some(ANSWER));
    assert_eq!(fixture.receipt()["attempts"], 0);
    assert!(fixture.state.lock().unwrap().received.is_empty());
    fixture.no_terminal_writes();
}

#[test]
fn bound_native_resend_rechecks_herdr_ownership_before_submission() {
    // Given a bound pending answer and a different server with colliding pane IDs.
    let mut fixture = Fixture::new();
    fixture.state.lock().unwrap().refuse = true;
    assert!(crate::reply(&fixture.ctx, &fixture.task.id, ANSWER).is_err());
    fixture.ctx.herdr.set_session(Some("foreign-server".into()));
    fixture.state.lock().unwrap().refuse = false;
    // When resend is invoked through that foreign server.
    assert!(crate::resend_reply(&fixture.ctx, &fixture.task.id).is_err());
    // Then neither the native request nor a terminal composer receives input.
    assert!(fixture.state.lock().unwrap().received.is_empty());
    assert_eq!(fixture.pending().as_deref(), Some(ANSWER));
    assert_eq!(fixture.receipt()["attempts"], 0);
    fixture.no_terminal_writes();
}

#[test]
fn planner_reply_uses_the_same_request_bound_native_transport() {
    // Given a live owned planner with a native question in the project directory.
    let fixture = Fixture::new().planner();
    // When the supported reply path selects that planner.
    let saved = crate::reply(&fixture.ctx, &fixture.task.id, ANSWER).unwrap();
    // Then the exact answer reaches the request once with planner delivery metadata.
    assert_eq!(fixture.state.lock().unwrap().received, [ANSWER]);
    assert!(saved.plan_runs.last().unwrap().pending_reply.is_none());
    let event = fixture
        .ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .find(|event| event.kind == "plan_replied")
        .unwrap();
    assert_eq!(event.detail["run"], "D1");
    assert_eq!(event.detail["via"], "omo_question");
    assert_eq!(event.detail["submitted"], true);
    fixture.no_terminal_writes();
}

#[test]
fn planner_lost_ack_resend_reconciles_without_a_second_submission() {
    // Given a planner answer whose native acknowledgement was lost.
    let fixture = Fixture::new().planner();
    fixture.state.lock().unwrap().lose_ack = true;
    assert!(crate::reply(&fixture.ctx, &fixture.task.id, ANSWER).is_err());
    assert_eq!(
        fixture
            .ctx
            .store
            .get(&fixture.task.id)
            .unwrap()
            .plan_runs
            .last()
            .unwrap()
            .pending_reply
            .as_deref(),
        Some(ANSWER)
    );
    // When the planner's supported resend path reconciles the same request.
    let saved = crate::resend_reply(&fixture.ctx, &fixture.task.id).unwrap();
    // Then the native answer is not duplicated or sent to the previous coding run.
    assert_eq!(fixture.state.lock().unwrap().received, [ANSWER]);
    assert!(saved.plan_runs.last().unwrap().pending_reply.is_none());
    let event = fixture
        .ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .rev()
        .find(|event| event.kind == "plan_replied")
        .unwrap();
    assert_eq!(event.detail["attempts"], 0);
    assert_eq!(event.detail["submitted"], true);
    fixture.no_terminal_writes();
}
