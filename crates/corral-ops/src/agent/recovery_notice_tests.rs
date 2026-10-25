//! T-252 recovery: the notice and the inbox around a resume. A notice too large for the Pi
//! adapter's 8 KiB input keeps every class in an evidence file and strands nothing queued; an
//! envelope the lost host never confirmed is settled without a resend so the notice goes first;
//! a Pi question's answer queued while the host was lost reaches generation 2 after the notice,
//! through the production adapter, for a worker and for a reviewer.
use corral_core::Status;
use corral_core::inbox::{
    Inbox, Post, Receipt, ReceiptLevel, SessionBinding, TerminalState, Urgency,
};
use serde_json::{Value, json};

use super::tests::{ask, messages, receipt, resume, run_recipient, state, sync};
use super::*;
use crate::agent::bridge::fixture::{self as f, Fx, HostProc, SESSION};
use crate::agent::delivery::{INPUT_MAX_BYTES, PiNativeAdapter, wire_key};
use crate::agent::test_support as ts;

fn binding(fx: &Fx, to: &Recipient, host: &ts::Host) -> SessionBinding {
    let n = host.identity.generation;
    SessionBinding {
        session: SESSION.into(),
        endpoint: Some(
            generation_dir(&fx.ctx.store, to, n)
                .unwrap()
                .join(SOCKET_FILE),
        ),
        transcript: Some(f::session_file(host)),
    }
}

fn post(inbox: &Inbox<'_>, kind: &str, body: &str) -> Message {
    inbox
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
}

fn sha256(text: &str) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(text.as_bytes()))
}

/// Appends the `corral_delivery` entry the host's delivery module writes before Pi sees `text`.
fn host_entry(host: &ts::Host, key: &str, text: &str) {
    let entry = json!({
        "type": "custom", "customType": "corral_delivery", "id": format!("d-{key}"),
        "timestamp": jiff::Timestamp::now(), "data": {
            "version": 1, "idempotency_key": key, "request_id": "rq-delivery",
            "origin": "corral-inbox", "busy_policy": "follow_up", "text_sha256": sha256(text),
            "text_bytes": text.len(), "generation": host.identity.generation, "session_id": SESSION,
        },
    });
    let path = f::session_file(host);
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    text.push_str(&format!("{entry}\n"));
    std::fs::write(path, text).unwrap();
}

fn observed(inbox: &Inbox<'_>, seq: u64) -> Option<Receipt> {
    inbox.receipt(seq, ReceiptLevel::Observed).unwrap()
}

#[test]
fn a_notice_too_large_for_the_pi_input_keeps_every_class_in_evidence_and_strands_nothing() {
    // Given a lost host whose last message left 256 corral calls, each with a long stored result:
    let fx = f::fixture(false, 5);
    let mut host = HostProc::spawn(true);
    let bound = fx.bind(&fx.worker_target(), "w0:p1", &host);
    let ids: Vec<String> = (0..256).map(|i| format!("c-{i:03}")).collect();
    let calls: Vec<(&str, &str)> = ids
        .iter()
        .map(|id| (id.as_str(), "corral_progress"))
        .collect();
    f::write_session(&bound, &f::session(&calls, &[]));
    let note = "x".repeat(600);
    let stored: Vec<RequestReceipt> = ids
        .iter()
        .map(|id| receipt(&bound, id, "progress", "committed", json!({"note": note})))
        .collect();
    let update = |t: &mut Task| {
        let state = agent::container_state_mut(t, &fx.worker()).unwrap();
        state.receipts.extend(stored.clone());
        Ok(())
    };
    fx.ctx.store.update(&fx.id, update).unwrap();
    let inbox = Inbox::new(&fx.ctx.store, run_recipient(&fx)).unwrap();
    let queued = post(&inbox, "owner_reply", "Queued while the host was lost.");
    host.kill();

    // When the owner resumes the run:
    resume(&fx, &fx.worker(), false).unwrap();

    // Then the notice names the evidence file that keeps every class, and its subject has all:
    let notice = messages(&fx, &run_recipient(&fx), NOTICE_KIND).remove(0);
    let marker = "more not shown here: read the complete notice at ";
    let at = notice
        .body
        .find(marker)
        .expect("a bounded notice names its evidence");
    let path = notice.body[at + marker.len()..].lines().next().unwrap();
    let complete = std::fs::read_to_string(path).unwrap();
    for id in &ids {
        assert!(complete.contains(&format!("(call {id}): result")), "{id}");
    }
    let title = format!(
        "Complete recovery notice ({} generation 1)",
        fx.worker().key()
    );
    assert!(fx.task().artifacts.iter().any(|a| a.title == title));
    let subject = notice.subject.clone().unwrap();
    assert_eq!(subject["calls"].as_array().unwrap().len(), 256);

    // And generation 2's first envelope carries it whole within the input cap, and the queued
    // reply follows once it is observed:
    let next = ts::Host {
        identity: corral_core::agent::RequestIdentity {
            generation: 2,
            ..bound.identity.clone()
        },
        ..bound.clone()
    };
    let wire = binding(&fx, &run_recipient(&fx), &next);
    let first = inbox
        .publish(&wire, "pi_native", INPUT_MAX_BYTES, true)
        .unwrap()
        .unwrap();
    let payload = inbox.payload(&first).unwrap();
    assert!(payload.len() <= INPUT_MAX_BYTES, "{}", payload.len());
    assert_eq!(first.members[0], notice.seq);
    assert!(payload.contains(&notice.body));
    if !first.members.contains(&queued.seq) {
        let seen = Receipt {
            envelope: Some(first.number),
            binding: Some(wire.clone()),
            observed_at: jiff::Timestamp::now(),
            via: "test".into(),
            detail: Value::Null,
        };
        for seq in &first.members {
            inbox
                .record_receipt(*seq, ReceiptLevel::Observed, &seen)
                .unwrap();
        }
        assert!(inbox.close_settled(&first).unwrap());
        let second = inbox
            .publish(&wire, "pi_native", INPUT_MAX_BYTES, true)
            .unwrap()
            .unwrap();
        assert_eq!(second.members, [queued.seq]);
    }
}

#[test]
fn an_envelope_the_lost_host_never_confirmed_is_settled_without_a_resend_and_the_notice_goes_first()
{
    for confirmed in [false, true] {
        // Given generation 1 was sent an envelope it never confirmed before its host died:
        let fx = f::fixture(false, 5);
        let mut host = HostProc::spawn(true);
        let bound = fx.bind(&fx.worker_target(), "w0:p1", &host);
        f::write_session(&bound, &f::session(&[], &[]));
        let inbox = Inbox::new(&fx.ctx.store, run_recipient(&fx)).unwrap();
        let earlier = post(&inbox, "owner_reply", "Sent before the loss.");
        let wire = binding(&fx, &run_recipient(&fx), &bound);
        let mut sent = inbox
            .publish(&wire, "pi_native", INPUT_MAX_BYTES, true)
            .unwrap()
            .unwrap();
        inbox
            .begin_attempt(&mut sent, jiff::Timestamp::now())
            .unwrap();
        if confirmed {
            let text = inbox.payload(&sent).unwrap();
            host_entry(&bound, &wire_key(&sent.idempotency_key), &text);
        }
        host.kill();

        // When the owner resumes the run:
        let outcome = resume(&fx, &fx.worker(), false).unwrap();

        // Then the old envelope is settled from the session's evidence and never resent:
        let case = format!("confirmed={confirmed}");
        let unconfirmed = &outcome["unconfirmed"][0];
        assert_eq!(unconfirmed["id"], json!(earlier.id), "{case}");
        assert_eq!(unconfirmed["observed"], json!(confirmed), "{case}");
        assert!(inbox.state().unwrap().open_envelope.is_none(), "{case}");
        let notice = messages(&fx, &run_recipient(&fx), NOTICE_KIND).remove(0);
        let reported = format!("message {} (owner_reply): delivery unknown", earlier.id);
        if confirmed {
            let receipt = observed(&inbox, earlier.seq).unwrap();
            assert_eq!(receipt.via, "recovery");
            assert!(!notice.body.contains(&reported), "{case}");
        } else {
            assert!(observed(&inbox, earlier.seq).is_none(), "{case}");
            assert_eq!(
                inbox.terminal(earlier.seq).unwrap(),
                Some(TerminalState::Failed)
            );
            assert!(notice.body.contains(&reported), "{case}: {}", notice.body);
        }
        assert_eq!(fx.events("delivery_fenced").len(), 1, "{case}");

        // And generation 2's first envelope is the notice alone:
        let next = ts::Host {
            identity: corral_core::agent::RequestIdentity {
                generation: 2,
                ..bound.identity.clone()
            },
            ..bound.clone()
        };
        let g2 = binding(&fx, &run_recipient(&fx), &next);
        let first = inbox
            .publish(&g2, "pi_native", INPUT_MAX_BYTES, true)
            .unwrap()
            .unwrap();
        assert_eq!(first.members, [notice.seq], "{case}");
        assert!(
            !inbox
                .payload(&first)
                .unwrap()
                .contains("Sent before the loss.")
        );
    }
}

/// A host lost while its question waits, then answered by the PM; returns the generation-1
/// host and the answer's inbox sequence.
fn answered_while_lost(fx: &Fx, target: &BindTarget, pane: &str) -> (ts::Host, u64) {
    let container = target.container();
    let mut host = HostProc::spawn(true);
    let bound = fx.bind(target, pane, &host);
    f::write_session(&bound, &f::session(&[], &[]));
    f::bridge(
        &fx.ctx.store,
        &recipient(&container),
        1,
        Some(f::CAP),
        f::healthy,
    );
    let key = ask(fx, &container);
    host.kill();
    sync(fx);
    assert_eq!(state(fx, &container).binding.health, HostHealth::Lost);
    let revision = crate::progress_spec_revision(&fx.task()).unwrap();
    let answer = crate::questions::AnswerInput {
        text: "Use the first input.".into(),
        option: None,
        basis_refs: vec![format!("spec:{}@{revision}", fx.id)],
        scope_note: Some("inside the run's write scope".into()),
    };
    let pm = crate::questions::Actor::Pm(None);
    crate::questions::answer(&fx.ctx, &key, 1, pm, answer).unwrap();
    let id = state(fx, &container).questions[0]
        .clone()
        .answer
        .unwrap()
        .message_id
        .unwrap();
    (bound, id.rsplit_once('#').unwrap().1.parse().unwrap())
}

#[test]
fn an_answer_queued_while_the_host_was_lost_is_observed_in_generation_2_after_the_notice() {
    for reviewer in [false, true] {
        // Given a lost host whose open question the PM answered meanwhile:
        let fx = f::fixture(false, 5);
        let (target, pane) = match reviewer {
            false => (fx.worker_target(), "w0:p1"),
            true => {
                let target = f::reviewing(&fx);
                fx.pending_review("w0:p2", jiff::Timestamp::now());
                (target, "w0:p2")
            }
        };
        let container = target.container();
        let to = recipient(&container);
        let (bound, answer) = answered_while_lost(&fx, &target, pane);
        let inbox = Inbox::new(&fx.ctx.store, to.clone()).unwrap();
        assert!(observed(&inbox, answer).is_none() && inbox.envelopes().unwrap().is_empty());

        // When the owner resumes it and generation 2 binds on the same Pi session:
        resume(&fx, &container, false).unwrap();
        let host = HostProc::spawn(true);
        let next = fx.hello_next(&bound, &to, &host);
        let bridge = f::bridge(&fx.ctx.store, &to, 2, Some(f::CAP), f::quiet);

        // Then production delivery sends the notice first and the answer after it:
        let status = crate::delivery::deliver(&fx.ctx, &to, &PiNativeAdapter, None).unwrap();
        assert!(
            matches!(status, crate::delivery::DeliveryStatus::Sent { .. }),
            "{status:?}"
        );
        let sent: Vec<Value> = bridge.commands();
        let submit = sent
            .iter()
            .find(|f| f["command"] == "submit_input")
            .unwrap();
        let text = submit["args"]["text"].as_str().unwrap().to_owned();
        let notice = text.find(" recovery_notice]").unwrap();
        assert!(
            notice < text.find("Use the first input.").unwrap(),
            "{text}"
        );

        // And once generation 2's delivery module records the envelope, both are observed:
        let key = submit["args"]["idempotency_key"]
            .as_str()
            .unwrap()
            .to_owned();
        host_entry(&next, &key, &text);
        crate::delivery::deliver(&fx.ctx, &to, &PiNativeAdapter, None).unwrap();
        let receipt = observed(&inbox, answer).expect("the answer is observed");
        assert_eq!(
            receipt.binding.unwrap().endpoint,
            binding(&fx, &to, &next).endpoint
        );
        let notice = messages(&fx, &to, NOTICE_KIND).remove(0);
        assert!(observed(&inbox, notice.seq).is_some());
        let expected = if reviewer {
            Status::Review
        } else {
            Status::Running
        };
        assert_eq!(fx.task().status, expected, "reviewer={reviewer}");
    }
}
