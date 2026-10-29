use corral_core::agent::{AgentRole, ContainerRef, Executor, ReceiptState, RequestReceipt};
use corral_core::config::agent_capabilities::Preset;
use corral_core::{PlanRunState, Verdict};
use serde_json::{Value, json};

use crate::agent::test_support::{self as ts, Host, committed, envelope, hello, send};
use crate::agent::{BindTarget, CallResponse, open_generation, tool_request_id};

const PLAN: &str = "---\nhuman_review:\n  problem: Callers cannot request a single widget rotation\n  \
changes: [Add one explicit spin operation]\n  retained: [Existing widget operations keep their behavior]\n\
goal: Give the widget one explicit spin operation\nnon_goals: [No scheduling changes]\n\
decisions: [Keep the current widget contract]\nacceptance:\n  - scenario: Call Widget.spin\n    \
expected: One clockwise rotation\naffected_paths: [src/widget.rs]\ncontracts: [Widget.spin]\n\
dependencies: []\nverification: [cargo test -p widget]\nowner_decisions: []\n---\n\
## Problem\nCallers cannot request a single widget rotation.\n\n## Goal\nGive the widget one explicit spin operation.\n\n\
## Included changes and retained behavior\nAdd one explicit spin operation. Existing widget operations keep their behavior.\n\n\
## Exclusions\nNo scheduling changes.\n\n## Choices\nKeep the current widget contract.\n\n\
## Done scenarios\nCalling Widget.spin produces one clockwise rotation.\n\n\
## Implementation\nAdd Widget.spin in src/widget.rs and cover it with a unit test.\n";

/// A task in planning with native planner run D1 whose pane is p1.
fn planning_task(fx: &ts::Fx) -> String {
    let id = ts::worker_task(fx, "p1");
    let socket = fx.ctx.herdr.socket_path().unwrap();
    let output = fx.ctx.store.state_dir().join("plans").join(&id).join("D1");
    std::fs::create_dir_all(&output).unwrap();
    fx.ctx
        .store
        .update(&id, |task| {
            task.runs.clear();
            let revision = crate::plan_spec_revision(task)?;
            task.plan_runs.push(serde_json::from_value(json!({
                "id":"D1", "profile":"pi", "harness":"pi-native", "delivery":"inbox",
                "spec_revision":revision, "started":"2026-10-29T00:00:00Z",
                "output_dir":output, "rounds":0, "state":"planning",
                "herdr":{"workspace_id":"w1","pane_id":"p1","socket":socket}
            }))?);
            Ok(())
        })
        .unwrap();
    id
}

fn planner(fx: &ts::Fx, id: &str, session: &str) -> Host {
    let target = BindTarget::Planner {
        task: id.into(),
        run: "D1".into(),
    };
    let host = ts::bind(fx, &target, "p1", session);
    committed(&hello(fx, &host));
    host
}

fn op(fx: &ts::Fx, host: &Host, op: &str, call: &str, payload: Value) -> CallResponse {
    let request = tool_request_id(&host.identity.session_id, call);
    send(fx, &envelope(host, op, &request, payload, false), op)
}

fn receipt(response: &CallResponse) -> RequestReceipt {
    response
        .receipt()
        .cloned()
        .unwrap_or_else(|| panic!("expected a receipt, got {response:?}"))
}

/// A submitted P1 with a pending native review PR1 under the current reviewer policy.
fn reviewing_task(fx: &ts::Fx) -> (String, Host) {
    let id = planning_task(fx);
    let host = planner(fx, &id, "s-plan");
    committed(&op(fx, &host, "plan_write", "w", json!({"content": PLAN})));
    committed(&op(fx, &host, "plan_submit", "s", json!({})));
    let socket = fx.ctx.herdr.socket_path().unwrap();
    let config = crate::plan::config(&fx.ctx).unwrap();
    fx.ctx
        .store
        .update(&id, |task| {
            let digest = crate::plan::policy_digest(&config, task)?;
            task.plans[0].reviews.push(serde_json::from_value(json!({
                "id":"PR1", "reviewer":"pi", "policy_digest":digest, "delivery":"inbox",
                "at":"2026-10-29T00:00:00Z",
                "herdr":{"workspace_id":"w2","pane_id":"p2","socket":socket}
            }))?);
            task.plan_runs[0].state = PlanRunState::Reviewing;
            Ok(())
        })
        .unwrap();
    let target = BindTarget::PlanReviewer {
        task: id.clone(),
        plan: "P1".into(),
        review: "PR1".into(),
    };
    let reviewer = ts::bind(fx, &target, "p2", "s-review");
    committed(&hello(fx, &reviewer));
    (id, reviewer)
}

#[test]
fn planner_writes_only_its_bound_plan_file_and_submits_it_once() {
    // Given a bound native planner.
    let fx = ts::fx();
    let id = planning_task(&fx);
    let host = planner(&fx, &id, "s-plan");
    let path = fx
        .ctx
        .store
        .state_dir()
        .join("plans")
        .join(&id)
        .join("D1/plan.md");
    let (head, tail) = PLAN.split_at(200);
    // When it writes the plan in two bounded chunks and retries the first.
    let first = committed(&op(
        &fx,
        &host,
        "plan_write",
        "w1",
        json!({"content": head}),
    ));
    let retry = op(&fx, &host, "plan_write", "w1", json!({"content": head}));
    let second = op(
        &fx,
        &host,
        "plan_write",
        "w2",
        json!({"content": tail, "offset": 200}),
    );
    let gap = op(
        &fx,
        &host,
        "plan_write",
        "w3",
        json!({"content": "x", "offset": 9999}),
    );
    // Then the launcher-selected file holds exactly the plan and the retry replays.
    assert_eq!(first.result["path"], json!(path));
    assert!(matches!(
        retry,
        CallResponse::Receipt { replayed: true, .. }
    ));
    assert_eq!(committed(&second).result["bytes"], json!(PLAN.len()));
    assert_eq!(ts::code(&gap), Some("bad_request"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), PLAN);

    // When it submits, and the same request is retried.
    let submitted = committed(&op(&fx, &host, "plan_submit", "s1", json!({})));
    let again = op(&fx, &host, "plan_submit", "s1", json!({}));
    // Then the existing plan service snapshot P1 is bound to run D1 exactly once.
    assert_eq!(submitted.result["outcome"], "submitted");
    assert_eq!(submitted.result["plan"], "P1");
    assert_eq!(receipt(&again), submitted);
    let task = fx.ctx.store.get(&id).unwrap();
    assert_eq!(task.plans.len(), 1);
    assert_eq!(task.plans[0].run.as_deref(), Some("D1"));
    assert_eq!(task.plan_runs[0].state, PlanRunState::Submitted);
    assert!(task.plans[0].approval.is_none());
    // And the ended run accepts no further output.
    let late = op(&fx, &host, "plan_write", "w4", json!({"content": "late"}));
    assert_eq!(ts::code(&late), Some("run_ended"));
}

#[test]
fn a_refused_submission_settles_without_a_plan_and_the_planner_can_fix_it() {
    // Given a planner whose written plan is not a valid plan document.
    let fx = ts::fx();
    let id = planning_task(&fx);
    let host = planner(&fx, &id, "s-plan");
    let empty = op(&fx, &host, "plan_submit", "s0", json!({}));
    committed(&op(
        &fx,
        &host,
        "plan_write",
        "w1",
        json!({"content": "not a plan"}),
    ));
    // When it submits.
    let refused = receipt(&op(&fx, &host, "plan_submit", "s1", json!({})));
    // Then nothing is submitted and the receipt says why, without claiming an effect.
    assert_eq!(ts::code(&empty), Some("bad_request"));
    assert_eq!(refused.state, ReceiptState::Abandoned);
    assert_eq!(refused.result["outcome"], "refused");
    assert!(refused.executor.is_none());
    let task = fx.ctx.store.get(&id).unwrap();
    assert!(task.plans.is_empty());
    assert_eq!(task.plan_runs[0].state, PlanRunState::Planning);
    // And a corrected plan submits on a new request.
    committed(&op(
        &fx,
        &host,
        "plan_write",
        "w2",
        json!({"content": PLAN}),
    ));
    assert_eq!(
        committed(&op(&fx, &host, "plan_submit", "s2", json!({}))).result["plan"],
        "P1"
    );
}

#[test]
fn plan_reviewer_verdicts_use_the_bound_attempt_and_never_approve_for_the_owner() {
    for verdict in [Verdict::Approve, Verdict::Changes] {
        // Given a bound native plan reviewer of P1/PR1.
        let fx = ts::fx();
        let (id, reviewer) = reviewing_task(&fx);
        let wording = format!("{verdict:?}: the plan names its verification.");
        // When it records the verdict with its notes, then retries the request.
        let payload = json!({"verdict": verdict, "notes": wording});
        let recorded = committed(&op(&fx, &reviewer, "plan_verdict", "v1", payload.clone()));
        let again = op(&fx, &reviewer, "plan_verdict", "v1", payload);
        // Then the review attempt carries it, the notes report holds the notes, and no
        // owner approval exists: an approval can at most await the owner.
        assert_eq!(recorded.result["outcome"], "recorded");
        assert_eq!(receipt(&again), recorded);
        let task = fx.ctx.store.get(&id).unwrap();
        let review = &task.plans[0].reviews[0];
        assert!(review.notes.as_deref().unwrap().starts_with(&wording));
        assert_eq!(json!(review.verdict), recorded.result["verdict"]);
        assert!(task.plans[0].approval.is_none());
        assert_eq!(task.plans[0].reviews.len(), 1);
        let notes = std::fs::read_to_string(recorded_notes(&fx, &id)).unwrap();
        assert_eq!(notes, wording);
        if verdict == Verdict::Changes {
            assert_eq!(task.plan_runs[0].state, PlanRunState::Changes);
        } else {
            assert_ne!(task.plan_runs[0].state, PlanRunState::Approved);
        }
    }
}

fn recorded_notes(fx: &ts::Fx, id: &str) -> std::path::PathBuf {
    fx.ctx
        .store
        .state_dir()
        .join("reviews")
        .join(format!("{id}-P1-PR1-plan-notes.md"))
}

#[test]
fn planning_ops_are_role_bound_and_hello_advertises_them() {
    // Given a bound planner and plan reviewer.
    let fx = ts::fx();
    let (_id, reviewer) = reviewing_task(&fx);
    let fx2 = ts::fx();
    let id2 = planning_task(&fx2);
    let target = BindTarget::Planner {
        task: id2,
        run: "D1".into(),
    };
    let host = ts::bind(&fx2, &target, "p1", "s-ops");
    // When each says hello and calls the other role's operation.
    let ops = committed(&hello(&fx2, &host)).result["ops"].clone();
    let wrong_verdict = op(
        &fx2,
        &host,
        "plan_verdict",
        "v",
        json!({"verdict":"approve","notes":"x"}),
    );
    let wrong_submit = op(&fx, &reviewer, "plan_submit", "s", json!({}));
    // Then each sees only its own planning operations, plus the reused question ops.
    for name in ["ask", "withdraw", "plan_write", "plan_submit"] {
        assert!(ops.as_array().unwrap().contains(&json!(name)), "{name}");
    }
    assert!(!ops.as_array().unwrap().contains(&json!("plan_verdict")));
    assert_eq!(ts::code(&wrong_verdict), Some("op_not_allowed"));
    assert_eq!(ts::code(&wrong_submit), Some("op_not_allowed"));
}

#[test]
fn a_disabled_planning_role_refuses_its_launch_instead_of_falling_back() {
    for (strict, role_override) in [(true, None), (false, Some(false)), (true, Some(true))] {
        // Given native planning configured strict, or with role_enabled overridden.
        let mut fx = ts::fx();
        let id = planning_task(&fx);
        let capabilities = &mut fx.ctx.store.config.agent.capabilities;
        if strict {
            capabilities.set_preset(Preset::Strict);
        }
        if let Some(enabled) = role_override {
            capabilities
                .set(AgentRole::Planner, "role_enabled", enabled)
                .unwrap();
        }
        // When the planner's generation opens.
        let target = BindTarget::Planner {
            task: id.clone(),
            run: "D1".into(),
        };
        let opened = open_generation(&fx.ctx.store, &target, "p1");
        // Then a disabled role refuses before any binding is recorded; an enabled one binds.
        let task = fx.ctx.store.get(&id).unwrap();
        if role_override == Some(true) {
            let grant = opened.unwrap();
            assert!(grant.capabilities.enabled("role_enabled"));
        } else {
            let error = format!("{:#}", opened.unwrap_err());
            assert!(error.starts_with("role_disabled:"), "{error}");
            assert!(task.plan_runs[0].agent_host.is_none());
        }
        // And other roles have no such switch.
        assert!(
            fx.ctx
                .store
                .config
                .agent
                .capabilities
                .set(AgentRole::Worker, "role_enabled", false)
                .is_err()
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_reservation_whose_executor_died_settles_from_the_plan_record() {
    for submitted in [true, false] {
        // Given a planner reservation whose executor is verified gone, e.g. killed when the
        // submission closed its own pane.
        let fx = ts::fx();
        let id = planning_task(&fx);
        let host = planner(&fx, &id, "s-plan");
        committed(&op(&fx, &host, "plan_write", "w", json!({"content": PLAN})));
        if submitted {
            let path = fx
                .ctx
                .store
                .state_dir()
                .join("plans")
                .join(&id)
                .join("D1/plan.md");
            crate::plan::plan_submit(&fx.ctx, &id, &path).unwrap();
        }
        let container = ContainerRef::Planner {
            task: id.clone(),
            run: "D1".into(),
        };
        let mut reserved: RequestReceipt = serde_json::from_value(json!({
            "version": 1, "request_id": tool_request_id("s-plan", "lost"), "op": "plan_submit",
            "payload_sha256": "00", "identity": host.identity, "state": "reserved",
            "result": {"outcome": "reserved", "plans_before": 0}, "at": "2026-10-29T00:00:00Z",
        }))
        .unwrap();
        reserved.executor = Some(Executor {
            pid: u32::MAX,
            start_time: "0".into(),
        });
        fx.ctx
            .store
            .update(&id, |task| {
                corral_core::agent::container_state_mut(task, &container)
                    .unwrap()
                    .receipts
                    .push(reserved.clone());
                Ok(())
            })
            .unwrap();
        // When sync reconciles the task.
        crate::agent::effects::reconcile_task(&fx.ctx, &id).unwrap();
        // Then the receipt records exactly what the plan record shows.
        let state = ts::state(&fx, &container);
        let settled = state.receipt(&reserved.request_id).unwrap();
        assert!(settled.executor.is_none());
        if submitted {
            assert_eq!(settled.state, ReceiptState::Committed);
            assert_eq!(settled.result["plan"], "P1");
        } else {
            assert_eq!(settled.state, ReceiptState::Abandoned);
        }
    }
}
