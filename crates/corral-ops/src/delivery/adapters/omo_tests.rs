use corral_core::Status;
use corral_core::inbox::{Inbox, MessageBinding, TerminalState};

use super::*;
use crate::delivery::omo_fixture::{OmoWorld, STARTED};
use crate::delivery::{DeliveryStatus, owner};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/src/delivery/adapters/omo_fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn outcome(name: &str) -> SendOutcome {
    let code = name
        .rsplit_once(".exit")
        .and_then(|(_, rest)| rest.strip_suffix(".json"))
        .unwrap()
        .parse()
        .unwrap();
    classify(Some(code), &fixture(name))
}

#[test]
fn captured_send_results_map_to_the_outcome_contract() {
    for name in [
        "send-idle.exit0.json",
        "send-dedup.exit0.json",
        "send-busy.exit0.json",
        "send-draft.exit0.json",
        "send-long.exit0.json",
    ] {
        assert!(
            matches!(outcome(name), SendOutcome::Admitted { .. }),
            "{name}"
        );
    }
    let SendOutcome::Admitted { detail } = outcome("send-dedup.exit0.json") else {
        unreachable!()
    };
    assert_eq!(detail["deduplicated"], true);
    assert_eq!(
        detail["delivery_id"],
        "1ff2b7f3-d624-460e-9579-795d4eba724e"
    );
    assert!(matches!(
        outcome("send-offline.exit0.json"),
        SendOutcome::Held { .. }
    ));
    for name in [
        "send-scope-denied.exit1.json",
        "send-unknown-target.exit1.json",
        "send-steer-noturn.exit1.json",
        "send-unsupported.exit4.json",
    ] {
        assert!(
            matches!(outcome(name), SendOutcome::Rejected { .. }),
            "{name}"
        );
    }
    assert!(matches!(
        outcome("send-key-conflict.exit1.json"),
        SendOutcome::Conflicted { .. }
    ));
    for name in [
        "send-host-unavailable.exit3.json",
        "send-internal-error.exit5.json",
    ] {
        assert!(
            matches!(outcome(name), SendOutcome::Uncertain { .. }),
            "{name}"
        );
    }
    assert!(matches!(
        classify(Some(0), "not json"),
        SendOutcome::Uncertain { .. }
    ));
    assert!(matches!(classify(None, ""), SendOutcome::Uncertain { .. }));
}

#[test]
fn binding_requires_one_live_tui_session_inside_the_run_window() {
    let world = OmoWorld::new(Status::Running);
    let target = Target {
        cwd: world.worktree.clone(),
        pane: Some("w:p".into()),
        started: STARTED.parse().unwrap(),
        ended: None,
    };
    let now = "2026-10-19T01:00:00Z".parse().unwrap();
    let live = world.row("S1", "2026-10-19T00:00:01Z");
    assert_eq!(candidates(&json!([live]), &target, now).unwrap().len(), 1);
    // A session that predates the run (a stale binding) is never a candidate.
    let stale = world.row("S0", "2026-10-18T23:59:59Z");
    assert!(
        candidates(&json!([stale]), &target, now)
            .unwrap()
            .is_empty()
    );
    let mut closed = world.row("S2", "2026-10-19T00:00:02Z");
    closed["status"] = json!("resumable");
    let mut readonly = world.row("S3", "2026-10-19T00:00:02Z");
    readonly["controls"] = json!(["read"]);
    let mut elsewhere = world.row("S4", "2026-10-19T00:00:02Z");
    elsewhere["cwd"] = json!("/elsewhere");
    assert!(
        candidates(&json!([closed, readonly, elsewhere]), &target, now)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn delivered_entry_shape_matches_the_captured_gateway_transcript() {
    let entry: Value =
        serde_json::from_str(fixture("delivery-entries.jsonl").lines().next().unwrap()).unwrap();
    assert_eq!(
        delivered_texts(&entry),
        vec!["[corral capture] idle probe. Reply with exactly: OK".to_string(); 2]
    );
    assert_eq!(entry["details"]["source"], "session_control");
}

fn post(world: &OmoWorld, body: &str) -> u64 {
    owner::post(
        &world.ctx,
        &world.recipient(),
        "owner_reply",
        body,
        MessageBinding::default(),
        |_, _| Ok(()),
    )
    .unwrap()
    .seq
}

#[test]
fn idle_delivery_sends_once_with_auto_mode_and_is_observed() {
    let world = OmoWorld::new(Status::NeedsMe);
    let long = fixture("send-long.exit0.json");
    let body = format!(
        "{long}\n{}",
        "line with \"quotes\" and unicode \u{2014}\n".repeat(80)
    );
    let seq = post(&world, &body);
    let status = owner::kick_with(&world.ctx, &world.recipient(), &world.adapter()).unwrap();
    assert_eq!(status, DeliveryStatus::Idle);
    let sends = world.sends();
    assert_eq!(sends.len(), 1);
    let args = &sends[0];
    let flag = |name: &str| args[args.iter().position(|a| a == name).unwrap() + 1].clone();
    assert_eq!(flag("--mode"), "auto");
    assert_eq!(
        flag("--idempotency-key"),
        format!("{}@env1", world.recipient().key().unwrap())
    );
    assert!(args.contains(&"--all-scope".to_string()));
    assert!(args.last().unwrap().ends_with(&body));
    assert_eq!(world.entries(), 1);
    let entry = Inbox::new(&world.ctx.store, world.recipient())
        .unwrap()
        .entry(seq)
        .unwrap();
    assert_eq!(entry.observed.unwrap().via, "omo");
    assert!(world.terminal_writes().is_empty());
    let task = world.ctx.store.get(&world.task.id).unwrap();
    assert_eq!(task.status, Status::Running);
}

#[test]
fn uncertain_results_resend_the_identical_key_and_deliver_exactly_once() {
    for failure in [
        "5 send-internal-error.exit5.json 0",
        "3 send-host-unavailable.exit3.json 0",
        "0 - 0",
    ] {
        let world = OmoWorld::new(Status::Running);
        world.script(&[failure, "0 send-dedup.exit0.json 1"]);
        let seq = post(&world, "please rebase");
        owner::kick_with(&world.ctx, &world.recipient(), &world.adapter()).unwrap();
        let sends = world.sends();
        assert_eq!(sends.len(), 2, "{failure}");
        assert_eq!(sends[0], sends[1], "{failure}");
        assert_eq!(world.entries(), 1);
        assert!(owner::is_observed(&world.ctx, &world.recipient(), seq).unwrap());
        assert_eq!(world.events("delivery_uncertain").len(), 1);
    }
}

#[test]
fn rejected_send_fails_the_message_and_clears_the_pending_view() {
    let world = OmoWorld::new(Status::Running);
    world.script(&["1 send-scope-denied.exit1.json 0"]);
    let recipient = world.recipient();
    let seq = owner::post(
        &world.ctx,
        &recipient,
        "owner_reply",
        "stop",
        MessageBinding::default(),
        |task, _| {
            task.runs[0].pending_reply = Some("stop".into());
            Ok(())
        },
    )
    .unwrap()
    .seq;
    owner::kick_with(&world.ctx, &recipient, &world.adapter()).unwrap();
    let inbox = Inbox::new(&world.ctx.store, recipient).unwrap();
    assert_eq!(inbox.terminal(seq).unwrap(), Some(TerminalState::Failed));
    assert_eq!(world.sends().len(), 1);
    let task = world.ctx.store.get(&world.task.id).unwrap();
    assert_eq!(task.runs[0].pending_reply, None);
}

#[test]
fn held_offline_delivery_waits_without_resending() {
    let world = OmoWorld::new(Status::Running);
    world.script(&["0 send-offline.exit0.json 0"]);
    let seq = post(&world, "when you are back");
    for _ in 0..2 {
        owner::kick_with(&world.ctx, &world.recipient(), &world.adapter()).unwrap();
    }
    assert_eq!(world.sends().len(), 1);
    assert!(!owner::is_observed(&world.ctx, &world.recipient(), seq).unwrap());
}

#[test]
fn stale_or_ambiguous_bindings_block_without_sending() {
    let world = OmoWorld::new(Status::Running);
    world.set_list(&[world.row("S0", "2026-10-18T23:00:00Z")]);
    post(&world, "anyone there?");
    let status = owner::kick_with(&world.ctx, &world.recipient(), &world.adapter()).unwrap();
    assert!(
        matches!(&status, DeliveryStatus::Blocked { reason } if reason.starts_with("session_unbound")),
        "{status:?}"
    );
    world.set_list(&[
        world.row("S1", "2026-10-19T00:00:01Z"),
        world.row("S2", "2026-10-19T00:00:02Z"),
    ]);
    let status = owner::kick_with(&world.ctx, &world.recipient(), &world.adapter()).unwrap();
    assert!(
        matches!(&status, DeliveryStatus::Blocked { reason } if reason.starts_with("session_ambiguous")),
        "{status:?}"
    );
    assert!(world.sends().is_empty());
}

#[test]
fn question_bound_messages_are_never_packed() {
    let world = OmoWorld::new(Status::NeedsMe);
    let recipient = world.recipient();
    let bound = owner::post(
        &world.ctx,
        &recipient,
        "owner_reply",
        "answer to the open question",
        MessageBinding {
            question_request: Some("call-1".into()),
            ..Default::default()
        },
        |_, _| Ok(()),
    )
    .unwrap();
    assert_eq!(
        owner::kick_with(&world.ctx, &recipient, &world.adapter()).unwrap(),
        DeliveryStatus::Idle
    );
    assert!(world.sends().is_empty());
    let ordinary = post(&world, "an ordinary note");
    owner::kick_with(&world.ctx, &recipient, &world.adapter()).unwrap();
    let inbox = Inbox::new(&world.ctx.store, recipient).unwrap();
    assert_eq!(inbox.envelopes().unwrap()[0].members, vec![ordinary]);
    assert!(!world.sends()[0].last().unwrap().contains("open question"));
    assert!(inbox.entry(bound.seq).unwrap().observed.is_none());
}

#[test]
fn a_foreign_delivery_entry_is_never_counted() {
    let world = OmoWorld::new(Status::Running);
    world.script(&["0 send-busy.exit0.json 0"]);
    let seq = post(&world, "do the thing");
    owner::kick_with(&world.ctx, &world.recipient(), &world.adapter()).unwrap();
    // Same envelope header, different payload bytes (as if truncated): no receipt.
    let entry = json!({"type":"custom_message","customType":"session_control_delivery",
        "content":"x\ny\n\"z\"","details":{"delivery_id":"3e184ffa-e124-4e8f-a207-c031105f758a",
        "source":"session_control","display_text":format!("[corral envelope {}@env1]\n\ntruncated",
        world.recipient().key().unwrap())}});
    let mut transcript = std::fs::read_to_string(&world.transcript).unwrap();
    transcript.push_str(&format!("{entry}\n"));
    std::fs::write(&world.transcript, transcript).unwrap();
    owner::kick_with(&world.ctx, &world.recipient(), &world.adapter()).unwrap();
    assert!(!owner::is_observed(&world.ctx, &world.recipient(), seq).unwrap());
    assert_eq!(world.sends().len(), 1);
}

#[test]
fn a_conflicting_key_stops_for_an_owner_decision_instead_of_retrying() {
    let world = OmoWorld::new(Status::Running);
    world.script(&["1 send-key-conflict.exit1.json 0"]);
    let seq = post(&world, "cannot be settled by retry");
    for _ in 0..2 {
        let status = owner::kick_with(&world.ctx, &world.recipient(), &world.adapter()).unwrap();
        assert_eq!(status, DeliveryStatus::Uncertain);
    }
    assert_eq!(world.sends().len(), 1);
    let alerts = world.events("delivery_uncertain");
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0]["owner_decision"], true);
    // The owner's recover decision resends under a new key, which settles the message.
    Inbox::new(&world.ctx.store, world.recipient())
        .unwrap()
        .recover(corral_core::inbox::Recovery::Resend)
        .unwrap();
    owner::kick_with(&world.ctx, &world.recipient(), &world.adapter()).unwrap();
    let sends = world.sends();
    assert_eq!(sends.len(), 2);
    let key = |args: &Vec<String>| {
        args[args.iter().position(|a| a == "--idempotency-key").unwrap() + 1].clone()
    };
    assert_ne!(key(&sends[0]), key(&sends[1]));
    assert!(owner::is_observed(&world.ctx, &world.recipient(), seq).unwrap());
}

#[test]
fn an_unavailable_gateway_host_blocks_binding_without_sending() {
    let world = OmoWorld::new(Status::Running);
    world.list_fails();
    post(&world, "waits for the host");
    let error = owner::kick_with(&world.ctx, &world.recipient(), &world.adapter()).unwrap_err();
    assert!(error.contains("exit Some(3)"), "{error}");
    assert!(world.sends().is_empty());
    assert_eq!(world.events("delivery_blocked").len(), 1);
}
