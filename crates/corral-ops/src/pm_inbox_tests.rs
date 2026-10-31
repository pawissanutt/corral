use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::thread::JoinHandle;

use corral_core::agent::{self, AgentPm};
use corral_core::inbox::{Inbox, ReceiptLevel, Recipient};
use corral_core::pm_inbox::{ItemKind, NewPmItem, NoticeState, Resolution, WAKE_KIND};
use serde_json::{Value, json};

use super::*;
use crate::agent::BindTarget;
use crate::agent::test_support::{self as ts, Fx};

fn assistant() -> Recipient {
    Recipient::Assistant {
        session: "A-1".into(),
    }
}

/// Session A-1 at generation 1, bound and healthy after its host's hello.
fn live_assistant(fx: &Fx) -> ts::Host {
    fx.ctx
        .store
        .create_assistant("pi", None, Some("asst-pane"), None)
        .unwrap();
    let host = ts::bind(
        fx,
        &BindTarget::Assistant {
            session: "A-1".into(),
        },
        "asst-pane",
        "s-asst",
    );
    ts::committed(&ts::hello(fx, &host));
    host
}

fn post(fx: &Fx, from: &str) {
    fx.ctx
        .store
        .pm_inbox_post(NewPmItem {
            from: from.into(),
            kind: ItemKind::Routing,
            subject: "Land whatever is ready".into(),
            body: "The owner's request names no task.".into(),
            refs: vec![],
            requested_action: None,
            owner_words: None,
            key: None,
        })
        .unwrap();
}

fn claim_and_resolve(fx: &Fx) {
    fx.ctx.store.pm_inbox_claim("PMI-1", 0, None).unwrap();
    resolve(
        &fx.ctx,
        "PMI-1",
        1,
        (Resolution::Done, "Landed T-5.", vec!["T-5".into()]),
        None,
    )
    .unwrap();
}

/// A private bridge on the assistant's generation socket that speaks the production delivery
/// protocol and records every submit_input; never a provider.
struct Bridge {
    dir: PathBuf,
    socket: PathBuf,
    thread: Option<JoinHandle<Vec<Value>>>,
}

impl Bridge {
    fn start(fx: &Fx, host: &ts::Host) -> Self {
        use crate::agent::launch::{
            ensure_private_dir, generation_dir, runtime_root, write_ctl_cap,
        };
        let dir = generation_dir(&fx.ctx.store, &assistant(), 1).unwrap();
        ensure_private_dir(&runtime_root(), &dir).unwrap();
        write_ctl_cap(&dir, &host.ctl_cap).unwrap();
        let socket = dir.join("host.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        std::fs::create_dir_all(&host.sessions).unwrap();
        let session_file = host.sessions.join("session.jsonl");
        let session = host.identity.session_id.clone();
        let thread = std::thread::spawn(move || {
            let mut submits = Vec::new();
            for stream in listener.incoming() {
                let stream = stream.unwrap();
                let mut writer = stream.try_clone().unwrap();
                for line in BufReader::new(stream).lines() {
                    let frame: Value = serde_json::from_str(&line.unwrap()).unwrap();
                    if frame["stop"] == true {
                        return submits;
                    }
                    let result = match frame["command"].as_str().unwrap() {
                        "hello" => json!({"bound": true}),
                        "submit_input" => {
                            let args = &frame["args"];
                            submits.push(args.clone());
                            let text = args["text"].as_str().unwrap();
                            let entry = json!({"type":"custom", "customType":"corral_delivery",
                                "id":"e1", "timestamp":"2026-10-30T00:00:00Z",
                                "data":{"version":1, "idempotency_key":args["idempotency_key"],
                                    "request_id":frame["id"], "origin":args["origin"],
                                    "busy_policy":args["busy_policy"], "generation":1,
                                    "session_id":session,
                                    "text_sha256":agent::paths::sha256_hex(text.as_bytes()),
                                    "text_bytes":text.len()}});
                            let mut file = std::fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(&session_file)
                                .unwrap();
                            writeln!(file, "{entry}").unwrap();
                            json!({"request_id":frame["id"], "idempotency_key":args["idempotency_key"],
                                "disposition":"started", "turn":1, "replayed":false})
                        }
                        other => panic!("unexpected bridge command {other}"),
                    };
                    writeln!(
                        writer,
                        "{}",
                        json!({"version":1, "id":frame["id"], "generation":1, "ok":true, "result":result})
                    )
                    .unwrap();
                }
            }
            submits
        });
        Self {
            dir,
            socket,
            thread: Some(thread),
        }
    }

    fn stop(&mut self) -> Vec<Value> {
        writeln!(
            UnixStream::connect(&self.socket).unwrap(),
            "{{\"stop\":true}}"
        )
        .unwrap();
        self.thread.take().unwrap().join().unwrap()
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.stop();
        }
        std::fs::remove_dir_all(&self.dir).unwrap();
    }
}

#[test]
fn c7_resolve_starts_one_native_turn_in_the_live_author_and_sync_never_repeats_it() {
    // Given live assistant A-1 behind a provider-free bridge, with its item claimed by the PM.
    let fx = ts::fx();
    let host = live_assistant(&fx);
    let mut bridge = Bridge::start(&fx, &host);
    post(&fx, "assistant:A-1");
    // When the PM resolves it, and sync passes run twice more.
    claim_and_resolve(&fx);
    sweep(&fx.ctx).unwrap();
    sweep(&fx.ctx).unwrap();
    // Then exactly one native input named pmi:PMI-1:2 reached A-1, as a follow-up turn.
    let submits = bridge.stop();
    assert_eq!(submits.len(), 1);
    assert_eq!(submits[0]["busy_policy"], "follow_up");
    let text = submits[0]["text"].as_str().unwrap();
    assert!(text.contains("pmi:PMI-1:2"), "{text}");
    assert!(text.contains("Landed T-5."), "{text}");
    let inbox = Inbox::new(&fx.ctx.store, assistant()).unwrap();
    let messages = inbox.messages().unwrap();
    assert_eq!(messages.len(), 1);
    assert!(
        inbox
            .receipt(messages[0].seq, ReceiptLevel::Observed)
            .unwrap()
            .is_some()
    );
    let notice = fx.ctx.store.pm_inbox_item("PMI-1").unwrap().notice.unwrap();
    assert_eq!(notice.state, NoticeState::Posted);
    // And nothing was queued for the PM: no Pi-native PM is bound.
    assert!(
        Inbox::new(&fx.ctx.store, Recipient::Pm)
            .unwrap()
            .messages()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_notice_owed_while_the_author_host_is_unreachable_is_delivered_once_by_sync() {
    // Given A-1 bound but with no host listening when the PM resolves its item.
    let fx = ts::fx();
    let host = live_assistant(&fx);
    post(&fx, "assistant:A-1");
    claim_and_resolve(&fx);
    let inbox = Inbox::new(&fx.ctx.store, assistant()).unwrap();
    assert_eq!(inbox.messages().unwrap().len(), 1);
    assert!(inbox.receipt(1, ReceiptLevel::Observed).unwrap().is_none());
    // When the host comes back and sync passes run.
    let mut bridge = Bridge::start(&fx, &host);
    sweep(&fx.ctx).unwrap();
    sweep(&fx.ctx).unwrap();
    // Then the queued notice was delivered exactly once.
    let submits = bridge.stop();
    assert_eq!(submits.len(), 1);
    assert!(submits[0]["text"].as_str().unwrap().contains("pmi:PMI-1:2"));
    assert_eq!(inbox.messages().unwrap().len(), 1);
}

fn bound_pm(fx: &Fx) -> AgentPm {
    let host = ts::bind(
        fx,
        &BindTarget::Pm {
            project: "corral".into(),
        },
        "pm-pane",
        "s-pm",
    );
    ts::committed(&ts::hello(fx, &host));
    pm_state::load(&fx.ctx.store).unwrap().unwrap()
}

#[test]
fn a_healthy_pi_native_pm_gets_one_wake_per_open_item_and_none_after_a_claim() {
    // Given a healthy Pi-native PM and two owner items, one already claimed.
    let fx = ts::fx();
    bound_pm(&fx);
    post(&fx, "owner");
    post(&fx, "owner");
    fx.ctx.store.pm_inbox_claim("PMI-2", 0, None).unwrap();
    // When sync passes run twice.
    sweep(&fx.ctx).unwrap();
    sweep(&fx.ctx).unwrap();
    // Then the PM inbox holds one wake, keyed to the open item's revision.
    let messages = Inbox::new(&fx.ctx.store, Recipient::Pm)
        .unwrap()
        .messages()
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].kind, WAKE_KIND);
    assert_eq!(messages[0].subject.as_ref().unwrap()["key"], "pmi:PMI-1:0");
}

#[test]
fn without_a_pi_native_pm_the_sweep_posts_no_wake() {
    // Given an OmO-style workspace: no Pi-native PM record, one open item.
    let fx = ts::fx();
    post(&fx, "owner");
    // When sync passes run.
    sweep(&fx.ctx).unwrap();
    // Then nothing is queued anywhere; the item waits for `corral pm next`.
    assert!(
        Inbox::new(&fx.ctx.store, Recipient::Pm)
            .unwrap()
            .messages()
            .unwrap()
            .is_empty()
    );
    assert!(
        fx.ctx
            .store
            .pm_inbox_item("PMI-1")
            .unwrap()
            .pm_wake
            .is_none()
    );
}

#[test]
fn an_interrupted_pi_pm_call_reconciles_from_the_item_history() {
    // Given one claim committed by request req-1.
    let fx = ts::fx();
    post(&fx, "owner");
    fx.ctx
        .store
        .pm_inbox_claim("PMI-1", 0, Some("req-1"))
        .unwrap();
    // When its receipt and an unrelated request are reconciled.
    let (state, result) = reconciled(&fx.ctx, "req-1", "PMI-1").unwrap();
    let (other, _) = reconciled(&fx.ctx, "req-2", "PMI-1").unwrap();
    // Then only the marked request committed; the other never started.
    assert_eq!(state, ReceiptState::Committed);
    assert_eq!(result["revision"], 1);
    assert_eq!(other, ReceiptState::Abandoned);
}
