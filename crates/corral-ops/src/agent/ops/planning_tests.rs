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
        // Inline notes never touch the notes report: only the receipted plan_write does.
        assert!(!recorded_notes(&fx, &id).exists());
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

#[test]
fn a_verdict_without_inline_notes_records_the_written_report() {
    // Given a plan reviewer whose notes report was written through plan_write.
    let fx = ts::fx();
    let (id, reviewer) = reviewing_task(&fx);
    let report = "Changes: name the regression test.";
    committed(&op(
        &fx,
        &reviewer,
        "plan_write",
        "n",
        json!({"content": report}),
    ));
    // When it records a verdict without inline notes.
    let recorded = committed(&op(
        &fx,
        &reviewer,
        "plan_verdict",
        "v",
        json!({"verdict": "changes"}),
    ));
    // Then the review carries the report's text.
    assert_eq!(recorded.result["outcome"], "recorded");
    let task = fx.ctx.store.get(&id).unwrap();
    assert!(
        task.plans[0].reviews[0]
            .notes
            .as_deref()
            .unwrap()
            .starts_with(report)
    );
}

/// Runs `at_barrier` once, when the planning op reaches its reservation-to-effect barrier.
fn at_barrier(mut at_barrier: impl FnMut() + 'static) {
    crate::agent::FAILPOINT.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |point: &str| {
            if point == "plan_reserved_before_effect" {
                at_barrier();
            }
        }));
    });
}

fn clear_barrier() {
    crate::agent::FAILPOINT.with(|hook| hook.borrow_mut().take());
}

#[test]
fn a_planner_fenced_or_replaced_after_reserving_never_submits() {
    for replace in [false, true] {
        // Given a planner whose plan is written and whose submission has reserved its receipt.
        let fx = ts::fx();
        let id = planning_task(&fx);
        let host = planner(&fx, &id, "s-plan");
        committed(&op(&fx, &host, "plan_write", "w", json!({"content": PLAN})));
        let store = fx.ctx.store.reopen().unwrap();
        let task_id = id.clone();
        // When, at the barrier, a new generation fences it, or D1 is cancelled for a new D2.
        at_barrier(move || {
            let container = ContainerRef::Planner {
                task: task_id.clone(),
                run: "D1".into(),
            };
            if replace {
                store
                    .update(&task_id, |task| {
                        let mut next = task.plan_runs[0].clone();
                        task.plan_runs[0].state = PlanRunState::Cancelled;
                        task.plan_runs[0].ended = Some(jiff::Timestamp::now());
                        next.id = "D2".into();
                        next.agent_host = None;
                        task.plan_runs.push(next);
                        Ok(())
                    })
                    .unwrap();
            } else {
                assert!(crate::agent::fence_generation(&store, &container, 1, "replaced").unwrap());
            }
        });
        let settled = receipt(&op(&fx, &host, "plan_submit", "s", json!({})));
        clear_barrier();
        // Then the service's own transaction refuses it: no plan, D2 untouched.
        assert_eq!(settled.state, ReceiptState::Abandoned, "replace={replace}");
        let task = fx.ctx.store.get(&id).unwrap();
        assert!(task.plans.is_empty(), "replace={replace}");
        assert!(task.plan_runs.iter().all(|run| run.submitted.is_none()));
    }
}

#[test]
fn a_plan_reviewer_fenced_after_reserving_records_no_verdict_and_no_report() {
    // Given a bound plan reviewer whose verdict reaches its reservation.
    let fx = ts::fx();
    let (id, reviewer) = reviewing_task(&fx);
    let store = fx.ctx.store.reopen().unwrap();
    let (task_id, notes) = (id.clone(), recorded_notes(&fx, &id));
    let seen = std::rc::Rc::new(std::cell::RefCell::new(None));
    let observed = seen.clone();
    // When, at the barrier, its generation is fenced.
    at_barrier(move || {
        let container = ContainerRef::PlanReviewer {
            task: task_id.clone(),
            plan: "P1".into(),
            review: "PR1".into(),
        };
        let state = corral_core::agent::container_state(&store.get(&task_id).unwrap(), &container)
            .unwrap()
            .clone();
        let reserved = state
            .receipts
            .iter()
            .any(|r| r.op == "plan_verdict" && r.state == ReceiptState::Reserved);
        *observed.borrow_mut() = Some((reserved, notes.exists()));
        assert!(crate::agent::fence_generation(&store, &container, 1, "replaced").unwrap());
    });
    let payload = json!({"verdict": "approve", "notes": "Looks complete."});
    let settled = receipt(&op(&fx, &reviewer, "plan_verdict", "v", payload));
    clear_barrier();
    // Then the reservation was durable before any effect, nothing touched the report, and the
    // verdict service refused the fenced generation.
    assert_eq!(*seen.borrow(), Some((true, false)));
    assert_eq!(settled.state, ReceiptState::Abandoned);
    let task = fx.ctx.store.get(&id).unwrap();
    assert!(task.plans[0].reviews[0].verdict.is_none());
    assert!(task.plans[0].approval.is_none());
}

/// Pushes a reserved receipt of `op` whose executor is verified gone into `container`.
fn dead_reservation(
    fx: &ts::Fx,
    host: &Host,
    container: &ContainerRef,
    op: &str,
    result: Value,
) -> String {
    let id = container.task().unwrap();
    let mut reserved: RequestReceipt = serde_json::from_value(json!({
        "version": 1, "request_id": tool_request_id(&host.identity.session_id, "lost"), "op": op,
        "payload_sha256": "00", "identity": host.identity, "state": "reserved",
        "result": result, "at": "2026-10-29T00:00:00Z",
    }))
    .unwrap();
    reserved.executor = Some(Executor {
        pid: u32::MAX,
        start_time: "0".into(),
    });
    let request = reserved.request_id.clone();
    fx.ctx
        .store
        .update(id, |task| {
            corral_core::agent::container_state_mut(task, container)
                .unwrap()
                .receipts
                .push(reserved);
            Ok(())
        })
        .unwrap();
    request
}

#[cfg(target_os = "linux")]
#[test]
fn normal_sync_settles_planning_reservations_and_closes_their_questions() {
    // Given a planning-only task: a planner whose submission committed but whose executor died
    // with its pane, and an open question that the closed run left pending.
    let fx = ts::fx();
    let id = planning_task(&fx);
    let host = planner(&fx, &id, "s-plan");
    let question = json!({"kind": "clarification", "text": "Which widget?"});
    let asked = committed(&op(&fx, &host, "ask", "q", question));
    committed(&op(&fx, &host, "plan_write", "w", json!({"content": PLAN})));
    let path = fx
        .ctx
        .store
        .state_dir()
        .join("plans")
        .join(&id)
        .join("D1/plan.md");
    crate::plan::plan_submit(&fx.ctx, &id, &path).unwrap();
    let planner = ContainerRef::Planner {
        task: id.clone(),
        run: "D1".into(),
    };
    let lost = dead_reservation(
        &fx,
        &host,
        &planner,
        "plan_submit",
        json!({"plans_before": 0}),
    );
    assert!(crate::agent::hooks::sync::wants(
        &fx.ctx.store.get(&id).unwrap()
    ));
    // When ordinary sync runs.
    crate::sync_at(&fx.ctx, jiff::Timestamp::now()).unwrap();
    // Then the reservation settles from the plan record and the question is terminalized.
    let state = ts::state(&fx, &planner);
    assert_eq!(state.receipt(&lost).unwrap().state, ReceiptState::Committed);
    let key = asked.result["key"].as_str().unwrap();
    let q = state.questions.iter().find(|q| q.key == key).unwrap();
    assert!(!q.recorded_pending());
    // And a settled planning-only task no longer asks sync to visit it.
    assert!(!crate::agent::hooks::sync::wants(
        &fx.ctx.store.get(&id).unwrap()
    ));
}

#[cfg(target_os = "linux")]
#[test]
fn normal_sync_abandons_a_dead_plan_reviewer_reservation_that_recorded_nothing() {
    // Given a plan reviewer reservation whose executor died before the verdict committed.
    let fx = ts::fx();
    let (id, reviewer) = reviewing_task(&fx);
    let container = ContainerRef::PlanReviewer {
        task: id.clone(),
        plan: "P1".into(),
        review: "PR1".into(),
    };
    let detail = json!({"plan": "P1", "review": "PR1", "notes_bytes": 3, "notes_sha256": "00"});
    let lost = dead_reservation(&fx, &reviewer, &container, "plan_verdict", detail);
    // When ordinary sync runs.
    crate::sync_at(&fx.ctx, jiff::Timestamp::now()).unwrap();
    // Then the receipt records that this request recorded nothing, even if sync's own plan-review
    // reconciliation closed the attempt meanwhile, and no approval exists.
    let state = ts::state(&fx, &container);
    assert_eq!(state.receipt(&lost).unwrap().state, ReceiptState::Abandoned);
    assert!(fx.ctx.store.get(&id).unwrap().plans[0].approval.is_none());
}

/// One pass of the per-task hook ordinary sync runs for Pi-native work, behind its own
/// `wants` predicate. The full sync loop also observes the herdr engine, absent here.
fn sync_agent(fx: &ts::Fx, id: &str, now: jiff::Timestamp) {
    let task = fx.ctx.store.get(id).unwrap();
    assert!(crate::agent::hooks::sync::wants(&task));
    crate::agent::hooks::sync::task(&fx.ctx, &task, now).unwrap();
}

fn question_key(response: &CallResponse) -> String {
    committed(response).result["key"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn owner_answers(fx: &ts::Fx, key: &str) {
    let revision = fx_question(fx, key).revision;
    let input = crate::questions::AnswerInput {
        text: "Use the round widget.".into(),
        ..Default::default()
    };
    crate::questions::answer(
        &fx.ctx,
        key,
        revision,
        crate::questions::Actor::Owner,
        input,
    )
    .unwrap();
}

fn fx_question(fx: &ts::Fx, key: &str) -> corral_core::agent::AgentQuestion {
    let task = fx.ctx.store.get(key.split('/').next().unwrap()).unwrap();
    crate::questions::views(&task)
        .into_iter()
        .find(|view| view.question.key == key)
        .unwrap()
        .question
}

fn past_unattended(fx: &ts::Fx) -> jiff::Timestamp {
    let secs = fx.ctx.store.config.agent.question_unattended_secs as i64 + 1;
    jiff::Timestamp::now() + jiff::SignedDuration::from_secs(secs)
}

fn hold(fx: &ts::Fx, container: &ContainerRef) -> Option<String> {
    ts::state(fx, container).question_hold
}

#[test]
fn a_planner_question_unattended_past_its_deadline_holds_needs_me_until_the_owner_answers() {
    // Given a bound planner on a Running planning task with a blocking question.
    let fx = ts::fx();
    let id = planning_task(&fx);
    let host = planner(&fx, &id, "s-plan");
    let planner = ContainerRef::Planner {
        task: id.clone(),
        run: "D1".into(),
    };
    let ask = json!({"kind": "clarification", "text": "Which widget?", "blocking": true});
    let key = question_key(&op(&fx, &host, "ask", "q", ask));
    // When ordinary sync runs after the unattended deadline.
    let later = past_unattended(&fx);
    sync_agent(&fx, &id, later);
    // Then the task enters Needs me on the planner's own hold, and the planner stays live.
    let task = fx.ctx.store.get(&id).unwrap();
    assert_eq!(task.status, corral_core::Status::NeedsMe);
    assert!(hold(&fx, &planner).unwrap().contains(&key));
    assert!(corral_core::agent::container_closure(&task, &planner).is_none());
    // And the planner pane reporting Working on the next pass does not clear that hold.
    let pane = corral_herdr::PaneInfo {
        pane_id: "p1".into(),
        workspace_id: "w1".into(),
        tab_id: "t1".into(),
        agent: Some("pi".into()),
        status: corral_herdr::AgentStatus::Working,
        title: None,
        cwd: None,
    };
    let panes = std::collections::HashMap::from([("p1".to_owned(), pane)]);
    let snapshot = crate::pane_liveness::Snapshot::injected(&panes);
    crate::plan_pilot::reconcile(&fx.ctx, &task, snapshot, later).unwrap();
    assert_eq!(
        fx.ctx.store.get(&id).unwrap().status,
        corral_core::Status::NeedsMe
    );
    // When the owner answers and sync runs again.
    owner_answers(&fx, &key);
    sync_agent(&fx, &id, later);
    // Then exactly the hold the question raised is cleared and the planner keeps working.
    assert_eq!(
        fx.ctx.store.get(&id).unwrap().status,
        corral_core::Status::Running
    );
    assert!(hold(&fx, &planner).is_none());
    committed(&op(&fx, &host, "plan_write", "w", json!({"content": PLAN})));
}

#[test]
fn a_plan_reviewer_unattended_question_holds_needs_me_until_it_is_withdrawn() {
    // Given a bound plan reviewer with a blocking question.
    let fx = ts::fx();
    let (id, reviewer) = reviewing_task(&fx);
    let container = ContainerRef::PlanReviewer {
        task: id.clone(),
        plan: "P1".into(),
        review: "PR1".into(),
    };
    let ask =
        json!({"kind": "clarification", "text": "May the plan drop the CLI?", "blocking": true});
    let key = question_key(&op(&fx, &reviewer, "ask", "q", ask));
    // When ordinary sync runs after the unattended deadline.
    sync_agent(&fx, &id, past_unattended(&fx));
    // Then the task waits on the owner through the reviewer's own hold.
    let task = fx.ctx.store.get(&id).unwrap();
    assert_eq!(task.status, corral_core::Status::NeedsMe);
    assert!(hold(&fx, &container).unwrap().contains(&key));
    // When the reviewer withdraws its question.
    let withdraw = json!({"key": key, "reason": "The plan answers it."});
    committed(&op(&fx, &reviewer, "withdraw", "wd", withdraw));
    // Then the task returns to Review and the reviewer can still record its verdict.
    assert_eq!(
        fx.ctx.store.get(&id).unwrap().status,
        corral_core::Status::Review
    );
    assert!(hold(&fx, &container).is_none());
    let verdict = json!({"verdict": "changes", "notes": "Name the CLI test."});
    assert_eq!(
        committed(&op(&fx, &reviewer, "plan_verdict", "v", verdict)).result["outcome"],
        "recorded"
    );
}

#[test]
fn a_planning_question_never_clears_a_needs_me_hold_it_did_not_raise() {
    // Given a planning task already in Needs me for another reason, and a planner question.
    let fx = ts::fx();
    let id = planning_task(&fx);
    let host = planner(&fx, &id, "s-plan");
    let planner = ContainerRef::Planner {
        task: id.clone(),
        run: "D1".into(),
    };
    let ask = json!({"kind": "clarification", "text": "Which widget?", "blocking": true});
    let key = question_key(&op(&fx, &host, "ask", "q", ask));
    fx.ctx
        .store
        .update(&id, |task| {
            task.status = corral_core::Status::NeedsMe;
            Ok(())
        })
        .unwrap();
    // When sync runs past the unattended deadline and the planner then withdraws its question.
    sync_agent(&fx, &id, past_unattended(&fx));
    assert!(hold(&fx, &planner).is_none());
    let withdraw = json!({"key": key, "reason": "Resolved in the plan."});
    committed(&op(&fx, &host, "withdraw", "wd", withdraw));
    // Then the unrelated hold stays in place.
    assert_eq!(
        fx.ctx.store.get(&id).unwrap().status,
        corral_core::Status::NeedsMe
    );
}
