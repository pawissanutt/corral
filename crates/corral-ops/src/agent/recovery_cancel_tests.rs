//! T-252 recovery: a turn `corral agent cancel` stopped is `cancelled` for every consumer, however
//! the host's lifecycle reports, the cancel's reply and sync's consumers interleave. The reports
//! commit while the cancel's reply is pending (from inside the bridge's cancel handler), or after
//! it, with OD11, provider-stall and the reviewer watchdog run between each report, the reviewer
//! watchdog ahead of the agent hook as in sync. Each ordering is fixed by construction, never by
//! timing. No case may publish a nudge, spend the generation's nudge, retry or escalate a
//! provider error, or close the reviewer.
use std::collections::HashMap;

use corral_core::Status;
use corral_core::agent::TurnClass;
use corral_herdr::{AgentStatus, PaneInfo};
use serde_json::{Value, json};

use super::tests::{messages, run_recipient, state};
use super::*;
use crate::agent::bridge::fixture::{self as f, Fx, HostProc};
use crate::agent::test_support as ts;

#[derive(Clone, Copy, Debug)]
enum Order {
    /// The reports commit, and the consumers run, while the cancel's reply is pending.
    WhilePending,
    /// The reports commit after the cancel returned, the consumers running between them.
    AfterReply,
}

fn report(root: &Path, host: &ts::Host, seq: u64, payload: Value) {
    let ctx = f::ctx_at(root);
    let id = crate::agent::event_request_id(host.identity.generation, seq);
    let input = ts::envelope(host, "event", &id, payload, false);
    ts::committed(&crate::agent::call(&ctx, "event", &input).unwrap());
}

fn inventory() -> HashMap<String, PaneInfo> {
    let pane = |id: &str| PaneInfo {
        pane_id: id.into(),
        workspace_id: "w0".into(),
        tab_id: "w0:t1".into(),
        agent: None,
        status: AgentStatus::Idle,
        title: None,
        cwd: None,
    };
    ["w0:p1", "w0:p2"]
        .into_iter()
        .map(|id| (id.to_owned(), pane(id)))
        .collect()
}

/// One pass of every consumer of an ended turn, in sync's order: the reviewer watchdog (with
/// its provider-stall driver), then the agent hook (OD11), then the worker's provider stall.
fn consumers(root: &Path, id: &str) {
    let ctx = f::ctx_at(root);
    let now = jiff::Timestamp::now();
    crate::reviewer::reconcile_review_at(&ctx, id, now, &inventory()).unwrap();
    crate::agent::hooks::sync::task(&ctx, &ctx.store.get(id).unwrap(), now).unwrap();
    let task = ctx.store.get(id).unwrap();
    crate::agent::hooks::stall::advance_agent(&ctx, &task, &task.runs[0], now).unwrap();
}

/// The host's reports of its cancelled turn 1, failed on `provider` first when given, with every
/// consumer run after each report.
fn cancelled_turn(root: &Path, host: &ts::Host, id: &str, provider: Option<&str>) {
    let origin = json!({"kind": "bridge", "idempotency_key": "k1"});
    report(
        root,
        host,
        1,
        json!({"kind": "turn_started", "turn": 1, "origin": origin}),
    );
    consumers(root, id);
    let mut seq = 2;
    if let Some(class) = provider {
        report(
            root,
            host,
            seq,
            json!({"kind": "provider_error", "turn": 1, "class": class}),
        );
        consumers(root, id);
        seq += 1;
    }
    let ended = json!({"kind": "turn_ended", "turn": 1, "cancelled": true});
    report(root, host, seq, ended);
    consumers(root, id);
}

/// A healthy bound host of the worker or of a pending review; its turn 1 has not reached corral.
fn setup(fx: &Fx, reviewer: bool) -> (BindTarget, Recipient, ts::Host, HostProc) {
    let host = HostProc::spawn(true);
    let (target, recipient, pane) = if reviewer {
        let target = f::reviewing(fx);
        fx.pending_review("w0:p2", jiff::Timestamp::now());
        let to = Recipient::ReviewRun {
            task: fx.id.clone(),
            run: "R1".into(),
            review: "R1-review1".into(),
        };
        (target, to, "w0:p2")
    } else {
        (fx.worker_target(), run_recipient(fx), "w0:p1")
    };
    let bound = fx.bind(&target, pane, &host);
    (target, recipient, bound, host)
}

fn case(reviewer: bool, named: bool, order: Order, provider: Option<&'static str>) {
    let case = format!("reviewer={reviewer} named={named} {order:?} provider={provider:?}");
    // Given a healthy host whose turn 1 has not reached corral yet:
    let fx = f::fixture(false, 5);
    let (target, recipient, bound, _host) = setup(&fx, reviewer);
    let container = target.container();
    let (root, reporter, id) = (fx.root().to_path_buf(), bound.clone(), fx.id.clone());
    let _bridge = f::bridge(
        &fx.ctx.store,
        &recipient,
        1,
        Some(f::CAP),
        move |frame: &Value| {
            if frame["command"] != "cancel" {
                return f::healthy(frame);
            }
            if matches!(order, Order::WhilePending) {
                cancelled_turn(&root, &reporter, &id, provider);
            }
            json!({"ok": true, "result": {"cancelled": true, "turn": 1}})
        },
    );
    let before = fx.task().status;

    // When the owner cancels it, the host cancels turn 1, and every consumer runs around the
    // host's reports of that turn:
    let target = AgentTarget::Task {
        task: fx.id.clone(),
        run: None,
        review: reviewer.then(|| "R1-review1".to_owned()),
    };
    let args = CancelArgs {
        target,
        turn: named.then_some(1),
    };
    cancel(&fx.ctx, &args).unwrap();
    if matches!(order, Order::AfterReply) {
        cancelled_turn(fx.root(), &bound, &fx.id, provider);
    }
    consumers(fx.root(), &fx.id);

    // Then turn 1 is cancelled, no wake was published, the nudge is unspent, no provider error
    // was retried or escalated, and the run or review stays open and unfenced:
    let after = state(&fx, &container);
    let turn = after.turns.iter().find(|t| t.seq == 1).unwrap();
    assert_eq!(turn.class, Some(TurnClass::Cancelled), "{case}");
    assert!(messages(&fx, &recipient, "turn_nudge").is_empty(), "{case}");
    let generation = after.generation(1).unwrap();
    assert_eq!(generation.nudged_at, None, "{case}");
    assert!(generation.fenced.is_none(), "{case}");
    assert_eq!(after.binding.health, HostHealth::Healthy, "{case}");
    for kind in [
        "provider_retry",
        "provider_escalated",
        "review_escalated",
        "review_interrupted",
    ] {
        assert!(fx.events(kind).is_empty(), "{case}: {kind}");
    }
    let task = fx.task();
    assert_eq!(task.status, before, "{case}");
    assert_eq!(agent::container_closure(&task, &container), None, "{case}");
}

#[test]
fn a_cancelled_turn_publishes_no_wake_and_starts_no_provider_recovery_in_any_report_order() {
    for reviewer in [false, true] {
        for named in [false, true] {
            for order in [Order::WhilePending, Order::AfterReply] {
                for provider in [None, Some("auth"), Some("overload")] {
                    case(reviewer, named, order, provider);
                }
            }
        }
    }
}

#[test]
fn a_cancelled_turn_that_committed_its_report_stays_completed_and_only_turn_ended_says_cancelled() {
    // Given a worker turn whose corral_done committed before the host cancelled it:
    let fx = f::fixture(false, 5);
    let (_, _, bound, _host) = setup(&fx, false);
    let origin = json!({"kind": "bridge", "idempotency_key": "k1"});
    report(
        fx.root(),
        &bound,
        1,
        json!({"kind": "turn_started", "turn": 1, "origin": origin}),
    );
    let mut done = super::tests::receipt(&bound, "c-done", "done", "committed", json!({}));
    done.identity.turn = Some(1);
    let update = |t: &mut Task| {
        let state = agent::container_state_mut(t, &fx.worker()).unwrap();
        state.receipts.push(done.clone());
        Ok(())
    };
    fx.ctx.store.update(&fx.id, update).unwrap();

    // When the host reports that turn ended cancelled:
    let ended = json!({"kind": "turn_ended", "turn": 1, "cancelled": true});
    report(fx.root(), &bound, 2, ended);

    // Then the committed report wins, as for corral's own mark:
    let turn = state(&fx, &fx.worker()).turns[0].clone();
    assert_eq!(turn.class, Some(TurnClass::Completed));
    assert_eq!(fx.task().status, Status::Running);

    // And another lifecycle event carrying `cancelled` is refused as a bad request:
    let id = crate::agent::event_request_id(bound.identity.generation, 3);
    let started = json!({"kind": "turn_started", "turn": 2, "cancelled": true});
    let input = ts::envelope(&bound, "event", &id, started, false);
    let refused = crate::agent::call(&fx.ctx, "event", &input).unwrap();
    assert_eq!(ts::code(&refused), Some("bad_request"));
}
