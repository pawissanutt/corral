use corral_core::agent::{RESULT_MAX_BYTES, RequestReceipt, pm_state};
use corral_core::{RunOutcome, Status};
use serde_json::{Value, json};

use super::test_support::*;
use super::*;

fn worker(fx: &Fx, id: &str) -> Host {
    let target = BindTarget::Worker {
        task: id.into(),
        run: "R1".into(),
    };
    let host = bind(fx, &target, "p1", &format!("s-{id}"));
    committed(&hello(fx, &host));
    host
}

fn pm(fx: &Fx) -> Host {
    let target = BindTarget::Pm {
        project: "corral".into(),
    };
    let host = bind(fx, &target, "pm-pane", "s-pm");
    committed(&hello(fx, &host));
    host
}

fn lookup_with(fx: &Fx, host: &Host, own: &str, payload: Value) -> CallResponse {
    send(
        fx,
        &envelope(host, "receipt_lookup", own, payload, false),
        "receipt_lookup",
    )
}

fn stored(template: &RequestReceipt, request_id: &str, result: Value) -> RequestReceipt {
    RequestReceipt {
        request_id: request_id.into(),
        op: "artifact".into(),
        result,
        ..template.clone()
    }
}

#[test]
fn a_lookup_reusing_a_committed_request_id_is_rejected_as_a_payload_mismatch() {
    let fx = fx();
    let id = worker_task(&fx, "p1");
    let host = worker(&fx, &id);
    committed(&send(
        &fx,
        &event_input(&host, 1, "turn_started", 1),
        "event",
    ));
    let before = task_bytes(&fx, &id);
    for reused in ["ev-1-0", "ev-1-1"] {
        let response = lookup_with(&fx, &host, reused, json!({"request_id": "ev-1-999"}));
        assert_eq!(code(&response), Some("payload_mismatch"), "{reused}");
    }
    assert_eq!(task_bytes(&fx, &id), before);

    // A PM's receipts on a target task belong to its container too.
    let pm = pm(&fx);
    let target = worker_task(&fx, "p5");
    let taken = tool_request_id("s-pm", "comment-1");
    let template = pm_state::load(&fx.ctx.store)
        .unwrap()
        .unwrap()
        .state
        .receipts[0]
        .clone();
    fx.ctx
        .store
        .update(&target, |t| {
            t.agent_pm_receipts
                .push(stored(&template, &taken, json!({"comment": 1})));
            Ok(())
        })
        .unwrap();
    let collision = lookup_with(
        &fx,
        &pm,
        &taken,
        json!({"request_id": taken, "task": target}),
    );
    assert_eq!(code(&collision), Some("payload_mismatch"));
    let fresh = tool_request_id("s-pm", "lookup-1");
    let found = lookup_with(
        &fx,
        &pm,
        &fresh,
        json!({"request_id": taken, "task": target}),
    );
    assert_eq!(
        found.found().map(|r| r.request_id.as_str()),
        Some(taken.as_str())
    );
}

#[test]
fn only_the_started_open_turn_settles_after_done_or_a_verdict() {
    let fx = fx();
    let id = worker_task(&fx, "p1");
    let host = worker(&fx, &id);
    // A live run never invents a turn it did not see start.
    let unopened = send(&fx, &event_input(&host, 1, "turn_ended", 5), "event");
    assert_eq!(code(&unopened), Some("bad_request"));
    committed(&send(
        &fx,
        &event_input(&host, 2, "turn_started", 1),
        "event",
    ));
    fx.ctx
        .store
        .update(&id, |t| {
            t.status = Status::Review;
            t.runs[0].outcome = Some(RunOutcome::Done);
            Ok(())
        })
        .unwrap();
    let invented = send(&fx, &event_input(&host, 3, "turn_ended", 999), "event");
    assert_eq!(code(&invented), Some("run_ended"));
    committed(&send(&fx, &event_input(&host, 4, "turn_ended", 1), "event"));
    let again = send(&fx, &event_input(&host, 5, "turn_ended", 1), "event");
    assert_eq!(
        code(&again),
        Some("run_ended"),
        "the done turn settles once"
    );
    let saved = state(&fx, &host_container(&host));
    assert!(
        saved.turns.iter().all(|t| t.seq == 1),
        "no turn was invented"
    );

    let (rid, target) = reviewer_task(&fx);
    let reviewer = bind(&fx, &target, "p2", "s-r");
    committed(&hello(&fx, &reviewer));
    committed(&send(
        &fx,
        &event_input(&reviewer, 1, "turn_started", 1),
        "event",
    ));
    fx.ctx
        .store
        .update(&rid, |t| {
            t.runs[0].review_runs[0].review = Some(
                serde_json::from_value(json!({
                    "verdict": "changes", "reviewer": "pi", "sha": "abc",
                    "at": "2026-10-23T00:00:00Z"
                }))
                .unwrap(),
            );
            Ok(())
        })
        .unwrap();
    let invented = send(&fx, &event_input(&reviewer, 2, "turn_ended", 999), "event");
    assert_eq!(code(&invented), Some("review_closed"));
    let container = target.container();
    assert!(
        state(&fx, &container)
            .generation(1)
            .unwrap()
            .fenced
            .is_none()
    );
    let settled = committed(&send(
        &fx,
        &event_input(&reviewer, 3, "turn_ended", 1),
        "event",
    ));
    assert_eq!(settled.result["fenced"], json!(true));
    assert!(
        state(&fx, &container)
            .generation(1)
            .unwrap()
            .fenced
            .is_some()
    );
    let after = send(&fx, &event_input(&reviewer, 4, "turn_ended", 1), "event");
    assert_eq!(
        code(&after),
        Some("review_closed"),
        "the verdict turn settles once"
    );
}

fn host_container(host: &Host) -> corral_core::agent::ContainerRef {
    corral_core::agent::ContainerRef::Worker {
        task: host.identity.task.clone().unwrap(),
        run: host.identity.run.clone().unwrap(),
    }
}

#[test]
fn a_lookup_returns_a_maximum_size_stored_result_unchanged_within_the_bound() {
    let fx = fx();
    let id = worker_task(&fx, "p1");
    let host = worker(&fx, &id);
    let pm = pm(&fx);
    let largest = json!("x".repeat(RESULT_MAX_BYTES - 2));
    assert_eq!(
        serde_json::to_vec(&largest).unwrap().len(),
        RESULT_MAX_BYTES
    );
    let template = state(&fx, &host_container(&host)).receipts[0].clone();
    let big = stored(&template, &tool_request_id("s", "big"), largest.clone());
    fx.ctx
        .store
        .update(&id, |t| {
            let state = t.runs[0].agent_host.as_mut().unwrap();
            state.receipts.push(big.clone());
            t.agent_pm_receipts.push(big.clone());
            Ok(())
        })
        .unwrap();
    pm_state::update(&fx.ctx.store, |area| {
        area.as_mut().unwrap().state.receipts.push(big.clone());
        Ok(((), true))
    })
    .unwrap();
    let cases = [
        (&host, json!({"request_id": big.request_id})),
        (&pm, json!({"request_id": big.request_id})),
        (&pm, json!({"request_id": big.request_id, "task": id})),
    ];
    for (n, (asker, payload)) in cases.into_iter().enumerate() {
        let own = tool_request_id(&asker.identity.session_id, &format!("lookup-{n}"));
        let response = lookup_with(&fx, asker, &own, payload);
        assert_eq!(response.found(), Some(&big));
        let printed = response.to_json().unwrap();
        assert_eq!(printed["result"], largest);
        assert_eq!(
            serde_json::to_vec(&printed["result"]).unwrap().len(),
            RESULT_MAX_BYTES
        );
        assert_eq!(printed["lookup"]["by"], json!(own));
    }
}

#[test]
fn identical_rejected_retries_are_recorded_once() {
    let fx = fx();
    let id = worker_task(&fx, "p1");
    let mut forged = worker(&fx, &id);
    forged.host_cap = secret("forged", 1);
    let before = events(&fx, "agent_request_rejected");
    let wrong = event_input(&forged, 1, "turn_started", 1);
    for _ in 0..3 {
        assert_eq!(code(&send(&fx, &wrong, "event")), Some("auth_failed"));
    }
    assert_eq!(events(&fx, "agent_request_rejected"), before + 1);
    let other = event_input(&forged, 2, "turn_started", 2);
    assert_eq!(code(&send(&fx, &other, "event")), Some("auth_failed"));
    assert_eq!(events(&fx, "agent_request_rejected"), before + 2);
    for malformed in [b"{\"version\":1}".as_slice(), b"{\"version\":1}", b"[]"] {
        assert_eq!(code(&send(&fx, malformed, "event")), Some("bad_request"));
    }
    assert_eq!(events(&fx, "agent_request_rejected"), before + 4);
}

#[test]
fn parse_diagnostics_never_echo_input_bytes() {
    let fx = fx();
    let id = worker_task(&fx, "p1");
    let host = worker(&fx, &id);
    let planted = "synthetic_host_cap_not_a_real_secret_12345";
    let base: Value = serde_json::from_slice(&event_input(&host, 1, "turn_started", 1)).unwrap();
    let with_payload = |payload: Value| {
        let mut envelope = base.clone();
        envelope["payload_sha256"] = json!(payload_sha256(&payload));
        envelope["payload"] = payload;
        envelope
    };
    let mut auth_field = base.clone();
    auth_field["auth"][planted] = json!(1);
    let mut identity_field = base.clone();
    identity_field["identity"][planted] = json!(1);
    let mut role_variant = base.clone();
    role_variant["identity"]["role"] = json!(planted);
    let cases = [
        auth_field,
        identity_field,
        role_variant,
        with_payload(json!({"kind": "turn_started", "turn": 1, planted: 1})),
        with_payload(json!({"kind": planted, "turn": 1})),
        with_payload(json!({"kind": "turn_started", "turn": planted})),
    ];
    for envelope in cases {
        let response = send(&fx, &serde_json::to_vec(&envelope).unwrap(), "event");
        let rejection = response.rejection().unwrap();
        assert_eq!(rejection.code.as_str(), "bad_request");
        assert!(!response.to_json().unwrap().to_string().contains(planted));
    }
    let log = std::fs::read_to_string(fx.ctx.store.dir().join("events.jsonl")).unwrap();
    assert!(
        !log.contains(planted),
        "no input bytes in rejection records"
    );
}
