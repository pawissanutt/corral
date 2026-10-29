use corral_core::agent::{HostHealth, paths, pm_state};
use corral_core::{RunOutcome, Status};
use serde_json::{Value, json};

use super::test_support::*;
use super::*;

#[test]
fn continued_reviewer_hello_keeps_the_session_but_not_old_callback_authority() {
    // Given a new reviewer container bound to one exact prior role transcript.
    let fx = fx();
    let (id, target) = reviewer_task(&fx);
    let mut host = bind(&fx, &target, "p2", "continued-session");
    let selected = fx.ctx.store.state_dir().join("prior-review-session");
    host.sessions = selected.clone();
    fx.ctx.store.update(&id, |task| {
        task.runs[0].review_runs[0].session = Some(serde_json::from_value(json!({
            "policy":"reuse", "directory":selected, "harness":"pi-native",
            "id":"continued-session", "transcript":selected.join("session.jsonl"), "action":"resumed"
        }))?);
        Ok(())
    }).unwrap();

    // When the new generation proves its nonce and the selected conversation.
    committed(&hello(&fx, &host));

    // Then its own binding retains the current callback, never an older attempt's authority.
    let saved = state(&fx, &target.container());
    assert_eq!(
        saved
            .current_generation()
            .unwrap()
            .bound
            .as_ref()
            .unwrap()
            .session_id,
        "continued-session"
    );
    let mut stale = host;
    stale.identity.callback = Some("old-callback".into());
    let refused = send(&fx, &event_input(&stale, 1, "turn_started", 1), "event");
    assert_eq!(code(&refused), Some("auth_failed"));
}

#[test]
fn continued_reviewer_hello_refuses_a_different_session_id() {
    // Given a new review child whose launcher selected an exact prior conversation.
    let fx = fx();
    let (id, target) = reviewer_task(&fx);
    let host = bind(&fx, &target, "p2", "empty-replacement");
    fx.ctx.store.update(&id, |task| {
        task.runs[0].review_runs[0].session = Some(serde_json::from_value(json!({
            "policy":"reuse", "directory":host.sessions, "harness":"pi-native",
            "id":"recorded-session", "transcript":host.sessions.join("session.jsonl"), "action":"resumed"
        }))?);
        Ok(())
    }).unwrap();

    // When a replacement process tries to claim reuse with another conversation.
    let refused = hello(&fx, &host);

    // Then the new generation remains unbound and cannot act as reviewer.
    assert_eq!(code(&refused), Some("bad_request"));
    assert!(
        state(&fx, &target.container())
            .current_generation()
            .unwrap()
            .bound
            .is_none()
    );
}

#[test]
fn worker_reviewer_and_pm_requests_commit_their_receipt_with_the_mutation() {
    // Given one bound host per role: a worker run, a review child and the AgentPm area.
    let fx = fx();
    let worker = worker_task(&fx, "p1");
    let (reviewed, review_target) = reviewer_task(&fx);
    let targets = [
        (
            BindTarget::Worker {
                task: worker.clone(),
                run: "R1".into(),
            },
            "p1",
        ),
        (review_target, "p2"),
        (
            BindTarget::Pm {
                project: "corral".into(),
            },
            "pm-pane",
        ),
    ];
    for (target, pane) in targets {
        let host = bind(&fx, &target, pane, &format!("s-{pane}"));
        let bound = committed(&hello(&fx, &host));
        let landed = match target {
            BindTarget::Planner { .. } | BindTarget::PlanReviewer { .. } => {
                json!(["hello", "event", "receipt_lookup"])
            }
            BindTarget::Pm { .. } => json!([
                "hello",
                "event",
                "receipt_lookup",
                "todo",
                "question_answer",
                "question_escalate",
                "question_cancel",
                "reply",
                "comment",
                "capture",
                "read_brief",
                "read_next",
                "read_show",
                "read_list",
                "read_search",
                "read_route",
                "read_artifacts",
                "read_lessons"
            ]),
            BindTarget::Worker { .. } => json!([
                "hello",
                "event",
                "receipt_lookup",
                "ask",
                "withdraw",
                "wait",
                "progress",
                "todo",
                "checkpoint",
                "evidence",
                "artifact",
                "gate",
                "done"
            ]),
            BindTarget::Reviewer { .. } => {
                json!([
                    "hello",
                    "event",
                    "receipt_lookup",
                    "ask",
                    "withdraw",
                    "evidence",
                    "artifact",
                    "verdict"
                ])
            }
        };
        assert_eq!(bound.result["ops"], landed);
        // When each sends a new authenticated lifecycle event.
        let receipt = committed(&send(
            &fx,
            &event_input(&host, 1, "turn_started", 1),
            "event",
        ));
        // Then the named container holds the receipt beside the turn it committed.
        let container = target.container();
        let saved = state(&fx, &container);
        assert_eq!(saved.receipt(&receipt.request_id), Some(&receipt));
        assert!(
            saved
                .turns
                .iter()
                .any(|t| t.seq == 1 && t.started.is_some())
        );
        assert_eq!(saved.binding.health, HostHealth::Healthy);
        assert_eq!(receipt.identity.role, host.identity.role);
    }
    let review = fx.ctx.store.get(&reviewed).unwrap();
    assert!(
        review.runs[0].agent_host.is_none(),
        "reviewer receipts live on the child"
    );
    assert!(review.runs[0].review_runs[0].agent_host.is_some());
    let pm = pm_state::load(&fx.ctx.store).unwrap().unwrap();
    assert_eq!(
        pm.state.receipts.len(),
        2,
        "PM hello and event, with no task or run"
    );
    assert!(pm.state.receipts.iter().all(|r| r.identity.task.is_none()));
}

#[test]
fn committed_requests_replay_unchanged_from_historical_runs_and_review_children() {
    let fx = fx();
    let id = worker_task(&fx, "p1");
    let target = BindTarget::Worker {
        task: id.clone(),
        run: "R1".into(),
    };
    let host = bind(&fx, &target, "p1", "s-w");
    committed(&hello(&fx, &host));
    let input = event_input(&host, 1, "turn_started", 1);
    let stored = committed(&send(&fx, &input, "event"));
    let replay = |fx: &Fx| match send(fx, &input, "event") {
        CallResponse::Receipt {
            receipt,
            replayed: true,
        } => receipt,
        other => panic!("expected the stored receipt, got {other:?}"),
    };
    // done moves the run to Review: replay is unchanged, the open turn still settles, new work is refused.
    fx.ctx
        .store
        .update(&id, |t| {
            t.status = Status::Review;
            t.runs[0].outcome = Some(RunOutcome::Done);
            Ok(())
        })
        .unwrap();
    assert_eq!(replay(&fx), stored);
    committed(&send(&fx, &event_input(&host, 2, "turn_ended", 1), "event"));
    let refused = send(&fx, &event_input(&host, 3, "turn_started", 2), "event");
    assert_eq!(code(&refused), Some("run_ended"));
    // retry supersedes R1 with R2: R1's receipt still replays by name.
    fx.ctx
        .store
        .update(&id, |t| {
            t.status = Status::Running;
            t.runs[0].outcome = Some(RunOutcome::Superseded);
            t.runs.push(run("R2", "p3"));
            Ok(())
        })
        .unwrap();
    assert_eq!(replay(&fx), stored);
    let refused = send(&fx, &event_input(&host, 4, "turn_started", 3), "event");
    assert_eq!(code(&refused), Some("run_ended"));

    // A review child after its verdict, and after a newer review child started.
    for newer_child in [false, true] {
        let (rid, target) = reviewer_task(&fx);
        let reviewer = bind(&fx, &target, "p2", &format!("s-r{newer_child}"));
        committed(&hello(&fx, &reviewer));
        let input = event_input(&reviewer, 1, "turn_started", 1);
        let stored = committed(&send(&fx, &input, "event"));
        fx.ctx
            .store
            .update(&rid, |t| {
                let child = &mut t.runs[0];
                if newer_child {
                    child.review_runs.push(run("R1-review2", "p4"));
                } else {
                    child.review_runs[0].review = Some(
                        serde_json::from_value(json!({
                            "verdict": "approve", "reviewer": "pi", "sha": "abc",
                            "at": "2026-10-23T00:00:00Z"
                        }))
                        .unwrap(),
                    );
                }
                Ok(())
            })
            .unwrap();
        let again = send(&fx, &input, "event");
        assert_eq!(again.receipt(), Some(&stored));
        let refused = send(&fx, &event_input(&reviewer, 2, "turn_started", 2), "event");
        assert_eq!(code(&refused), Some("review_closed"));
    }
}

#[test]
fn reused_request_id_with_another_payload_is_rejected_before_any_effect() {
    let fx = fx();
    let id = worker_task(&fx, "p1");
    let target = BindTarget::Worker {
        task: id.clone(),
        run: "R1".into(),
    };
    let host = bind(&fx, &target, "p1", "s-w");
    committed(&hello(&fx, &host));
    committed(&send(
        &fx,
        &event_input(&host, 1, "turn_started", 1),
        "event",
    ));
    let before = task_bytes(&fx, &id);
    let rejections = events(&fx, "agent_request_rejected");
    // Same ev-1-1, different payload.
    let reused = send(&fx, &event_input(&host, 1, "turn_started", 7), "event");
    assert_eq!(code(&reused), Some("payload_mismatch"));
    assert_eq!(task_bytes(&fx, &id), before, "nothing changes");
    assert_eq!(events(&fx, "agent_request_rejected"), rejections + 1);
}

#[test]
fn stale_foreign_and_cross_container_requests_are_rejected_once_without_secrets() {
    let fx = fx();
    let a = worker_task(&fx, "p1");
    let b = worker_task(&fx, "p5");
    let target = |task: &str| BindTarget::Worker {
        task: task.into(),
        run: "R1".into(),
    };
    let first = bind(&fx, &target(&a), "p1", "s-a1");
    committed(&hello(&fx, &first));
    let early = committed(&send(
        &fx,
        &event_input(&first, 1, "turn_started", 1),
        "event",
    ));
    let other = bind(&fx, &target(&b), "p5", "s-b");
    committed(&hello(&fx, &other));
    // Resume opens generation 2 of the same container, which fences generation 1.
    let second = bind(&fx, &target(&a), "p1", "s-a2");
    committed(&hello(&fx, &second));

    // Generation n+1 reads generation n's receipt with its own host_cap; the fenced one reads its own.
    assert_eq!(lookup(&fx, &second, &early.request_id), Some(early.clone()));
    assert!(lookup(&fx, &first, &early.request_id).is_some());
    assert!(lookup(&fx, &other, &early.request_id).is_none());

    let mut wrong_cap = second.clone();
    wrong_cap.host_cap = secret("forged", 2);
    let mut foreign_store = second.clone();
    foreign_store.identity.store = "st-0000000000000000".into();
    let mut foreign_project = second.clone();
    foreign_project.identity.project = "elsewhere".into();
    let mut cross = other.clone();
    cross.identity.task = Some(a.clone());
    let cases = [
        (&first, "agent_fenced"),
        (&wrong_cap, "auth_failed"),
        (&foreign_store, "foreign_store"),
        (&foreign_project, "foreign_project"),
        (&cross, "auth_failed"),
    ];
    let before = (task_bytes(&fx, &a), task_bytes(&fx, &b));
    for (seq, (host, expected)) in (10..).zip(cases) {
        let rejected = events(&fx, "agent_request_rejected");
        let response = send(&fx, &event_input(host, seq, "turn_started", seq), "event");
        assert_eq!(code(&response), Some(expected));
        assert_eq!(
            events(&fx, "agent_request_rejected"),
            rejected + 1,
            "{expected} recorded once"
        );
    }
    assert_eq!(
        (task_bytes(&fx, &a), task_bytes(&fx, &b)),
        before,
        "no effect"
    );

    // The run moved to another pane: the generation no longer matches the saved pane.
    fx.ctx
        .store
        .update(&a, |t| {
            t.runs[0].herdr.as_mut().unwrap().pane_id = "p9".into();
            Ok(())
        })
        .unwrap();
    let moved = send(&fx, &event_input(&second, 20, "turn_started", 20), "event");
    assert_eq!(code(&moved), Some("pane_mismatch"));

    let log = std::fs::read_to_string(fx.ctx.store.dir().join("events.jsonl")).unwrap();
    for host in [&first, &second, &other, &wrong_cap] {
        for secret in [&host.host_cap, &host.ctl_cap, &host.nonce] {
            assert!(
                !log.contains(secret.as_str()),
                "no secret bytes in the event log"
            );
        }
    }
    let task_file = String::from_utf8(task_bytes(&fx, &a)).unwrap();
    assert!(!task_file.contains(&second.host_cap) && !task_file.contains(&second.nonce));
}

#[test]
fn a_fence_between_admission_and_commit_refuses_and_the_missing_receipt_is_final() {
    let fx = fx();
    let id = worker_task(&fx, "p1");
    let worker = BindTarget::Worker {
        task: id.clone(),
        run: "R1".into(),
    };
    let pm = BindTarget::Pm {
        project: "corral".into(),
    };
    for (target, pane) in [(worker, "p1"), (pm, "pm-pane")] {
        let host = bind(&fx, &target, pane, &format!("s-{pane}"));
        committed(&hello(&fx, &host));
        let container = target.container();
        let before = state(&fx, &container);
        // Given admission passed, the generation is fenced before the final transaction.
        let store = fx.ctx.store.clone();
        let fenced = container.clone();
        receipts::FAILPOINT.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |point: &str| {
                if point == "agent_admitted_before_commit" {
                    assert!(fence_generation(&store, &fenced, 1, "resume").unwrap());
                }
            }));
        });
        let input = event_input(&host, 1, "turn_started", 1);
        let refused = send(&fx, &input, "event");
        receipts::FAILPOINT.with(|hook| hook.borrow_mut().take());
        // Then the final transaction refuses it and writes neither the turn nor a receipt.
        assert_eq!(code(&refused), Some("agent_fenced"));
        let after = state(&fx, &container);
        assert_eq!(after.receipts, before.receipts);
        assert_eq!(after.turns, before.turns);
        assert!(after.generation(1).unwrap().fenced.is_some());
        // And the request can never commit later.
        assert_eq!(code(&send(&fx, &input, "event")), Some("agent_fenced"));
        assert!(
            state(&fx, &container)
                .receipt(&event_request_id(1, 1))
                .is_none()
        );
    }
}

#[test]
fn hello_consumes_the_bind_nonce_once_and_records_both_secret_digests() {
    let fx = fx();
    let id = worker_task(&fx, "p1");
    let target = BindTarget::Worker {
        task: id.clone(),
        run: "R1".into(),
    };
    let host = bind(&fx, &target, "p1", "s-w");
    let first = committed(&hello(&fx, &host));
    let saved = state(&fx, &target.container());
    let bound = saved.generation(1).unwrap().bound.clone().unwrap();
    assert_eq!(
        bound.host_cap_sha256,
        paths::sha256_hex(host.host_cap.as_bytes())
    );
    assert_eq!(
        bound.ctl_cap_sha256,
        paths::sha256_hex(host.ctl_cap.as_bytes())
    );
    assert_eq!(bound.session_file, host.sessions.join("session.jsonl"));
    assert_eq!(bound.session_id, "s-w");
    // A lost acknowledgement replays the stored receipt; agent_bound is logged once.
    assert_eq!(hello(&fx, &host).receipt(), Some(&first));
    assert_eq!(events(&fx, "agent_bound"), 1);
    // A second hello with the same nonce is refused, with the bound or a different host_cap.
    let second = send(&fx, &hello_input(&host, &event_request_id(1, 9)), "hello");
    assert_eq!(code(&second), Some("nonce_consumed"));
    let mut thief = host.clone();
    thief.host_cap = secret("thief", 1);
    let stolen = send(&fx, &hello_input(&thief, &event_request_id(1, 0)), "hello");
    assert_eq!(code(&stolen), Some("nonce_consumed"));
    assert_eq!(
        state(&fx, &target.container()).generation(1).unwrap().bound,
        Some(bound)
    );
    // A wrong nonce never binds a fresh generation, and hello must come from its pane.
    let mut next = bind(&fx, &target, "p1", "s-w2");
    let mut forged = next.clone();
    forged.nonce = secret("nonce", 2);
    let refused = send(&fx, &hello_input(&forged, &event_request_id(2, 0)), "hello");
    assert_eq!(code(&refused), Some("auth_failed"));
    next.pane = "p7".into();
    let elsewhere = send(&fx, &hello_input(&next, &event_request_id(2, 0)), "hello");
    assert_eq!(code(&elsewhere), Some("pane_mismatch"));
    assert!(
        state(&fx, &target.container())
            .generation(2)
            .unwrap()
            .bound
            .is_none()
    );
}

#[test]
fn landed_pm_capture_and_read_next_preserve_receipts_and_role_rejection() {
    // Given an authenticated PM in a configured project.
    let mut fx = fx();
    fx.ctx.store.config.projects.insert(
        "corral".into(),
        serde_json::from_value(json!({"path":fx.ctx.store.root()})).unwrap(),
    );
    fx.ctx.store.save_config().unwrap();
    let pm = bind(
        &fx,
        &BindTarget::Pm {
            project: "corral".into(),
        },
        "pm-pane",
        "s-pm",
    );
    committed(&hello(&fx, &pm));
    let rejected = events(&fx, "agent_request_rejected");
    // When the formerly unavailable capture runs, then creation and its marker are observable.
    let id = tool_request_id(&pm.identity.session_id, "capture");
    let input = envelope(
        &pm,
        "capture",
        &id,
        json!({"project":"corral","title":"captured","body":"bounded work"}),
        false,
    );
    let captured = committed(&send(&fx, &input, "capture"));
    let task = fx
        .ctx
        .store
        .get(captured.result["task"].as_str().unwrap())
        .unwrap();
    assert_eq!(task.agent_origin.unwrap().request_id, id);
    let before = pm_state::load(&fx.ctx.store).unwrap().unwrap();
    // When the formerly unavailable read_next runs, then it reads without changing PM state.
    let next = envelope(
        &pm,
        "read_next",
        &tool_request_id("s-pm", "next"),
        json!({}),
        false,
    );
    let response = send(&fx, &next, "read_next");
    assert_eq!(
        response.receipt().unwrap().state,
        corral_core::agent::ReceiptState::Committed
    );
    assert!(response.receipt().unwrap().result["kind"].is_string());
    assert_eq!(before, pm_state::load(&fx.ctx.store).unwrap().unwrap());
    assert_eq!(
        events(&fx, "agent_request_rejected"),
        rejected,
        "capability answers"
    );
    let not_allowed = envelope(&pm, "done", &tool_request_id("s-pm", "x"), json!({}), false);
    assert_eq!(
        code(&send(&fx, &not_allowed, "done")),
        Some("op_not_allowed")
    );
}

#[test]
fn envelope_bounds_reject_malformed_requests_and_derive_stable_ids() {
    let fx = fx();
    let id = worker_task(&fx, "p1");
    let host = bind(
        &fx,
        &BindTarget::Worker {
            task: id,
            run: "R1".into(),
        },
        "p1",
        "s-w",
    );
    let mut forged: Value =
        serde_json::from_slice(&event_input(&host, 1, "turn_started", 1)).unwrap();
    forged["payload"]["turn"] = json!(2);
    let tampered = serde_json::to_vec(&forged).unwrap();
    let nonce_elsewhere = envelope(
        &host,
        "event",
        "ev-1-2",
        json!({"kind": "turn_started", "turn": 2}),
        true,
    );
    let oversize = vec![b' '; ENVELOPE_MAX_BYTES + 1];
    for (input, op) in [
        (tampered.as_slice(), "event"),
        (nonce_elsewhere.as_slice(), "event"),
        (oversize.as_slice(), "event"),
        (b"{\"version\":1}".as_slice(), "event"),
        (event_input(&host, 1, "turn_started", 1).as_slice(), "hello"),
    ] {
        assert_eq!(code(&send(&fx, input, op)), Some("bad_request"));
    }
    assert_eq!(tool_request_id("s", "call"), tool_request_id("s", "call"));
    assert_ne!(tool_request_id("s", "call"), tool_request_id("s2", "call"));
    assert_eq!(tool_request_id("s", "call").len(), 35);
    assert_eq!(
        canonical_json(&json!({"b": 1, "a": {"d": [1, "x"], "c": null}})),
        r#"{"a":{"c":null,"d":[1,"x"]},"b":1}"#
    );
}
