use super::*;
use crate::agent::{call, tool_request_id};
use crate::session_control::pause::{self, PauseRecord, RECORDS};

fn request(
    fx: &Fx,
    host: &ts::Host,
    op: &str,
    label: &str,
    payload: Value,
) -> crate::agent::call::CallResponse {
    let id = tool_request_id(&host.identity.session_id, label);
    call(&fx.ctx, op, &ts::envelope(host, op, &id, payload, false)).unwrap()
}

#[test]
fn bound_peers_admit_information_in_the_posting_call_without_owner_effects() {
    let fx = ts::fx();
    let (_, sender, a) = corral_pi_worker(&fx);
    let (b_id, recipient, b) = corral_pi_worker(&fx);
    let (_, offline, c) = corral_pi_worker(&fx);
    let _runtime = runtime(&fx, &[&sender, &recipient, &offline]);
    let a = bound(&fx, &a, "p1", "peer-a");
    let b = bound(&fx, &b, "p1", "peer-b");
    let _unbound = ts::bind(&fx, &c, "p1", "peer-offline");
    let fake = FakeHost::start(&fx, &recipient, &b);
    fake.script().hold_entries = true;
    fx.ctx
        .store
        .update(&b_id, |task| {
            task.status = corral_core::Status::NeedsMe;
            task.rounds = 2;
            task.runs[0].blocked_excerpt = Some("Waiting for the owner".into());
            Ok(())
        })
        .unwrap();
    let key = recipient.key().unwrap();
    let roster = ts::committed(&request(&fx, &a, "peer_roster", "roster", json!({}))).result;
    assert_eq!(roster["self"], sender.key().unwrap());
    let peers = roster["peers"].as_array().unwrap();
    assert!(!peers.iter().any(|p| p["key"] == sender.key().unwrap()));
    assert!(
        peers
            .iter()
            .any(|p| p["key"] == key && p["state"] == "live")
    );
    assert!(
        peers
            .iter()
            .any(|p| p["key"] == offline.key().unwrap() && p["state"] == "offline")
    );
    let input = json!({"to":key, "text":"The parser accepts this input."});
    let sent = ts::committed(&request(&fx, &a, "peer_send", "send", input.clone()));
    assert_eq!(sent.result["state"], "delivered");
    assert_eq!(fake.submits().len(), 1);
    assert_eq!(fake.submits()[0]["busy_policy"], "steer");
    let inbox = Inbox::new(&fx.ctx.store, recipient.clone()).unwrap();
    let message = inbox.message(1).unwrap();
    assert_eq!(message.kind, "agent_message");
    assert_eq!(message.author, sender.key().unwrap());
    assert!(!message.owner_kind());
    let replay = request(&fx, &a, "peer_send", "send", input);
    assert!(matches!(
        replay,
        crate::agent::call::CallResponse::Receipt { replayed: true, .. }
    ));
    assert_eq!(fake.submits().len(), 1);
    assert!(!observed(&fx, &recipient, 1));
    owner::done_gate(&fx.ctx, &task(&fx, &b_id)).unwrap();
    fake.flush();
    owner::kick(&fx.ctx, &recipient).unwrap();
    let current = task(&fx, &b_id);
    assert_eq!(current.status, corral_core::Status::NeedsMe);
    assert_eq!(current.rounds, 2);
    assert_eq!(
        current.runs[0].blocked_excerpt.as_deref(),
        Some("Waiting for the owner")
    );
    assert!(current.runs[0].pending_reply.is_none());

    let pause_key =
        crate::delivery::peer::recipient(&crate::agent::admission::container_of(&b.identity));
    let container_key = crate::agent::admission::container_of(&b.identity).key();
    RECORDS
        .save(
            &fx.ctx.store,
            &container_key,
            &PauseRecord {
                id: container_key.clone(),
                generation: Some(1),
                at: fx.ctx.store.now(),
                actor: Some(corral_core::Actor::Owner),
                task: Some(b_id.clone()),
                hold: None,
            },
        )
        .unwrap();
    let queued = ts::committed(&request(
        &fx,
        &a,
        "peer_send",
        "paused",
        json!({"to":key,"text":"More context"}),
    ));
    assert_eq!(queued.result["state"], "queued");
    assert!(pause::recipient_paused(&fx.ctx, &pause_key).unwrap());
    assert_eq!(fake.submits().len(), 1);
    let next = inbox.state_head().unwrap().next_seq;
    let oversized = request(
        &fx,
        &a,
        "peer_send",
        "oversize",
        json!({"to":key,"text":"x".repeat(INPUT_MAX_BYTES)}),
    );
    assert_eq!(ts::code(&oversized), Some("bad_request"));
    assert_eq!(inbox.state_head().unwrap().next_seq, next);
    fx.ctx
        .store
        .update(&b_id, |task| {
            task.status = corral_core::Status::Done;
            task.runs[0].outcome = Some(RunOutcome::Done);
            Ok(())
        })
        .unwrap();
    crate::questions::terminalize(&fx.ctx, &b_id).unwrap();
    assert_eq!(inbox.terminal(2).unwrap(), Some(TerminalState::Cancelled));
    let closed = ts::committed(&request(
        &fx,
        &a,
        "peer_send",
        "closed",
        json!({"to":key,"text":"Late context"}),
    ));
    assert_eq!(closed.result["state"], "undeliverable");
    let self_send = ts::committed(&request(
        &fx,
        &a,
        "peer_send",
        "self",
        json!({"to":sender.key().unwrap(),"text":"Self"}),
    ));
    assert_eq!(self_send.result["state"], "undeliverable");
}

#[test]
fn the_sixty_first_peer_operation_is_refused_without_reserving_a_sequence() {
    let fx = ts::fx();
    let (_, sender, a) = corral_pi_worker(&fx);
    let (_, recipient, b) = corral_pi_worker(&fx);
    let a = bound(&fx, &a, "p1", "budget-sender");
    let _offline = ts::bind(&fx, &b, "p1", "budget-recipient");
    for n in 0..corral_core::inbox::AGENT_MESSAGES_PER_GENERATION {
        crate::delivery::peer::post(
            &fx.ctx,
            crate::delivery::peer::PostRequest {
                recipient: &recipient,
                sender: &sender,
                role: corral_core::agent::AgentRole::Worker,
                title: Some("Pi worker"),
                generation: 1,
                request_id: &format!("budget-{n}"),
                payload_sha256: "hash",
                text: "Context",
            },
        )
        .unwrap();
    }
    let inbox = Inbox::new(&fx.ctx.store, recipient.clone()).unwrap();
    let next = inbox.state_head().unwrap().next_seq;
    let response = request(
        &fx,
        &a,
        "peer_send",
        "over-budget",
        json!({"to":recipient.key().unwrap(), "text":"One too many"}),
    );
    assert_eq!(ts::code(&response), Some("agent_message_limit"));
    assert_eq!(inbox.state_head().unwrap().next_seq, next);
}
