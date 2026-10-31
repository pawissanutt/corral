use corral_core::agent::ContainerRef;
use corral_core::agent::request::{CONTINUATIONS_MAX, RequestLedger};
use serde_json::{Value, json};

use super::super::test_support::{self as ts, Host, committed, envelope, hello, send};
use super::super::{BindTarget, CallResponse, event_request_id, tool_request_id};

/// A bound PM or assistant session driven through real `todo` and `event` receipts.
struct Session {
    fx: ts::Fx,
    host: Host,
    target: BindTarget,
    container: ContainerRef,
    seq: u64,
    turn: u64,
    last_end: Vec<u8>,
}

fn session(pm: bool) -> Session {
    let fx = ts::fx();
    let (target, container) = if pm {
        let target = BindTarget::Pm {
            project: "corral".into(),
        };
        (target, ContainerRef::Pm)
    } else {
        let id = fx
            .ctx
            .store
            .create_assistant("pi", None, Some("a-pane"), None)
            .unwrap()
            .id;
        let container = ContainerRef::Assistant {
            session: id.clone(),
        };
        (BindTarget::Assistant { session: id }, container)
    };
    let host = ts::bind(&fx, &target, "pane", "s1");
    committed(&hello(&fx, &host));
    Session {
        fx,
        host,
        target,
        container,
        seq: 0,
        turn: 0,
        last_end: Vec::new(),
    }
}

fn init(steps: &[&str]) -> Value {
    let steps: Vec<Value> = steps
        .iter()
        .map(|id| json!({"id": id, "label": format!("Step {id}")}))
        .collect();
    json!({"op": "init", "phases": [{"id": "work", "label": "Work", "steps": steps}]})
}

impl Session {
    fn todo(&self, call: &str, payload: Value) -> CallResponse {
        let request = tool_request_id(&self.host.identity.session_id, call);
        send(
            &self.fx,
            &envelope(&self.host, "todo", &request, payload, false),
            "todo",
        )
    }

    fn event(&mut self, payload: Value) -> (Vec<u8>, CallResponse) {
        self.seq += 1;
        let request = event_request_id(self.host.identity.generation, self.seq);
        let input = envelope(&self.host, "event", &request, payload, false);
        let response = send(&self.fx, &input, "event");
        (input, response)
    }

    /// One turn from start to end; `before_end` reports provider or budget events inside it.
    fn turn_with(&mut self, origin: Value, before_end: &[&str], end: Value) -> Value {
        self.turn += 1;
        let n = self.turn;
        committed(
            &self
                .event(json!({"kind": "turn_started", "turn": n, "origin": origin}))
                .1,
        );
        for kind in before_end {
            let mut payload = json!({"kind": kind, "turn": n});
            if *kind == "provider_error" {
                payload["class"] = json!("rate_limit");
            }
            committed(&self.event(payload).1);
        }
        let mut payload = json!({"kind": "turn_ended", "turn": n});
        payload
            .as_object_mut()
            .unwrap()
            .extend(end.as_object().unwrap().clone());
        let (input, response) = self.event(payload);
        self.last_end = input;
        committed(&response).result["request"].clone()
    }

    fn turn(&mut self, origin: Value) -> Value {
        self.turn_with(origin, &[], json!({}))
    }

    fn owner_turn(&mut self) -> Value {
        self.turn(json!({"kind": "owner_terminal"}))
    }

    /// The turn a host starts for a due continuation, under the continuation's key.
    fn continue_turn(&mut self, due: &Value) -> Value {
        let key = due["continuation"]["key"].as_str().unwrap().to_owned();
        self.turn(json!({"kind": "continuation", "idempotency_key": key}))
    }

    fn ledger(&self) -> RequestLedger {
        ts::state(&self.fx, &self.container)
            .todo_requests
            .unwrap_or_default()
    }
}

fn state(request: &Value) -> &str {
    request["continuation"]["state"].as_str().unwrap()
}

#[test]
fn multi_step_plans_get_one_bounded_continuation_per_revision_until_complete() {
    for pm in [true, false] {
        // Given a session that opened a three-step plan and started the first step.
        let mut s = session(pm);
        committed(&s.todo("init", init(&["a", "b", "c"])));
        committed(&s.todo("start-a", json!({"op": "start", "step": "a"})));
        // When the owner's turn ends with that work open.
        let first = s.owner_turn();
        // Then corral grants one continuation for plan P1 at revision 2, naming the next step.
        assert_eq!(first["plan"], "P1");
        assert_eq!(first["revision"], 2);
        assert_eq!(
            first["disposition"],
            json!({"state": "actionable", "next": "a"})
        );
        assert_eq!(first["next"]["id"], "a");
        assert_eq!(
            first["continuation"],
            json!({"state": "due", "key": "continue:P1:2", "attempt": 1, "max": CONTINUATIONS_MAX})
        );
        // When the continued turn makes progress, the next continuation counts on.
        committed(&s.todo("done-a", json!({"op": "done", "step": "a"})));
        committed(&s.todo("start-b", json!({"op": "start", "step": "b", "plan": "P1"})));
        let second = s.continue_turn(&first);
        assert_eq!(second["continuation"]["key"], "continue:P1:4");
        assert_eq!(second["continuation"]["attempt"], 2);
        // When the plan completes, nothing is due and the durable disposition says so.
        for (call, step) in [("done-b", "b"), ("drop-c", "c")] {
            let payload = if step == "c" {
                json!({"op": "drop", "step": step, "reason": "Not needed"})
            } else {
                json!({"op": "done", "step": step})
            };
            committed(&s.todo(call, payload));
        }
        let last = s.continue_turn(&second);
        assert_eq!(last["disposition"]["state"], "complete");
        assert_eq!(state(&last), "not_due");
        let ledger = s.ledger();
        let recorded = ledger.turn.unwrap();
        assert_eq!((recorded.generation, recorded.turn), (1, 3));
        assert_eq!(recorded.plan.as_deref(), Some("P1"));
        assert_eq!(ts::events(&s.fx, "todo_continuation_stopped"), 0);
    }
}

#[test]
fn a_continued_turn_without_progress_stops_once_with_a_visible_reason() {
    for pm in [true, false] {
        // Given a due continuation for an open plan.
        let mut s = session(pm);
        committed(&s.todo("init", init(&["a"])));
        let due = s.owner_turn();
        assert_eq!(state(&due), "due");
        // When the continued turn ends without changing the plan.
        let stalled = s.continue_turn(&due);
        // Then continuation stops with a machine reason and one board event.
        assert_eq!(
            stalled["continuation"],
            json!({"state": "stopped", "reason": "no_progress"})
        );
        assert_eq!(ts::events(&s.fx, "todo_continuation_stopped"), 1);
        // And later turns at the same revision never nudge again.
        let again = s.owner_turn();
        assert_eq!(state(&again), "stopped");
        assert_eq!(ts::events(&s.fx, "todo_continuation_stopped"), 1);
        // Until the plan moves: the owner's next turn with progress is continued again.
        committed(&s.todo("start", json!({"op": "start", "step": "a"})));
        let resumed = s.owner_turn();
        assert_eq!(resumed["continuation"]["key"], "continue:P1:2");
        assert_eq!(resumed["continuation"]["attempt"], 1);
    }
}

#[test]
fn continuations_stop_at_the_bound_even_while_the_plan_keeps_growing() {
    for pm in [true, false] {
        // Given a plan whose continued turns each append one more step.
        let mut s = session(pm);
        committed(&s.todo("init", init(&["a"])));
        let mut last = s.owner_turn();
        for n in 1..=CONTINUATIONS_MAX {
            assert_eq!(last["continuation"]["attempt"], n);
            let step = json!({"op": "append", "phase": "work", "steps": [
                {"id": format!("more-{n}"), "label": "More"}]});
            committed(&s.todo(&format!("append-{n}"), step));
            last = s.continue_turn(&last);
        }
        // Then the turn after the last allowed continuation stops at the limit.
        assert_eq!(
            last["continuation"],
            json!({"state": "stopped", "reason": "limit"})
        );
        assert_eq!(ts::events(&s.fx, "todo_continuation_stopped"), 1);
        // And an owner turn without progress stays stopped rather than starting a new run.
        assert_eq!(state(&s.owner_turn()), "stopped");
    }
}

#[test]
fn duplicate_turn_reports_and_turns_at_a_continued_revision_never_double_continue() {
    for pm in [true, false] {
        // Given a due continuation.
        let mut s = session(pm);
        committed(&s.todo("init", init(&["a"])));
        let due = s.owner_turn();
        let ledger = s.ledger();
        // When the host's turn_ended report is delivered again (a retried or duplicate wake).
        let replay = send(&s.fx, &s.last_end.clone(), "event");
        // Then it replays the stored decision and changes nothing.
        assert!(matches!(
            replay,
            CallResponse::Receipt { replayed: true, .. }
        ));
        assert_eq!(replay.receipt().unwrap().result["request"], due);
        assert_eq!(s.ledger(), ledger);
        // When a corral-delivered turn ends at the same revision before the continuation ran.
        let bridged = s.turn(json!({"kind": "bridge", "idempotency_key": "wake-1"}));
        // Then it is a duplicate of the continuation already granted, not a second one.
        assert_eq!(
            bridged["continuation"],
            json!({"state": "duplicate", "key": "continue:P1:1"})
        );
        assert_eq!(s.ledger().continued, ledger.continued);
    }
}

#[test]
fn cancelled_aborted_failed_and_exhausted_turns_are_held_without_a_nudge() {
    for pm in [true, false] {
        let mut s = session(pm);
        committed(&s.todo("init", init(&["a"])));
        // When turns with open work end cancelled, aborted, failed or out of budget.
        let held = [
            s.turn_with(
                json!({"kind": "owner_terminal"}),
                &[],
                json!({"aborted": true}),
            ),
            s.turn_with(json!({"kind": "unknown"}), &[], json!({"cancelled": true})),
            s.turn_with(json!({"kind": "unknown"}), &["provider_error"], json!({})),
            s.turn_with(json!({"kind": "unknown"}), &["budget_exhausted"], json!({})),
        ];
        // Then each is held with its end recorded, and no continuation was granted.
        let ends: Vec<&str> = held.iter().map(|r| r["end"].as_str().unwrap()).collect();
        assert_eq!(
            ends,
            [
                "cancelled",
                "cancelled",
                "provider_failed",
                "budget_exhausted"
            ]
        );
        assert!(held.iter().all(|r| state(r) == "held"));
        assert_eq!(s.ledger().continued, None);
        // And a normal turn afterwards is continued for the first time.
        assert_eq!(s.owner_turn()["continuation"]["attempt"], 1);
    }
}

#[test]
fn event_and_owner_waits_hold_only_the_steps_that_wait_on_them() {
    for pm in [true, false] {
        // Given a plan with an owner question on one step and an independent step.
        let mut s = session(pm);
        committed(&s.todo("init", init(&["ask", "free"])));
        let owner = json!({"op": "wait", "step": "ask",
            "on": {"kind": "owner", "question": "Which project?"}});
        let receipt = committed(&s.todo("wait-ask", owner));
        assert_eq!(receipt.result["disposition"]["next"], "free");
        // When the turn ends, the independent step is still continued.
        let independent = s.owner_turn();
        assert_eq!(independent["disposition"]["next"], "free");
        assert_eq!(state(&independent), "due");
        // When the independent step waits on a named event too, the owner answer gates.
        let event = json!({"op": "wait", "step": "free",
            "on": {"kind": "event", "event": "T-12 lands on main"}});
        committed(&s.todo("wait-free", event));
        let owner_wait = s.owner_turn();
        assert_eq!(
            owner_wait["disposition"],
            json!({"state": "awaiting_owner", "question": "Which project?", "steps": ["ask"]})
        );
        assert_eq!(state(&owner_wait), "not_due");
        // When the owner answered and only the event wait remains, it waits on the event.
        committed(&s.todo(
            "drop-ask",
            json!({"op": "drop", "step": "ask", "reason": "Answered: corral"}),
        ));
        let event_wait = s.owner_turn();
        assert_eq!(
            event_wait["disposition"],
            json!({"state": "waiting_event", "event": "T-12 lands on main", "steps": ["free"]})
        );
        assert_eq!(state(&event_wait), "not_due");
        // And starting the step when the event arrives clears its wait.
        let started = committed(&s.todo("start-free", json!({"op": "start", "step": "free"})));
        assert!(started.result["progress"].get("waits").is_none());
        // A wait without text or on a finished step is refused.
        for payload in [
            json!({"op": "wait", "step": "free", "on": {"kind": "event", "event": " "}}),
            json!({"op": "wait", "step": "ask", "on": {"kind": "owner", "question": "Again?"}}),
        ] {
            assert_eq!(ts::code(&s.todo("bad-wait", payload)), Some("bad_request"));
        }
    }
}

#[test]
fn completed_plans_roll_over_with_history_and_open_plans_are_never_erased() {
    for pm in [true, false] {
        // Given a plan with open work.
        let s = session(pm);
        let first = committed(&s.todo("init-1", init(&["a", "b"])));
        // When an unrelated request tries to start a new plan over it.
        let refused = s.todo("init-2", init(&["x"]));
        // Then the open plan is kept.
        assert_eq!(ts::code(&refused), Some("bad_request"));
        assert!(format!("{refused:?}").contains("todo_plan_open"));
        // When every step is done or dropped, a new plan rolls the old one into history.
        committed(&s.todo("done-a", json!({"op": "done", "step": "a"})));
        committed(&s.todo(
            "drop-b",
            json!({"op": "drop", "step": "b", "reason": "Out of scope"}),
        ));
        let next = committed(&s.todo("init-3", init(&["a"])));
        assert_eq!(next.result["plan"], "P2");
        assert_eq!(next.result["revision"], 4);
        assert_eq!(next.result["closed_plans"], 1);
        let ledger = s.ledger();
        let closed = &ledger.history[0];
        assert_eq!(
            (
                closed.plan.as_str(),
                closed.revision,
                closed.done,
                closed.dropped
            ),
            ("P1", 3, 1, 1)
        );
        // When a stale update names the closed plan, it is refused without touching P2.
        let stale = s.todo("stale", json!({"op": "done", "step": "a", "plan": "P1"}));
        assert!(format!("{stale:?}").contains("stale_todo_plan"));
        // And replaying the first plan's init returns its original receipt, not a new plan.
        let replay = s.todo("init-1", init(&["a", "b"]));
        assert_eq!(replay.receipt(), Some(&first));
        let current = ts::state(&s.fx, &s.container).todo.unwrap();
        assert_eq!((current.plan.as_deref(), current.revision), (Some("P2"), 4));
    }
}

#[test]
fn a_new_owner_instruction_opens_a_phase_and_restarts_the_continuation_count() {
    for pm in [true, false] {
        // Given a plan already continued twice.
        let mut s = session(pm);
        committed(&s.todo("init", init(&["a", "b"])));
        let first = s.owner_turn();
        committed(&s.todo("start-a", json!({"op": "start", "step": "a"})));
        assert_eq!(s.continue_turn(&first)["continuation"]["attempt"], 2);
        // When the owner adds work mid-request, it lands in a new phase of the same plan.
        let correction = json!({"op": "append", "phase": "fix", "label": "Owner correction",
            "steps": [{"id": "fix", "label": "Apply the correction"}]});
        let appended = committed(&s.todo("append", correction));
        assert_eq!(
            appended.result["progress"]["phases"][1]["label"],
            "Owner correction"
        );
        let unnamed =
            json!({"op": "append", "phase": "missing", "steps": [{"id": "z", "label": "Z"}]});
        assert_eq!(ts::code(&s.todo("unnamed", unnamed)), Some("bad_request"));
        // Then the owner's turn starts a fresh continuation count for the same plan.
        let after = s.owner_turn();
        assert_eq!(after["plan"], "P1");
        assert_eq!(after["continuation"]["attempt"], 1);
    }
}

#[test]
fn a_resumed_generation_keeps_the_ledger_and_does_not_repeat_a_granted_continuation() {
    for pm in [true, false] {
        // Given a due continuation whose host was lost before it ran.
        let mut s = session(pm);
        committed(&s.todo("init", init(&["a", "b"])));
        assert_eq!(state(&s.owner_turn()), "due");
        // When the store is reopened and the same container resumes as generation 2.
        s.fx.ctx.store = corral_core::Store::open_unobserved(s.fx.ctx.store.root()).unwrap();
        s.host = ts::bind(&s.fx, &s.target, "pane", "s2");
        committed(&hello(&s.fx, &s.host));
        s.seq = 0;
        s.turn = 0;
        // Then the readback the new host restores from carries the plan and its request record.
        let snapshot = match &s.container {
            ContainerRef::Pm => super::super::install::state_value(
                &s.fx.ctx,
                &super::super::cli::AgentTarget::Pm,
                &json!({}),
            )
            .unwrap()["container"]
                .clone(),
            _ => serde_json::to_value(ts::state(&s.fx, &s.container)).unwrap(),
        };
        assert_eq!(snapshot["todo"]["plan"], "P1");
        assert_eq!(
            snapshot["todo_requests"]["continued"]["key"],
            "continue:P1:1"
        );
        // And the recovery turn at the same revision does not grant the same continuation again.
        let recovered = s.turn(json!({"kind": "bridge", "idempotency_key": "recovery"}));
        assert_eq!(state(&recovered), "duplicate");
        // While progress in the new generation is continued as usual.
        committed(&s.todo("start-a", json!({"op": "start", "step": "a"})));
        let resumed = s.owner_turn();
        assert_eq!(resumed["continuation"]["key"], "continue:P1:2");
        assert_eq!(s.ledger().turn.unwrap().generation, 2);
    }
}

#[test]
fn a_board_full_of_work_never_continues_a_session_without_an_open_plan() {
    for pm in [true, false] {
        // Given open board work and a session that never kept a plan.
        let mut s = session(pm);
        for status in [corral_core::Status::Ready, corral_core::Status::Running] {
            s.fx.ctx
                .store
                .create("Board work", Some("corral"), status, "")
                .unwrap();
        }
        // When its turns end.
        let request = s.owner_turn();
        // Then nothing is due and no request ledger is written for it.
        assert_eq!(request["disposition"]["state"], "complete");
        assert_eq!(state(&request), "not_due");
        assert!(ts::state(&s.fx, &s.container).todo_requests.is_none());
    }
}

#[test]
fn workers_keep_their_run_ledger_without_plans_waits_or_request_records() {
    // Given a bound worker with an initialized ledger.
    let fx = ts::fx();
    let task = ts::worker_task(&fx, "p1");
    let target = BindTarget::Worker {
        task: task.clone(),
        run: "R1".into(),
    };
    let host = ts::bind(&fx, &target, "p1", "w");
    committed(&hello(&fx, &host));
    let todo = |call: &str, payload: Value| {
        let request = tool_request_id(&host.identity.session_id, call);
        send(
            &fx,
            &envelope(&host, "todo", &request, payload, false),
            "todo",
        )
    };
    committed(&todo("init", init(&["a"])));
    // When it uses session-only arguments.
    let waits = json!({"op": "wait", "step": "a", "on": {"kind": "event", "event": "CI"}});
    // Then they are refused and its turns carry no request decision.
    assert_eq!(ts::code(&todo("wait", waits)), Some("bad_request"));
    let named = json!({"op": "start", "step": "a", "plan": "P1"});
    assert_eq!(ts::code(&todo("plan", named)), Some("bad_request"));
    let start = envelope(
        &host,
        "event",
        &event_request_id(1, 1),
        json!({"kind": "turn_started", "turn": 1}),
        false,
    );
    committed(&send(&fx, &start, "event"));
    let end = envelope(
        &host,
        "event",
        &event_request_id(1, 2),
        json!({"kind": "turn_ended", "turn": 1}),
        false,
    );
    assert!(committed(&send(&fx, &end, "event")).result["request"].is_null());
    let run = fx.ctx.store.get(&task).unwrap();
    let progress = run.current_run().unwrap().progress.clone().unwrap();
    assert_eq!((progress.plan, progress.waits.len()), (None, 0));
}
