use corral_core::agent::{RESULT_MAX_BYTES, RequestReceipt};

use corral_core::agent::pm_state;
use corral_core::{RunOutcome, Status};
use serde_json::Value;

use serde_json::json;

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
fn admission_work_is_equal_with_tenfold_settled_native_history() {
    use corral_core::work::{self, Counter, HotScope};
    let mut measured = Vec::new();
    for scale in [1, 10] {
        let fx = fx();
        let id = worker_task(&fx, "p1");
        let host = worker(&fx, &id);
        let mut task = fx.ctx.store.get(&id).unwrap();
        let at = "2026-01-01T00:00:00Z".parse().unwrap();
        task.created = at;
        task.updated = at;
        task.status_since = Some(at);
        let state = task.current_run_mut().unwrap().agent_host.as_mut().unwrap();
        let template = state.receipts[0].clone();
        for generation in &mut state.generations {
            generation.created = at;
            if let Some(bound) = &mut generation.bound {
                bound.at = at;
            }
        }
        for receipt in &mut state.receipts {
            receipt.at = at;
            receipt.committed_at = receipt.committed_at.map(|_| at);
        }
        let generation = state.generations[0].clone();
        for n in 0..2 * scale {
            let mut old = generation.clone();
            old.n = n + 2;
            old.fenced = Some(corral_core::agent::GenerationFence {
                reason: "finished".into(),
                at,
            });
            state.generations.push(old);
            let mut receipt = stored(
                &template,
                &format!("older-{n:06}"),
                json!({"finished":true}),
            );
            receipt.at = at;
            receipt.committed_at = Some(at);
            state.receipts.push(receipt);
            state.turns.push(
                serde_json::from_value(json!({
                    "generation":n+2,"seq":1,"origin":{"kind":"bridge"},"started":at,"ended":at
                }))
                .unwrap(),
            );
        }
        fx.ctx.store.fixture_put_task(&task).unwrap();
        let fresh: super::envelope::Envelope =
            serde_json::from_slice(&envelope(&host, "artifact", "probe", json!({}), false))
                .unwrap();
        let mut replay: super::envelope::Envelope =
            serde_json::from_slice(&envelope(&host, "artifact", "replay", json!({}), false))
                .unwrap();
        replay.request_id = template.request_id;
        replay.op = template.op;
        replay.payload_sha256 = template.payload_sha256;
        let _hot = HotScope::enter();
        let before = work::snapshot();
        assert!(matches!(
            admission::admit(
                &fx.ctx,
                &fresh,
                admission::Mode::Mutation { settles_turn: None }
            )
            .unwrap(),
            admission::Admission::New(_)
        ));
        assert!(matches!(
            admission::admit(&fx.ctx, &replay, admission::Mode::Lookup).unwrap(),
            admission::Admission::Replay(_)
        ));
        let after = work::snapshot();
        measured.push(
            [
                Counter::TaskHeadDecodes,
                Counter::NestedRowDecodes,
                Counter::HistoryRowDecodes,
                Counter::FullTaskHydrations,
                Counter::DecodedDocBytes,
                Counter::ViewsOpened,
            ]
            .map(|counter| after.total(counter) - before.total(counter)),
        );
    }
    assert_eq!(measured[0], measured[1]);
    assert_eq!(measured[0][3], 0);
}

#[test]
fn task_hello_commit_has_equal_work_with_tenfold_settled_native_history() {
    use corral_core::work::{self, Counter, HotScope};
    struct Unpin;
    impl Drop for Unpin {
        fn drop(&mut self) {
            corral_core::clock::pin_thread(None);
        }
    }
    let at = "2026-01-01T00:00:00Z".parse().unwrap();
    corral_core::clock::pin_thread(Some(at));
    let _clock = Unpin;
    let mut measured = Vec::new();
    for scale in [1, 10] {
        let fx = fx();
        let id = worker_task(&fx, "p1");
        let _first = worker(&fx, &id);
        let mut task = fx.ctx.store.get(&id).unwrap();
        let state = task.current_run_mut().unwrap().agent_host.as_mut().unwrap();
        let template = state.receipts[0].clone();
        for n in 0..2 * scale {
            state.receipts.push(stored(
                &template,
                &format!("older-{n:06}"),
                json!({"finished":true}),
            ));
            state.turns.push(
                serde_json::from_value(json!({
                    "generation":1,"seq":n+1,"origin":{"kind":"bridge"},"started":at,"ended":at
                }))
                .unwrap(),
            );
        }
        fx.ctx.store.fixture_put_task(&task).unwrap();
        let target = BindTarget::Worker {
            task: id.clone(),
            run: "R1".into(),
        };
        let next = bind(&fx, &target, "p1", "next-session");
        assert_eq!(next.identity.generation, 2);
        let _hot = HotScope::enter();
        let before = work::snapshot();
        let receipt = committed(&hello(&fx, &next));
        assert_eq!(receipt.result["generation"], 2);
        let after = work::snapshot();
        measured.push(
            [
                Counter::TaskHeadDecodes,
                Counter::NestedRowDecodes,
                Counter::HistoryRowDecodes,
                Counter::FullTaskHydrations,
                Counter::DecodedDocBytes,
                Counter::ViewsOpened,
            ]
            .map(|counter| after.total(counter) - before.total(counter)),
        );
    }
    assert_eq!(measured[0], measured[1]);
    assert_eq!(measured[0][3], 0);
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
    let saved = state(&fx, &container);
    assert!(saved.generation(1).unwrap().fenced.is_none());
    let settled = committed(&send(
        &fx,
        &event_input(&reviewer, 3, "turn_ended", 1),
        "event",
    ));
    assert_eq!(settled.result["fenced"], json!(true));
    let saved = state(&fx, &container);
    assert!(saved.generation(1).unwrap().fenced.is_some());
    let after = send(&fx, &event_input(&reviewer, 4, "turn_ended", 1), "event");
    assert_eq!(
        code(&after),
        Some("review_closed"),
        "the verdict turn settles once"
    );
}

type Context = (&'static str, fn(&mut corral_core::Task));

#[test]
fn a_judged_review_child_settles_its_open_turn_only_while_it_is_the_current_attempt() {
    let fx = fx();
    let contexts: [Context; 3] = [
        ("current last attempt in Review", |_| {}),
        ("a newer review child", |t| {
            t.runs[0].review_runs.push(run("R1-review2", "p6"))
        }),
        ("the worker reopened by changes", |t| {
            t.status = Status::Running;
            t.runs[0].outcome = None;
        }),
    ];
    for (n, (context, change)) in contexts.into_iter().enumerate() {
        let (rid, target) = reviewer_task(&fx);
        let reviewer = bind(&fx, &target, "p2", &format!("s-judged-{n}"));
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
                        "verdict": "approve", "reviewer": "pi", "sha": "abc",
                        "at": "2026-10-23T00:00:00Z"
                    }))
                    .unwrap(),
                );
                change(t);
                Ok(())
            })
            .unwrap();
        let ending = send(&fx, &event_input(&reviewer, 2, "turn_ended", 1), "event");
        let fenced = state(&fx, &target.container())
            .generation(1)
            .unwrap()
            .fenced
            .is_some();
        if n == 0 {
            assert_eq!(
                committed(&ending).result["fenced"],
                json!(true),
                "{context}"
            );
            assert!(fenced, "{context}");
        } else {
            assert_eq!(code(&ending), Some("review_closed"), "{context}");
            assert!(!fenced, "{context}: the refused ending fences nothing");
        }
    }
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
    let log = serde_json::to_string(&fx.ctx.store.events().unwrap()).unwrap();
    assert!(
        !log.contains(planted),
        "no input bytes in rejection records"
    );
}
