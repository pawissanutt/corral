use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;

use corral_core::agent::{ContainerRef, HostHealth};
use corral_core::inbox::{
    DeliveryMode, Envelope, Inbox, MessageBinding, ObserveCursor, Receipt, ReceiptLevel, Recipient,
    SendOutcome, SessionBinding,
};
use serde_json::{Value, json};

use super::*;
use crate::agent::delivery::CorralPiAdapter;
use crate::agent::test_support::{self as ts, Fx};
use crate::delivery::{
    BindOutcome, Capabilities, DeliveryAdapter, DeliveryStatus, Observation, Readiness,
    RecipientRef,
};
use crate::session_control::{keys, pause};

fn worker() -> (Fx, Target, Recipient) {
    let fx = ts::fx();
    let id = ts::worker_task(&fx, "p1");
    fx.ctx
        .store
        .update(&id, |task| {
            task.runs[0].harness = Some("corral-pi".into());
            task.runs[0].delivery = DeliveryMode::Inbox;
            Ok(())
        })
        .unwrap();
    let container = ContainerRef::Worker {
        task: id.clone(),
        run: "R1".into(),
    };
    let target = Target {
        id: format!("{id}/run/R1"),
        role: "worker".into(),
        task: Some(id.clone()),
        container: Some(container.clone()),
        pane: Some("p1".into()),
        generation: Some(1),
        health: HostHealth::Healthy,
        bridge: true,
        ended: false,
        adopted: false,
        owned: true,
        turn: None,
        replyable: true,
        record_key: container.key(),
    };
    let recipient = Recipient::Run {
        task: id,
        run: "R1".into(),
    };
    (fx, target, recipient)
}

#[test]
fn sending_to_paused_worker_lifts_fence_before_durable_reply_publication() {
    let (fx, target, recipient) = worker();
    pause::RECORDS
        .save(
            &fx.ctx.store,
            &target.record_key,
            &pause::PauseRecord {
                id: target.id.clone(),
                generation: target.generation,
                at: fx.ctx.store.now(),
                actor: Some(corral_core::Actor::Owner),
                task: target.task.clone(),
                hold: None,
            },
        )
        .unwrap();
    let too_large = "x".repeat(crate::agent::delivery::INPUT_MAX_BYTES + 1);
    assert!(
        send(&fx.ctx, &target, &too_large, "steer")
            .unwrap_err()
            .to_string()
            .starts_with("invalid_params:")
    );
    assert!(pause::recipient_paused(&fx.ctx, &recipient).unwrap());
    let result = send(
        &fx.ctx,
        &target,
        "Continue with these owner words",
        "follow-up",
    )
    .unwrap();
    assert_eq!(result["pause_lifted"], true);
    assert_eq!(result["delivery"], "pending");
    assert!(!pause::recipient_paused(&fx.ctx, &recipient).unwrap());
    let inbox = Inbox::new(&fx.ctx.store, recipient).unwrap();
    let messages = inbox.messages().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].kind, "owner_reply");
    assert_eq!(owner::text(&messages[0]), "Continue with these owner words");
    assert_eq!(
        messages[0].subject.as_ref().unwrap()["busy_policy"],
        "follow_up"
    );
    let task = fx.ctx.store.get(target.task.as_deref().unwrap()).unwrap();
    assert_eq!(
        task.runs[0].pending_reply.as_deref(),
        Some("Continue with these owner words")
    );
    assert!(inbox.terminal(messages[0].seq).unwrap().is_none());
}

#[test]
fn remote_keys_never_type_into_adopted_or_bridged_panes() {
    let (fx, mut target, _) = worker();
    target.bridge = false;
    target.adopted = true;
    assert!(
        keys::keys(&fx.ctx, &target, &["enter".into()], Some("owner draft"))
            .unwrap_err()
            .to_string()
            .starts_with("adopted_pane:")
    );
    target.adopted = false;
    target.bridge = true;
    assert!(
        keys::keys(&fx.ctx, &target, &["ctrl-c".into()], None)
            .unwrap_err()
            .to_string()
            .starts_with("has_bridge:")
    );
    target.bridge = false;
    for text in ["", " \t\n"] {
        let error = keys::keys(&fx.ctx, &target, &[], Some(text))
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("invalid_params:"));
    }
    assert!(
        keys::keys(&fx.ctx, &target, &["arbitrary text".into()], None)
            .unwrap_err()
            .to_string()
            .starts_with("invalid_params:")
    );
}

struct BoundAdapter(SessionBinding);

impl DeliveryAdapter for BoundAdapter {
    fn harness(&self) -> &'static str {
        CorralPiAdapter.harness()
    }
    fn capabilities(&self) -> Capabilities {
        CorralPiAdapter.capabilities()
    }
    fn readiness(&self, _: &Ctx, _: &RecipientRef) -> Result<Readiness> {
        Ok(Readiness::Native)
    }
    fn bind(&self, _: &Ctx, _: &RecipientRef) -> Result<BindOutcome> {
        Ok(BindOutcome::Bound(self.0.clone()))
    }
    fn send(&self, ctx: &Ctx, binding: &SessionBinding, envelope: &Envelope) -> SendOutcome {
        CorralPiAdapter.send(ctx, binding, envelope)
    }
    fn observe(
        &self,
        _: &Ctx,
        _: &SessionBinding,
        _: &mut ObserveCursor,
    ) -> Result<Vec<Observation>> {
        Ok(Vec::new())
    }
}

#[cfg(target_os = "linux")]
#[test]
fn follow_up_before_steer_keeps_order_and_steer_busy_policy_on_bridge() {
    let (fx, target, recipient) = worker();
    let host = ts::bind(
        &fx,
        &crate::agent::BindTarget::Worker {
            task: target.task.clone().unwrap(),
            run: "R1".into(),
        },
        "p1",
        "session-send-policy",
    );
    ts::committed(&ts::hello(&fx, &host));
    let dir = fx._tmp.path().join("g1");
    std::fs::create_dir(&dir).unwrap();
    let cap = dir.join("ctl.cap");
    std::fs::write(&cap, "a".repeat(64)).unwrap();
    std::fs::set_permissions(&cap, std::fs::Permissions::from_mode(0o600)).unwrap();
    let socket = dir.join("host.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let bridge = std::thread::spawn(move || {
        let mut inputs = Vec::new();
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            let frame: Value = serde_json::from_str(&line).unwrap();
            inputs.push(frame["args"].clone());
            writeln!(
                stream,
                "{}",
                json!({
                    "id": frame["id"], "generation": 1, "ok": true,
                    "result": {"request_id": frame["id"],
                        "idempotency_key": frame["args"]["idempotency_key"],
                        "disposition": "queued", "replayed": false},
                })
            )
            .unwrap();
        }
        inputs
    });
    let adapter = BoundAdapter(SessionBinding {
        session: host.identity.session_id,
        endpoint: Some(socket),
        transcript: None,
    });
    let inbox = Inbox::new(&fx.ctx.store, recipient.clone()).unwrap();
    let first = owner::post_with_policy(
        &fx.ctx,
        &recipient,
        "owner_reply",
        "First, follow up",
        MessageBinding::default(),
        Some("follow_up"),
        |_, _| Ok(()),
    )
    .unwrap();
    let second = owner::post_with_policy(
        &fx.ctx,
        &recipient,
        "owner_reply",
        "Then steer",
        MessageBinding::default(),
        Some("steer"),
        |_, _| Ok(()),
    )
    .unwrap();
    assert_eq!(
        crate::delivery::deliver(&fx.ctx, &recipient, &adapter, None).unwrap(),
        DeliveryStatus::Sent { envelope: 1 }
    );
    let envelope = inbox.envelope(1).unwrap();
    assert_eq!(envelope.members, vec![first.seq]);
    assert_eq!(envelope.mode, "follow_up");
    inbox
        .record_receipt(
            first.seq,
            ReceiptLevel::Observed,
            &Receipt {
                envelope: Some(1),
                binding: Some(adapter.0.clone()),
                observed_at: fx.ctx.store.now(),
                via: "test".into(),
                detail: json!({}),
            },
        )
        .unwrap();
    assert!(inbox.close_settled(&envelope).unwrap());
    assert_eq!(
        crate::delivery::deliver(&fx.ctx, &recipient, &adapter, None).unwrap(),
        DeliveryStatus::Sent { envelope: 2 }
    );
    let envelope = inbox.envelope(2).unwrap();
    assert_eq!(envelope.members, vec![second.seq]);
    assert_eq!(envelope.mode, "steer");
    let inputs = bridge.join().unwrap();
    assert_eq!(inputs[0]["busy_policy"], "follow_up");
    assert_eq!(inputs[1]["busy_policy"], "steer");
    assert!(
        inputs[0]["text"]
            .as_str()
            .unwrap()
            .contains("First, follow up")
    );
    assert!(!inputs[0]["text"].as_str().unwrap().contains("Then steer"));
    assert!(inputs[1]["text"].as_str().unwrap().contains("Then steer"));
}
