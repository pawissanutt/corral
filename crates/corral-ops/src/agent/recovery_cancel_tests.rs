//! T-252 recovery: `corral agent cancel` binds to the turn the host cancels though the host's
//! own lifecycle reports of that turn commit after the cancel's mark (before its reply) or after
//! the cancel returned, for a worker and a reviewer, with the turn named or not. The bridge
//! commits the reports from inside its cancel handler, or the test commits them afterwards, so
//! the ordering is fixed by construction, never by timing.
use corral_core::agent::TurnClass;
use serde_json::{Value, json};

use super::tests::{messages, run_recipient, state, sync};
use super::*;
use crate::agent::bridge::fixture::{self as f, HostProc};
use crate::agent::test_support as ts;

#[derive(Clone, Copy, Debug)]
enum Reports {
    /// Committed after the cancel's mark, before the host's reply.
    BeforeReply,
    /// Committed after the cancel returned.
    AfterReply,
}

fn report(root: &Path, host: &ts::Host, seq: u64, payload: Value) {
    let ctx = f::ctx_at(root);
    let id = crate::agent::event_request_id(host.identity.generation, seq);
    let input = ts::envelope(host, "event", &id, payload, false);
    ts::committed(&crate::agent::call(&ctx, "event", &input).unwrap());
}

/// The host's reports of its turn 1, started from a bridge input and ended without a report.
fn turn_reports(root: &Path, host: &ts::Host) {
    let origin = json!({"kind": "bridge", "idempotency_key": "k1"});
    report(
        root,
        host,
        1,
        json!({"kind": "turn_started", "turn": 1, "origin": origin}),
    );
    report(root, host, 2, json!({"kind": "turn_ended", "turn": 1}));
}

fn case(reviewer: bool, named: bool, reports: Reports) {
    let case = format!("reviewer={reviewer} named={named} {reports:?}");
    // Given a healthy host whose turn 1 has not reached corral yet:
    let fx = f::fixture(false, 5);
    let host = HostProc::spawn(true);
    let (target, recipient, pane) = if reviewer {
        let target = f::reviewing(&fx);
        fx.pending_review("w0:p2", jiff::Timestamp::now());
        let to = Recipient::ReviewRun {
            task: fx.id.clone(),
            run: "R1".into(),
            review: "R1-review1".into(),
        };
        (target, to, "w0:p2")
    } else {
        (fx.worker_target(), run_recipient(&fx), "w0:p1")
    };
    let bound = fx.bind(&target, pane, &host);
    let container = target.container();
    let (root, reporter) = (fx.root().to_path_buf(), bound.clone());
    let _bridge = f::bridge(
        &fx.ctx.store,
        &recipient,
        1,
        Some(f::CAP),
        move |frame: &Value| {
            if frame["command"] != "cancel" {
                return f::healthy(frame);
            }
            if matches!(reports, Reports::BeforeReply) {
                turn_reports(&root, &reporter);
            }
            json!({"ok": true, "result": {"cancelled": true, "turn": 1}})
        },
    );

    // When the owner cancels it and the host confirms cancelling turn 1:
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
    if matches!(reports, Reports::AfterReply) {
        turn_reports(fx.root(), &bound);
    }
    sync(&fx);

    // Then turn 1 is classified cancelled, gets no nudge, and nothing was fenced:
    let after = state(&fx, &container);
    let turn = after
        .turns
        .iter()
        .find(|t| t.generation == 1 && t.seq == 1)
        .unwrap();
    assert_eq!(turn.class, Some(TurnClass::Cancelled), "{case}");
    assert!(messages(&fx, &recipient, "turn_nudge").is_empty(), "{case}");
    assert!(after.generation(1).unwrap().fenced.is_none(), "{case}");
    assert_eq!(after.binding.health, HostHealth::Healthy, "{case}");
    sync(&fx);
    let settled = state(&fx, &container).turns[0].class;
    assert_eq!(settled, Some(TurnClass::Cancelled), "{case}: settled once");
}

#[test]
fn a_cancel_binds_to_the_turn_the_host_cancels_though_its_reports_arrive_late() {
    for reviewer in [false, true] {
        for named in [false, true] {
            for reports in [Reports::BeforeReply, Reports::AfterReply] {
                case(reviewer, named, reports);
            }
        }
    }
}
