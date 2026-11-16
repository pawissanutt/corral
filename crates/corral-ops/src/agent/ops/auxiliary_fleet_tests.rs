use crate::agent::test_support::{self as ts, Fx, Host};
use crate::agent::{BindTarget, CallResponse};
use corral_core::agent::{
    self, AgentRole, AuxiliaryOutcome, NativeRecord, UsageSessionKind, paths,
};
use serde_json::json;

fn event(
    fx: &Fx,
    host: &Host,
    seq: u64,
    kind: &str,
    action: &str,
    request: &str,
    outcome: Option<&str>,
) -> CallResponse {
    let mut payload =
        json!({"kind":"auxiliary", "usage_kind":kind, "action":action, "request":request});
    if action == "prepare" {
        payload["model"] = json!("openai-codex/gpt-5-codex");
    }
    if let Some(outcome) = outcome {
        payload["outcome"] = json!(outcome);
    }
    ts::send(
        fx,
        &ts::envelope(host, "event", &format!("ev-1-{seq}"), payload, false),
        "event",
    )
}

fn fleet(fx: &Fx, target: &BindTarget, role: AgentRole, enabled: bool) {
    let config: corral_core::agent_capabilities::AgentCapabilities =
        serde_json::from_value(json!({
            "worker":{"fleet":{"enabled":enabled,"max_parallel":1}},
            "assistant":{"fleet":{"enabled":enabled,"max_parallel":1}}
        }))
        .unwrap();
    let snapshot = config.resolve(role, None);
    let container = target.container();
    if let Some(session) = container.assistant() {
        fx.ctx
            .store
            .settle_assistant_container(session, |assistant, _| {
                assistant.state.generations[0].capabilities = Some(snapshot);
                Ok(())
            })
            .unwrap();
    } else {
        fx.ctx
            .store
            .update(container.task().unwrap(), |task| {
                agent::container_state_mut(task, &container)
                    .unwrap()
                    .generations[0]
                    .capabilities = Some(snapshot);
                Ok(())
            })
            .unwrap();
    }
}

fn worker(fx: &Fx) -> (BindTarget, Host) {
    let task = ts::worker_task(fx, "p1");
    fx.ctx
        .store
        .update(&task, |task| {
            task.runs[0].harness = Some("corral-pi".into());
            Ok(())
        })
        .unwrap();
    let target = BindTarget::Worker {
        task,
        run: "R1".into(),
    };
    let host = ts::bind(fx, &target, "p1", "fleet-worker");
    ts::committed(&ts::hello(fx, &host));
    (target, host)
}

#[test]
fn fleet_children_share_the_parallel_limit_replay_references_and_keep_possible_spend_on_the_run() {
    let fx = ts::fx();
    let (target, host) = worker(&fx);
    fleet(&fx, &target, AgentRole::Worker, true);
    let slice = ts::committed(&event(&fx, &host, 1, "slice", "prepare", "slice", None));
    let replay = event(&fx, &host, 1, "slice", "prepare", "slice", None);
    assert!(matches!(
        &replay,
        CallResponse::Receipt { replayed: true, .. }
    ));
    assert_eq!(replay.receipt().unwrap().result, slice.result);
    let review = ts::committed(&event(&fx, &host, 2, "review", "prepare", "review", None));
    for (request, receipt) in [("slice", &slice), ("review", &review)] {
        assert_eq!(receipt.result["session_id"], request);
        assert_eq!(
            receipt.result["session_file"],
            paths::usage_session_file(&fx.ctx.store, &target.container(), request)
                .to_str()
                .unwrap()
        );
    }
    ts::committed(&event(&fx, &host, 3, "slice", "started", "slice", None));
    let blocked = event(&fx, &host, 4, "review", "started", "review", None);
    assert_eq!(ts::code(&blocked), Some("bad_request"));
    ts::committed(&event(
        &fx,
        &host,
        5,
        "slice",
        "finish",
        "slice",
        Some("completed"),
    ));
    ts::committed(&event(&fx, &host, 6, "review", "started", "review", None));
    // Prepare is the capability admission boundary, not settlement of already-possible spend.
    fleet(&fx, &target, AgentRole::Worker, false);
    ts::committed(&event(
        &fx,
        &host,
        7,
        "review",
        "finish",
        "review",
        Some("uncertain"),
    ));
    fleet(&fx, &target, AgentRole::Worker, true);
    ts::committed(&event(&fx, &host, 8, "slice", "prepare", "prepared", None));
    ts::committed(&event(&fx, &host, 9, "review", "prepare", "running", None));
    ts::committed(&event(&fx, &host, 10, "review", "started", "running", None));
    let saved = ts::state(&fx, &target.container());
    assert_eq!(saved.usage_sessions.len(), 4);
    for (record, (kind, outcome)) in saved.usage_sessions.iter().zip([
        (UsageSessionKind::Slice, AuxiliaryOutcome::Completed),
        (UsageSessionKind::Review, AuxiliaryOutcome::Uncertain),
        (UsageSessionKind::Slice, AuxiliaryOutcome::Prepared),
        (UsageSessionKind::Review, AuxiliaryOutcome::Running),
    ]) {
        assert_eq!(record.kind, kind);
        assert_eq!(record.outcome.as_ref(), Some(&outcome));
        let kind_name = match kind {
            UsageSessionKind::Slice => "slice",
            _ => "review",
        };
        assert_eq!(
            NativeRecord::UsageSession(record.clone()).token(),
            format!("session:1:{kind_name}:{}", record.session_id)
        );
        let response = json!({"type":"message", "id":"response", "timestamp":"2026-11-15T00:00:00Z",
            "message":{"role":"assistant", "responseId":"response", "provider":"openai-codex",
                "model":"gpt-5-codex", "usage":{"input":10,"output":2,"cost":{"total":0.25}}}});
        std::fs::create_dir_all(record.session_file.parent().unwrap()).unwrap();
        // Duplicate response lines are one response; each child's distinct session still counts.
        std::fs::write(&record.session_file, format!("{response}\n{response}\n")).unwrap();
    }
    let task = target.container().task().unwrap().to_owned();
    crate::usage::refresh_usage(&fx.ctx.store, &task).unwrap();
    let once = fx.ctx.store.get(&task).unwrap().runs[0]
        .usage
        .clone()
        .unwrap();
    assert_eq!(once.entries.len(), 4);
    assert_eq!(once.total_tokens(), 48);
    assert_eq!(once.reported_usd, Some(1.0));
    crate::usage::refresh_usage(&fx.ctx.store, &task).unwrap();
    assert_eq!(
        fx.ctx.store.get(&task).unwrap().runs[0].usage.as_ref(),
        Some(&once)
    );
}

#[test]
fn fleet_prepare_refuses_disabled_workers_assistant_slices_and_planners_without_records() {
    let fx = ts::fx();
    let (target, host) = worker(&fx);
    fleet(&fx, &target, AgentRole::Worker, false);
    let refused = event(&fx, &host, 1, "slice", "prepare", "disabled", None);
    assert_eq!(ts::code(&refused), Some("op_not_allowed"));
    assert!(
        ts::state(&fx, &target.container())
            .usage_sessions
            .is_empty()
    );

    let session = fx
        .ctx
        .store
        .create_assistant("pi", None, Some("a-pane"), None)
        .unwrap();
    let assistant = BindTarget::Assistant {
        session: session.id,
    };
    let host = ts::bind(&fx, &assistant, "a-pane", "fleet-assistant");
    ts::committed(&ts::hello(&fx, &host));
    fleet(&fx, &assistant, AgentRole::Assistant, true);
    let refused = event(&fx, &host, 1, "slice", "prepare", "slice", None);
    assert_eq!(ts::code(&refused), Some("op_not_allowed"));
    assert!(
        ts::state(&fx, &assistant.container())
            .usage_sessions
            .is_empty()
    );
    ts::committed(&event(&fx, &host, 2, "review", "prepare", "review", None));
    assert_eq!(
        ts::state(&fx, &assistant.container()).usage_sessions[0].kind,
        UsageSessionKind::Review
    );

    let task = ts::worker_task(&fx, "planner-pane");
    let output = fx._tmp.path().join("plan");
    std::fs::create_dir_all(&output).unwrap();
    let socket = fx.ctx.herdr.socket_path().unwrap();
    fx.ctx.store.update(&task, |task| {
        task.runs.clear();
        let revision = crate::plan_spec_revision(task)?;
        task.plan_runs.push(serde_json::from_value(json!({"id":"D1", "profile":"pi", "harness":"corral-pi", "delivery":"inbox",
            "spec_revision":revision,"started":"2026-11-15T00:00:00Z","output_dir":output,"rounds":0,"state":"planning",
            "herdr":{"workspace_id":"w1","pane_id":"planner-pane","socket":socket}}))?);
        Ok(())
    }).unwrap();
    let planner = BindTarget::Planner {
        task,
        run: "D1".into(),
    };
    let host = ts::bind(&fx, &planner, "planner-pane", "fleet-planner");
    ts::committed(&ts::hello(&fx, &host));
    for (seq, kind) in [(1, "slice"), (2, "review")] {
        let refused = event(&fx, &host, seq, kind, "prepare", kind, None);
        assert_eq!(ts::code(&refused), Some("op_not_allowed"));
    }
    assert!(
        ts::state(&fx, &planner.container())
            .usage_sessions
            .is_empty()
    );
}
