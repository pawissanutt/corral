//! The pi_native adapter against a scripted bridge host that keeps the real host's contract:
//! one outcome per idempotency key per generation, and one `corral_delivery` entry written by
//! its delivery module for each new key. The real module and host are exercised by
//! packages/corral-pi/src/delivery/delivery.test.ts and its cross-lane fixture.
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use corral_core::inbox::{
    DeliveryMode, Inbox, MessageBinding, Post, ReceiptLevel, Recipient, TerminalState, Urgency,
};
use corral_core::{RunOutcome, Task};
use serde_json::{Value, json};

use super::*;
use crate::agent::BindTarget;
use crate::agent::launch::{ensure_private_dir, runtime_dir, runtime_root, write_ctl_cap};
use crate::agent::test_support::{self as ts, Fx};
use crate::delivery::{DeliveryStatus, deliver, owner};

#[derive(Default)]
struct Script {
    /// Close the next submit_input connection after admitting it, without a reply.
    close_after_admit: bool,
    /// Admit inputs but hold their entries until `flush`.
    hold_entries: bool,
    held: Vec<Value>,
    keys: HashMap<String, (String, Value)>,
    submits: Vec<Value>,
}

struct FakeHost {
    script: Arc<Mutex<Script>>,
    socket: PathBuf,
    session_file: PathBuf,
}

impl FakeHost {
    fn start(fx: &Fx, recipient: &Recipient, host: &ts::Host) -> FakeHost {
        let generation = host.identity.generation;
        let dir = generation_dir(&fx.ctx.store, recipient, generation).unwrap();
        ensure_private_dir(&runtime_root(), &dir).unwrap();
        write_ctl_cap(&dir, &host.ctl_cap).unwrap();
        let socket = dir.join(SOCKET_FILE);
        let listener = UnixListener::bind(&socket).unwrap();
        let session_file = host.sessions.join("session.jsonl");
        std::fs::create_dir_all(&host.sessions).unwrap();
        let script = Arc::new(Mutex::new(Script::default()));
        let shared = Arc::clone(&script);
        let session = host.identity.session_id.clone();
        let file = session_file.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let mut writer = stream.try_clone().unwrap();
                for line in BufReader::new(stream).lines().map_while(Result::ok) {
                    let frame: Value = serde_json::from_str(&line).unwrap();
                    let (mut answer, close) = respond(&shared, &frame, &file, &session, generation);
                    if close {
                        break;
                    }
                    answer["id"] = frame["id"].clone();
                    answer["generation"] = json!(generation);
                    answer["version"] = json!(1);
                    if writeln!(writer, "{answer}").is_err() {
                        break;
                    }
                }
            }
        });
        FakeHost {
            script,
            socket,
            session_file,
        }
    }

    fn script(&self) -> std::sync::MutexGuard<'_, Script> {
        self.script.lock().unwrap()
    }

    fn submits(&self) -> Vec<Value> {
        self.script().submits.clone()
    }

    fn flush(&self) {
        let held = std::mem::take(&mut self.script().held);
        for entry in held {
            append(&self.session_file, &entry);
        }
    }

    /// The host process is gone: nothing listens on the socket any more.
    fn kill(self) {
        std::fs::remove_file(&self.socket).unwrap();
    }

    fn entries(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.session_file)
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|entry| entry["customType"] == ENTRY_TYPE)
            .collect()
    }
}

fn append(path: &std::path::Path, entry: &Value) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(file, "{entry}").unwrap();
}

fn respond(
    script: &Mutex<Script>,
    frame: &Value,
    file: &std::path::Path,
    session: &str,
    generation: u32,
) -> (Value, bool) {
    let mut script = script.lock().unwrap();
    let args = &frame["args"];
    match frame["command"].as_str() {
        Some("hello") => (json!({"ok": true, "result": {"bound": true}}), false),
        Some("submit_input") => {
            script.submits.push(args.clone());
            let key = args["idempotency_key"].as_str().unwrap().to_owned();
            let text = args["text"].as_str().unwrap();
            let digest = observe::sha256(&format!("{text}|{}", args["busy_policy"]));
            let reply = |disposition: &str, replayed: bool| {
                json!({"ok": true, "result": {"request_id": frame["id"], "idempotency_key": key,
                    "disposition": disposition, "replayed": replayed}})
            };
            if let Some((prior, result)) = script.keys.get(&key) {
                if *prior != digest {
                    let mut answer = reply("rejected", true);
                    answer["result"]["reason"] = json!("idempotency_conflict");
                    return (answer, false);
                }
                let mut answer = reply(result["disposition"].as_str().unwrap(), true);
                answer["result"]["replayed"] = json!(true);
                return (answer, false);
            }
            let entry = json!({"type": "custom", "customType": ENTRY_TYPE,
                "id": format!("e{}", script.keys.len() + 1), "timestamp": "2026-10-24T00:00:00Z",
                "data": {"version": 1, "idempotency_key": key, "request_id": frame["id"],
                    "origin": args["origin"], "busy_policy": args["busy_policy"],
                    "text_sha256": observe::sha256(text), "text_bytes": text.len(),
                    "generation": generation, "session_id": session}});
            if script.hold_entries {
                script.held.push(entry);
            } else {
                append(file, &entry);
            }
            script
                .keys
                .insert(key.clone(), (digest, json!({"disposition": "queued"})));
            if std::mem::take(&mut script.close_after_admit) {
                return (Value::Null, true);
            }
            (reply("queued", false), false)
        }
        _ => (
            json!({"ok": false, "error": {"code": "unknown_command", "reason": "scripted"}}),
            false,
        ),
    }
}

/// Removes the runtime directories a test created under the shared runtime root.
struct Runtime(Vec<PathBuf>);

impl Drop for Runtime {
    fn drop(&mut self) {
        for dir in &self.0 {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn runtime(fx: &Fx, recipients: &[&Recipient]) -> Runtime {
    Runtime(
        recipients
            .iter()
            .map(|r| runtime_dir(&fx.ctx.store, r).unwrap())
            .collect(),
    )
}

fn bound(fx: &Fx, target: &BindTarget, pane: &str, session: &str) -> ts::Host {
    let host = ts::bind(fx, target, pane, session);
    ts::committed(&ts::hello(fx, &host));
    host
}

fn pi_native_worker(fx: &Fx) -> (String, Recipient, BindTarget) {
    let id = ts::worker_task(fx, "p1");
    fx.ctx
        .store
        .update(&id, |t| {
            t.runs[0].harness = Some("pi-native".into());
            t.runs[0].delivery = DeliveryMode::Inbox;
            Ok(())
        })
        .unwrap();
    let recipient = Recipient::Run {
        task: id.clone(),
        run: "R1".into(),
    };
    let target = BindTarget::Worker {
        task: id.clone(),
        run: "R1".into(),
    };
    (id, recipient, target)
}

fn post(fx: &Fx, recipient: &Recipient, kind: &str, body: &str) -> u64 {
    Inbox::new(&fx.ctx.store, recipient.clone())
        .unwrap()
        .post(Post {
            kind,
            urgency: Urgency::Wake,
            author: "owner",
            body,
            binding: MessageBinding::default(),
            subject: None,
            max_payload: corral_core::inbox::MAX_PAYLOAD,
        })
        .unwrap()
        .seq
}

fn observed(fx: &Fx, recipient: &Recipient, seq: u64) -> bool {
    owner::is_observed(&fx.ctx, recipient, seq).unwrap()
}

fn attempts(fx: &Fx, recipient: &Recipient) -> usize {
    Inbox::new(&fx.ctx.store, recipient.clone())
        .unwrap()
        .envelopes()
        .unwrap()
        .iter()
        .map(|e| e.attempts.len())
        .sum()
}

fn task(fx: &Fx, id: &str) -> Task {
    fx.ctx.store.get(id).unwrap()
}

#[test]
fn wire_keys_round_trip_and_pass_the_host_key_charset() {
    for key in ["T-144_R2@env7", "T-144_R1_rv-R1-review1@env12", "pm@env1"] {
        let wire = wire_key(key);
        assert!(
            wire.len() <= 128
                && wire
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b)),
            "{wire}"
        );
        let (recipient, number) = envelope_key(&wire).unwrap();
        assert_eq!(format!("{recipient}@env{number}"), key);
    }
    assert_eq!(envelope_key("xl-start"), None);
}

#[test]
fn owner_reply_changes_and_answer_reach_a_live_worker_and_done_waits_for_the_entry() {
    let fx = ts::fx();
    let (id, recipient, target) = pi_native_worker(&fx);
    let _runtime = runtime(&fx, &[&recipient]);
    let host = bound(&fx, &target, "p1", "s-worker");
    let fake = FakeHost::start(&fx, &recipient, &host);
    fake.script().hold_entries = true;
    let seqs = [
        post(&fx, &recipient, "owner_reply", "Use the typed payload."),
        post(
            &fx,
            &recipient,
            "changes_requested",
            "Fix the review finding.",
        ),
        post(&fx, &recipient, "question_answer", "Q1: yes."),
    ];

    // The registered adapter for a pi-native run is this one, through the owner kick.
    let status = owner::kick(&fx.ctx, &recipient).unwrap();
    assert!(
        matches!(
            status,
            DeliveryStatus::Waiting | DeliveryStatus::Sent { .. }
        ),
        "{status:?}"
    );
    let submits = fake.submits();
    assert_eq!(submits.len(), 1, "one envelope carries all three");
    let envelope = &Inbox::new(&fx.ctx.store, recipient.clone())
        .unwrap()
        .envelopes()
        .unwrap()[0];
    assert_eq!(envelope.adapter, ADAPTER);
    assert_eq!(envelope.members, seqs.to_vec());
    assert_eq!(
        submits[0]["idempotency_key"],
        json!(wire_key(&envelope.idempotency_key))
    );
    assert_eq!(submits[0]["origin"], json!(ORIGIN));
    assert_eq!(submits[0]["busy_policy"], json!("steer"));
    assert_eq!(
        observe::sha256(submits[0]["text"].as_str().unwrap()),
        envelope.payload_sha256
    );
    // Admitted is not observed: done refuses until the host's entry exists.
    assert!(seqs.iter().all(|seq| !observed(&fx, &recipient, *seq)));
    let refusal = owner::done_gate(&fx.ctx, &task(&fx, &id)).unwrap_err();
    assert!(format!("{refusal:#}").contains("owner_messages_unobserved"));

    fake.flush();
    owner::done_gate(&fx.ctx, &task(&fx, &id)).unwrap();
    for seq in seqs {
        assert!(observed(&fx, &recipient, seq));
        let receipt = Inbox::new(&fx.ctx.store, recipient.clone())
            .unwrap()
            .receipt(seq, ReceiptLevel::Observed)
            .unwrap()
            .unwrap();
        assert_eq!(receipt.via, ADAPTER);
        assert_eq!(receipt.detail["generation"], json!(1));
    }
    assert_eq!(
        fake.submits().len(),
        1,
        "an observed envelope is never resent"
    );
}

#[test]
fn a_reviewer_and_a_bound_pm_receive_through_the_same_adapter() {
    let fx = ts::fx();
    let (id, target) = ts::reviewer_task(&fx);
    let reviewer = Recipient::ReviewRun {
        task: id.clone(),
        run: "R1".into(),
        review: "R1-review1".into(),
    };
    let _runtime = runtime(&fx, &[&reviewer, &Recipient::Pm]);
    let host = bound(&fx, &target, "p2", "s-review");
    let fake = FakeHost::start(&fx, &reviewer, &host);
    fake.script().hold_entries = true;
    let answer = post(
        &fx,
        &reviewer,
        "question_answer",
        "Q1: the cited plan says yes.",
    );
    deliver(&fx.ctx, &reviewer, &PiNativeAdapter, None).unwrap();
    assert_eq!(fake.submits()[0]["busy_policy"], json!("follow_up"));
    assert!(!observed(&fx, &reviewer, answer));
    fake.flush();
    deliver(&fx.ctx, &reviewer, &PiNativeAdapter, None).unwrap();
    assert!(observed(&fx, &reviewer, answer));

    let pm = bound(
        &fx,
        &BindTarget::Pm {
            project: "corral".into(),
        },
        "pm-pane",
        "s-pm",
    );
    let pm_host = FakeHost::start(&fx, &Recipient::Pm, &pm);
    let wake = post(&fx, &Recipient::Pm, "question_wake", "T-001/R1/Q1 is open.");
    deliver(&fx.ctx, &Recipient::Pm, &PiNativeAdapter, None).unwrap();
    deliver(&fx.ctx, &Recipient::Pm, &PiNativeAdapter, None).unwrap();
    assert!(observed(&fx, &Recipient::Pm, wake));
    assert_eq!(pm_host.submits()[0]["busy_policy"], json!("follow_up"));
    assert!(
        pm_host.submits()[0]["idempotency_key"]
            .as_str()
            .unwrap()
            .starts_with("pm:env")
    );
}

#[test]
fn an_uncertain_send_resends_the_identical_key_and_the_host_writes_one_entry() {
    let fx = ts::fx();
    let (_, recipient, target) = pi_native_worker(&fx);
    let _runtime = runtime(&fx, &[&recipient]);
    let host = bound(&fx, &target, "p1", "s-worker");
    let fake = FakeHost::start(&fx, &recipient, &host);
    {
        let mut script = fake.script();
        script.close_after_admit = true;
        script.hold_entries = true;
    }
    let seq = post(&fx, &recipient, "owner_reply", "Continue.");

    deliver(&fx.ctx, &recipient, &PiNativeAdapter, None).unwrap();
    let inbox = Inbox::new(&fx.ctx.store, recipient.clone()).unwrap();
    let first = inbox.envelopes().unwrap().remove(0);
    assert!(
        matches!(first.outcome, Some(SendOutcome::Uncertain { .. })),
        "{:?}",
        first.outcome
    );
    deliver(&fx.ctx, &recipient, &PiNativeAdapter, None).unwrap();
    let submits = fake.submits();
    assert_eq!(submits.len(), 2, "the uncertain attempt is retried");
    assert_eq!(submits[0], submits[1], "identical payload, key and policy");
    assert!(
        fake.entries().is_empty(),
        "the replayed key writes no second entry"
    );
    fake.flush();
    deliver(&fx.ctx, &recipient, &PiNativeAdapter, None).unwrap();
    assert!(observed(&fx, &recipient, seq));
    assert_eq!(fake.entries().len(), 1);
    assert_eq!(inbox.envelopes().unwrap().len(), 1, "one envelope, one key");
}

#[test]
fn a_lost_host_keeps_a_never_sent_message_pending_for_the_next_generation() {
    let fx = ts::fx();
    let (_, recipient, target) = pi_native_worker(&fx);
    let _runtime = runtime(&fx, &[&recipient]);
    let first = bound(&fx, &target, "p1", "s-g1");
    FakeHost::start(&fx, &recipient, &first).kill();
    let seq = post(&fx, &recipient, "owner_reply", "Pick up where you stopped.");

    let status = deliver(&fx.ctx, &recipient, &PiNativeAdapter, None).unwrap();
    assert!(
        matches!(&status, DeliveryStatus::Blocked { reason } if reason.starts_with("host_unreachable")),
        "{status:?}"
    );
    assert_eq!(
        attempts(&fx, &recipient),
        0,
        "never attempted, so not uncertain"
    );
    let inbox = Inbox::new(&fx.ctx.store, recipient.clone()).unwrap();
    assert!(inbox.terminal(seq).unwrap().is_none());

    let second = bound(&fx, &target, "p1", "s-g2");
    assert_eq!(second.identity.generation, 2);
    let fake = FakeHost::start(&fx, &recipient, &second);
    deliver(&fx.ctx, &recipient, &PiNativeAdapter, None).unwrap();
    deliver(&fx.ctx, &recipient, &PiNativeAdapter, None).unwrap();
    assert!(observed(&fx, &recipient, seq));
    assert_eq!(fake.submits().len(), 1);
    let receipt = inbox.receipt(seq, ReceiptLevel::Observed).unwrap().unwrap();
    assert_eq!(receipt.detail["generation"], json!(2));
}

#[test]
fn an_uncertain_delivery_across_a_replacement_still_needs_the_owner_recover_decision() {
    let fx = ts::fx();
    let (_, recipient, target) = pi_native_worker(&fx);
    let _runtime = runtime(&fx, &[&recipient]);
    let first = bound(&fx, &target, "p1", "s-g1");
    let old = FakeHost::start(&fx, &recipient, &first);
    {
        let mut script = old.script();
        script.close_after_admit = true;
        script.hold_entries = true;
    }
    let seq = post(&fx, &recipient, "owner_reply", "Rebase first.");
    deliver(&fx.ctx, &recipient, &PiNativeAdapter, None).unwrap();
    old.kill();

    let second = bound(&fx, &target, "p1", "s-g2");
    let fake = FakeHost::start(&fx, &recipient, &second);
    let status = deliver(&fx.ctx, &recipient, &PiNativeAdapter, None).unwrap();
    assert_eq!(status, DeliveryStatus::Orphaned);
    assert!(
        fake.submits().is_empty(),
        "never transferred without a decision"
    );
    assert!(!observed(&fx, &recipient, seq));

    let inbox = Inbox::new(&fx.ctx.store, recipient.clone()).unwrap();
    inbox.recover(corral_core::inbox::Recovery::Resend).unwrap();
    deliver(&fx.ctx, &recipient, &PiNativeAdapter, None).unwrap();
    deliver(&fx.ctx, &recipient, &PiNativeAdapter, None).unwrap();
    assert_eq!(fake.submits().len(), 1);
    assert!(observed(&fx, &recipient, seq));
}

#[test]
fn a_closed_review_child_is_never_sent_its_messages_are_cancelled_recipient_closed() {
    let fx = ts::fx();
    let (id, target) = ts::reviewer_task(&fx);
    let reviewer = Recipient::ReviewRun {
        task: id.clone(),
        run: "R1".into(),
        review: "R1-review1".into(),
    };
    let _runtime = runtime(&fx, &[&reviewer]);
    let host = bound(&fx, &target, "p2", "s-review");
    let fake = FakeHost::start(&fx, &reviewer, &host);
    let answer = post(&fx, &reviewer, "question_answer", "Q1: no.");
    let note = post(&fx, &reviewer, "owner_note", "FYI.");
    fx.ctx
        .store
        .update(&id, |t| {
            t.runs[0].review_runs[0].outcome = Some(RunOutcome::Superseded);
            Ok(())
        })
        .unwrap();
    crate::questions::terminalize(&fx.ctx, &id).unwrap();
    let inbox = Inbox::new(&fx.ctx.store, reviewer.clone()).unwrap();
    assert_eq!(
        inbox.terminal(answer).unwrap(),
        Some(TerminalState::Cancelled)
    );

    let readiness = PiNativeAdapter.readiness(&fx.ctx, &reviewer).unwrap();
    assert!(
        matches!(&readiness, Readiness::Unavailable { reason } if reason.starts_with("recipient_closed")),
        "{readiness:?}"
    );
    let status = deliver(&fx.ctx, &reviewer, &PiNativeAdapter, None).unwrap();
    assert!(
        matches!(&status, DeliveryStatus::Blocked { reason } if reason.starts_with("recipient_closed")),
        "{status:?}"
    );
    assert!(fake.submits().is_empty());
    assert!(!observed(&fx, &reviewer, note));
    assert_eq!(attempts(&fx, &reviewer), 0);
}

#[test]
fn omo_and_legacy_harnesses_keep_their_launch_mode_and_never_reach_this_adapter() {
    crate::agent::hooks::take_consulted();
    assert_eq!(owner::launch_mode("omo"), DeliveryMode::Inbox);
    for harness in ["claude", "codex", "omp"] {
        assert_eq!(owner::launch_mode(harness), DeliveryMode::Legacy);
        assert_eq!(owner::review_launch_mode(harness), DeliveryMode::Legacy);
    }
    assert_eq!(owner::review_launch_mode("omo"), DeliveryMode::Legacy);
    let mut omo = ts::run("R1", "p1");
    omo.harness = Some("omo".into());
    assert!(crate::delivery::adapters::pi_native(&omo).is_none());
    assert!(crate::agent::hooks::take_consulted().is_empty());

    assert_eq!(owner::launch_mode("pi-native"), DeliveryMode::Inbox);
    omo.harness = Some("pi-native".into());
    let adapter = crate::delivery::adapters::pi_native(&omo).unwrap();
    assert_eq!(adapter.harness(), ADAPTER);
    assert!(adapter.capabilities().idempotent_retry);
    assert_eq!(adapter.capabilities().max_payload, INPUT_MAX_BYTES);
}

/// The host asks a blocking clarification question and returns its key.
fn ask(fx: &Fx, host: &ts::Host, call: &str) -> String {
    let request = crate::agent::tool_request_id(&host.identity.session_id, call);
    let payload = json!({"kind": "clarification", "blocking": true, "text": "Which payload?"});
    let input = ts::envelope(host, "ask", &request, payload, false);
    let receipt = ts::committed(&ts::send(fx, &input, "ask"));
    receipt.result["key"].as_str().unwrap().to_owned()
}

/// The PM (with a resolvable spec basis) or the owner answers through the question triage.
fn triage_answer(fx: &Fx, id: &str, key: &str, pm: bool) {
    use crate::questions::{Actor, AnswerInput};
    let revision = crate::questions::show(&fx.ctx, key)
        .unwrap()
        .question
        .revision;
    let (actor, input) = if pm {
        let spec = crate::progress_spec_revision(&task(fx, id)).unwrap();
        let input = AnswerInput {
            text: "Use the typed payload.".into(),
            option: None,
            basis_refs: vec![format!("spec:{id}@{spec}")],
            scope_note: Some("inside the approved write scope".into()),
        };
        (Actor::Pm(None), input)
    } else {
        let input = AnswerInput {
            text: "Use the typed payload.".into(),
            ..AnswerInput::default()
        };
        (Actor::Owner, input)
    };
    crate::questions::answer(&fx.ctx, key, revision, actor, input).unwrap();
}

fn answer_message(fx: &Fx, recipient: &Recipient) -> corral_core::inbox::Message {
    let mut found: Vec<_> = Inbox::new(&fx.ctx.store, recipient.clone())
        .unwrap()
        .messages()
        .unwrap()
        .into_iter()
        .filter(|m| m.kind == "question_answer")
        .collect();
    assert_eq!(found.len(), 1, "one triage answer is queued");
    found.remove(0)
}

/// One bridge submit carries the answer and the host's own entry observes it.
fn assert_answer_submitted(fx: &Fx, recipient: &Recipient, fake: &FakeHost, generation: u64) {
    let message = answer_message(fx, recipient);
    deliver(&fx.ctx, recipient, &PiNativeAdapter, None).unwrap();
    deliver(&fx.ctx, recipient, &PiNativeAdapter, None).unwrap();
    let submits = fake.submits();
    assert_eq!(submits.len(), 1, "exactly one submit_input");
    assert!(
        submits[0]["text"].as_str().unwrap().contains(&message.body),
        "the envelope carries the answer"
    );
    assert_eq!(submits[0]["origin"], json!(ORIGIN));
    let entries = fake.entries();
    assert_eq!(entries.len(), 1, "one corral_delivery entry");
    assert_eq!(
        entries[0]["data"]["idempotency_key"],
        submits[0]["idempotency_key"]
    );
    assert!(observed(fx, recipient, message.seq));
    let inbox = Inbox::new(&fx.ctx.store, recipient.clone()).unwrap();
    let receipt = inbox
        .receipt(message.seq, ReceiptLevel::Observed)
        .unwrap()
        .unwrap();
    assert_eq!(receipt.via, ADAPTER);
    assert_eq!(receipt.detail["generation"], json!(generation));
}

#[test]
fn a_pm_or_owner_answer_to_a_worker_question_is_submitted_to_the_host() {
    for pm in [true, false] {
        let fx = ts::fx();
        let (id, recipient, target) = pi_native_worker(&fx);
        let _runtime = runtime(&fx, &[&recipient]);
        let host = bound(&fx, &target, "p1", "s-worker");
        let fake = FakeHost::start(&fx, &recipient, &host);
        let key = ask(&fx, &host, "call-ask");
        triage_answer(&fx, &id, &key, pm);
        assert_answer_submitted(&fx, &recipient, &fake, 1);
    }
}

#[test]
fn an_owner_answer_to_a_review_child_question_is_submitted_to_the_host() {
    let fx = ts::fx();
    let (id, target) = ts::reviewer_task(&fx);
    let reviewer = Recipient::ReviewRun {
        task: id.clone(),
        run: "R1".into(),
        review: "R1-review1".into(),
    };
    let _runtime = runtime(&fx, &[&reviewer]);
    let host = bound(&fx, &target, "p2", "s-review");
    let fake = FakeHost::start(&fx, &reviewer, &host);
    let key = ask(&fx, &host, "call-ask");
    triage_answer(&fx, &id, &key, false);
    assert_answer_submitted(&fx, &reviewer, &fake, 1);
}

#[test]
fn an_answer_queued_while_the_host_is_lost_is_delivered_to_the_resumed_generation() {
    let fx = ts::fx();
    let (id, recipient, target) = pi_native_worker(&fx);
    let _runtime = runtime(&fx, &[&recipient]);
    let first = bound(&fx, &target, "p1", "s-g1");
    let key = ask(&fx, &first, "call-ask");
    FakeHost::start(&fx, &recipient, &first).kill();
    triage_answer(&fx, &id, &key, true);

    let status = deliver(&fx.ctx, &recipient, &PiNativeAdapter, None).unwrap();
    assert!(
        matches!(&status, DeliveryStatus::Blocked { reason } if reason.starts_with("host_unreachable")),
        "{status:?}"
    );
    assert_eq!(attempts(&fx, &recipient), 0, "never attempted while lost");

    let second = bound(&fx, &target, "p1", "s-g2");
    assert_eq!(second.identity.generation, 2);
    let fake = FakeHost::start(&fx, &recipient, &second);
    assert_answer_submitted(&fx, &recipient, &fake, 2);
}
