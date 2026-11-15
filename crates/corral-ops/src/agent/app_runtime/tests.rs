//! Consumer-visible boundaries: keyed sends cannot replace words, escaped text cannot stall paging.
use super::*;
use corral_protocol::assistant::EntryKind;

fn message(session: &str, text: String) -> Message {
    Message {
        id: String::new(),
        session: session.into(),
        seq: 0,
        cmd_id: "send-1".into(),
        device: "device-1".into(),
        name: "Owner".into(),
        target: "assistant".into(),
        text,
        typed_at: "2026-11-14T00:00:00Z".into(),
        at: "2026-11-14T00:00:00Z".into(),
        state: "sent".into(),
        input: None,
        pm_item: None,
        saved_task: None,
        reply: None,
        failure: None,
        turn: None,
        generation: None,
        late: false,
        tainted: false,
        model: None,
        context: Vec::new(),
        attachments: Vec::new(),
        rev: 1,
    }
}

#[test]
fn keyed_send_preserves_original_words_and_rejects_payload_reuse() {
    let fx = crate::agent::test_support::fx();
    let head = fx.ctx.store.app_thread("quick").unwrap();
    let original = message(&head.session, "Owner's original words".into());
    let (first, replayed) = fx
        .ctx
        .store
        .append_app_message(original.clone(), "digest-1", &[])
        .unwrap();
    assert!(!replayed);
    let (again, replayed) = fx
        .ctx
        .store
        .append_app_message(original, "digest-1", &[])
        .unwrap();
    assert!(replayed);
    assert_eq!(again.id, first.id);
    assert!(
        fx.ctx
            .store
            .append_app_message(message(&head.session, "different".into()), "digest-2", &[])
            .is_err()
    );
    assert_eq!(
        fx.ctx.store.app_message(&first.id).unwrap().text,
        "Owner's original words"
    );
    assert_eq!(fx.ctx.store.app_thread_head(&head.session).unwrap().next, 2);
}

#[test]
fn escaped_and_multibyte_text_pages_progress_without_losing_bytes() {
    let fx = crate::agent::test_support::fx();
    let head = fx.ctx.store.app_thread("quick").unwrap();
    let owner = "\u{0001}".repeat(16 * 1024);
    let answer = "界".repeat(7000);
    let (saved, _) = fx
        .ctx
        .store
        .append_app_message(message(&head.session, owner.clone()), "digest", &[])
        .unwrap();
    fx.ctx
        .store
        .update_app_message(&saved.id, |m| {
            m.reply = Some(answer.clone());
            m.state = "done".into();
            Ok(())
        })
        .unwrap();
    let mut before = None;
    let mut pages = Vec::new();
    loop {
        let (rows, next, _) = thread(&fx.ctx, "quick", before, 100).unwrap();
        assert!(!rows.is_empty());
        assert!(serde_json::to_vec(&rows).unwrap().len() <= 54 * 1024 + 100);
        pages.push(rows);
        let Some(next) = next else {
            break;
        };
        assert!(before.is_none_or(|old| next < old));
        before = Some(next);
    }
    let rows: Vec<_> = pages.into_iter().rev().flatten().collect();
    let rendered = |kind| {
        rows.iter()
            .filter(|row| row.kind == kind)
            .filter_map(|row| row.text.as_deref())
            .collect::<String>()
    };
    assert_eq!(rendered(EntryKind::Message), owner);
    assert_eq!(rendered(EntryKind::Answer), answer);
}

fn device(ctx: &Ctx) -> AuthenticatedDevice {
    let device = AuthenticatedDevice {
        id: "device-1".into(),
        name: "Owner".into(),
        assistant: true,
        assistant_act: false,
        reply: true,
        act: true,
    };
    corral_core::records::RecordArea::<serde_json::Value>::new("devices")
        .save(
            &ctx.store,
            &device.id,
            &serde_json::json!({
                "name": device.name, "revoked_at": null,
                "authority": {"assistant": true, "assistant_act": false, "reply": true, "act": true}
            }),
        )
        .unwrap();
    device
}

fn request(session: &str, cmd: &str, target: AskTarget) -> AssistantSend {
    AssistantSend {
        cmd_id: cmd.into(),
        thread: session.into(),
        target,
        text: "Owner's message".into(),
        typed_at: jiff::Timestamp::now().to_string(),
        context: Vec::new(),
        attachments: Vec::new(),
    }
}

#[cfg(target_os = "linux")]
struct PauseHost {
    fx: crate::agent::test_support::Fx,
    session: String,
    device: AuthenticatedDevice,
    _host: crate::agent::bridge::fixture::HostProc,
    bridge: crate::agent::bridge::fixture::Bridge,
    events: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
    cancel_results: std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<serde_json::Value>>>,
}

#[cfg(target_os = "linux")]
impl PauseHost {
    fn new(open: bool) -> Self {
        use crate::agent::{admission, bridge::fixture as f, test_support as ts};
        use serde_json::json;
        use std::sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        };

        let fx = ts::fx();
        let device = device(&fx.ctx);
        let session = fx.ctx.store.app_thread("quick").unwrap().session;
        let target = crate::agent::BindTarget::Assistant {
            session: session.clone(),
        };
        let mut bound = ts::bind(&fx, &target, "owner-pane", "assistant-session");
        let grant =
            admission::open_generation_host(&fx.ctx.store, &target, None, false, None, None, false)
                .unwrap();
        bound.identity.generation = grant.generation;
        bound.nonce = grant.bind_nonce;
        bound.host_cap = ts::secret("remote-host", grant.generation);
        bound.ctl_cap = ts::secret("remote-ctl", grant.generation);
        let host = f::HostProc::spawn(true);
        let payload = json!({
            "ctl_cap_sha256": corral_core::agent::paths::sha256_hex(bound.ctl_cap.as_bytes()),
            "session_file": bound.sessions.join("session.jsonl"),
            "pid": host.pid(), "start_time": host.start_time(), "pane": null,
        });
        let hello_request = crate::agent::event_request_id(bound.identity.generation, 0);
        ts::committed(&ts::send(
            &fx,
            &ts::envelope(&bound, "hello", &hello_request, payload, true),
            "hello",
        ));
        let events = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
        let queued_events = events.clone();
        let active = AtomicBool::new(open);
        let cancel_results = Arc::new(Mutex::new(
            std::collections::VecDeque::<serde_json::Value>::new(),
        ));
        let queued_cancels = cancel_results.clone();
        let bridge = f::bridge(
            &fx.ctx.store,
            &corral_core::inbox::Recipient::Assistant {
                session: session.clone(),
            },
            grant.generation,
            Some(&bound.ctl_cap),
            move |frame| {
                let result = match frame["command"].as_str().unwrap() {
                    "hello" => json!({"bound": true, "remote_ready": true}),
                    "get_state" => json!({"bound": true,
                        "turn": {"current": 7, "open": active.load(Ordering::SeqCst)}}),
                    "cancel" => {
                        if let Some(reply) = queued_cancels.lock().unwrap().pop_front() {
                            if reply["cancelled"] == true {
                                active.store(false, Ordering::SeqCst);
                            }
                            reply
                        } else if active.swap(false, Ordering::SeqCst) {
                            json!({"cancelled": true, "turn": 7})
                        } else {
                            json!({"cancelled": false, "reason": "idle"})
                        }
                    }
                    "events_since" => {
                        let after = frame["args"]["seq"].as_u64().unwrap();
                        let events: Vec<_> = queued_events
                            .lock()
                            .unwrap()
                            .iter()
                            .filter(|event| event["seq"].as_u64().unwrap() > after)
                            .cloned()
                            .collect();
                        let next = events
                            .last()
                            .map_or(after, |event| event["seq"].as_u64().unwrap());
                        json!({"generation": frame["generation"], "events": events,
                            "next": next, "last": next, "more": false})
                    }
                    "owner_turn" => json!({"disposition": "started"}),
                    other => panic!("unexpected bridge command {other}"),
                };
                json!({"ok": true, "result": result})
            },
        );
        Self {
            fx,
            session,
            device,
            _host: host,
            bridge,
            events,
            cancel_results,
        }
    }
}

#[cfg(target_os = "linux")]
impl Drop for PauseHost {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(crate::agent::launch::assistant_runtime_dir(
            &self.fx.ctx.store,
            &self.session,
        ));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn queued_assistant_message_is_not_delivered_while_paused() {
    let host = PauseHost::new(false);
    let ctx = &host.fx.ctx;
    let ack = send(
        ctx,
        &host.device,
        &request(&host.session, "queued", AskTarget::Assistant),
    )
    .unwrap();
    let mut runtime = start(ctx, &host.session).unwrap();
    let row = session(
        ctx,
        &host.device,
        &AssistantSessionOp::Pause {
            session: host.session.clone(),
        },
    )
    .unwrap();
    assert_eq!(row.state, corral_protocol::rows::AssistantState::Paused);
    assert_eq!(row.pending, 1);
    warm(ctx, &host.device, &host.session).unwrap();
    assert!(!pending_scopes(ctx).unwrap().contains(&host.session));
    let before = ctx.store.app_message(&ack.entry).unwrap();
    pump(ctx, &mut runtime).unwrap();
    ctx.store
        .set_app_thread_state_unless(&host.session, "ready", None, &["paused"])
        .unwrap();
    assert_eq!(
        ctx.store.app_thread_head(&host.session).unwrap().state,
        "paused"
    );
    let after = ctx.store.app_message(&ack.entry).unwrap();
    assert_eq!(
        (after.state, after.failure, after.late),
        (before.state, before.failure, before.late)
    );
    assert!(
        !host
            .bridge
            .commands()
            .iter()
            .any(|frame| frame["command"] == "owner_turn")
    );
    session(
        ctx,
        &host.device,
        &AssistantSessionOp::Resume {
            session: host.session.clone(),
        },
    )
    .unwrap();
    pump(ctx, &mut runtime).unwrap();
    assert_eq!(ctx.store.app_message(&ack.entry).unwrap().state, "working");
    assert!(
        host.bridge
            .commands()
            .iter()
            .any(|frame| frame["command"] == "owner_turn")
    );
}

#[test]
fn owner_assistant_send_does_not_leave_thread_paused() {
    let fx = crate::agent::test_support::fx();
    let device = device(&fx.ctx);
    let head = fx.ctx.store.app_thread("quick").unwrap();
    assert!(!lift_pause_for_owner_input(&fx.ctx, &head.session).unwrap());
    let unthreaded = fx
        .ctx
        .store
        .create_assistant("pi", None, None, None)
        .unwrap();
    assert!(!lift_pause_for_owner_input(&fx.ctx, &unthreaded.id).unwrap());
    fx.ctx
        .store
        .set_app_thread_state(&head.session, "paused", None)
        .unwrap();
    let pm = request(&head.session, "pm", AskTarget::Pm);
    send(&fx.ctx, &device, &pm).unwrap();
    assert_eq!(
        fx.ctx.store.app_thread_head(&head.session).unwrap().state,
        "paused"
    );
    let owner = request(&head.session, "owner", AskTarget::Assistant);
    let ack = send(&fx.ctx, &device, &owner).unwrap();
    assert_eq!(
        fx.ctx.store.app_thread_head(&head.session).unwrap().state,
        "cold"
    );
    assert!(pending_scopes(&fx.ctx).unwrap().contains(&head.session));
    fx.ctx
        .store
        .set_app_thread_state(&head.session, "paused", None)
        .unwrap();
    assert!(send(&fx.ctx, &device, &owner).unwrap().replayed);
    assert_eq!(
        fx.ctx.store.app_thread_head(&head.session).unwrap().state,
        "paused"
    );
    assert_eq!(
        fx.ctx.store.app_message(&ack.entry).unwrap().text,
        owner.text
    );
    assert!(lift_pause_for_owner_input(&fx.ctx, &head.session).unwrap());
    assert_eq!(
        fx.ctx.store.app_thread_head(&head.session).unwrap().state,
        "cold"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn pausing_does_not_shutdown_host_or_fence_generation() {
    let host = PauseHost::new(true);
    let ctx = &host.fx.ctx;
    let ack = send(
        ctx,
        &host.device,
        &request(&host.session, "in-flight", AskTarget::Assistant),
    )
    .unwrap();
    let mut runtime = start(ctx, &host.session).unwrap();
    let before = ctx.store.get_assistant_container(&host.session).unwrap();
    let generation = before.state.binding.generation;
    let message = ctx
        .store
        .update_app_message(&ack.entry, |message| {
            message.state = "working".into();
            message.turn = Some(7);
            message.generation = Some(generation);
            Ok(())
        })
        .unwrap();
    session(
        ctx,
        &host.device,
        &AssistantSessionOp::Pause {
            session: host.session.clone(),
        },
    )
    .unwrap();
    host.events.lock().unwrap().push(serde_json::json!({
        "seq": 1, "kind": "owner_done", "entry": message.id,
        "input": message.input, "turn": 7, "aborted": true,
    }));
    let update = pump(ctx, &mut runtime).unwrap();
    assert!(!update.exited);
    assert!(update.live.iter().any(|live| live.done && live.turn == 7));
    assert_eq!(ctx.store.app_message(&ack.entry).unwrap().state, "done");
    assert_eq!(
        ctx.store.app_thread_head(&host.session).unwrap().state,
        "paused"
    );
    let after = ctx.store.get_assistant_container(&host.session).unwrap();
    assert_eq!(after.state.binding.generation, generation);
    assert!(after.state.current_generation().unwrap().fenced.is_none());
    assert!(crate::agent::pi::host_alive(&after.state).unwrap());
    assert!(host.bridge.dir.is_dir());
    let frames = host.bridge.commands();
    assert!(
        frames
            .iter()
            .any(|frame| frame["command"] == "cancel" && frame["args"]["turn"] == 7)
    );
    assert!(!frames.iter().any(|frame| frame["command"] == "shutdown"));
    assert!(lift_pause_for_owner_input(ctx, &host.session).unwrap());
    let row = projection::row(ctx, &host.session).unwrap();
    assert_eq!(row.state, corral_protocol::rows::AssistantState::Ready);
}

#[path = "audit_tests.rs"]
mod audit;
