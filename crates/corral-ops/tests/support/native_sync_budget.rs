//! Owning native sync pipeline, not an isolated selector: real hello, PID identity and RPCs.
use crate::agent::{bridge::fixture as bridge, native_test_support as native};
use corral_core::{
    agent::{AgentRole, paths},
    inbox::Recipient,
    work::{self, Counter, HotScope},
};
use serde_json::json;

fn retained(fx: &bridge::Fx, scale: usize) {
    let mut task = fx.task();
    let run = task.runs.last_mut().unwrap();
    let mut state = native::host(AgentRole::Worker, 2 * scale);
    state.binding.store = paths::store_id(&fx.ctx.store);
    state.binding.project = "corral".into();
    for generation in state
        .generations
        .iter_mut()
        .filter(|generation| generation.n != 999)
    {
        generation.fenced = Some(corral_core::agent::GenerationFence {
            reason: "new_generation".into(),
            at: native::data::at(50),
        });
    }
    for i in 1..=2 * scale {
        let at = native::data::at(i as i64);
        let request = format!("old-{i:04}");
        state.receipts.push(
            serde_json::from_value(json!({
                "version":1,"request_id":request,"op":"reply","payload_sha256":"44".repeat(32),
                "identity":{"store":state.binding.store,"project":"corral","role":"worker",
                    "task":task.id,"run":run.id,"session_id":"fixed-session","generation":i},
                "state":"committed","at":at,"result":{"message_id":request},"evidence":[]
            }))
            .unwrap(),
        );
        let mut pm_receipt = state.receipts.last().unwrap().clone();
        pm_receipt.request_id = format!("pm-old-{i:04}");
        pm_receipt.identity.role = AgentRole::Pm;
        pm_receipt.identity.run = None;
        pm_receipt.identity.session_id = "historical-pm-session".into();
        task.agent_pm_receipts.push(pm_receipt);
        state.questions.push(serde_json::from_value(json!({
            "version":1,"key":format!("{}/{}/Q{i}",task.id,run.id),"revision":2,
            "asked":{"request_id":request,"payload_sha256":"44".repeat(32),
                "session_id":"fixed-session","generation":i,"at":at},
            "kind":"clarification","blocking":false,"text":"Retired question",
            "state":"withdrawn","triage":[{"action":"withdraw","by":"agent","revision":2,"at":at}],
            "wake":{"route":"none","key":request,"state":"posted"},
            "terminal":{"state":"withdrawn","reason":"withdrawn","at":at}
        })).unwrap());
        state.wakes.push(
            serde_json::from_value(json!({
                "key":request,"kind":"wait","target":"old-wait","request_id":request,
                "state":"fired","at":at,"fired_at":at,"message_id":request
            }))
            .unwrap(),
        );
        state.reports.push(
            serde_json::from_value(json!({
                "version":1,"kind":"done","request_id":request,"generation":i,"at":at
            }))
            .unwrap(),
        );
        state.jobs.push(
            serde_json::from_value(json!({
                "id":request,"generation":i,"command":"true","state":"exited",
                "prepared_at":at,"ended_at":at,"exit_code":0,"group_clear":true,"swept":true
            }))
            .unwrap(),
        );
        state.rewinds.push(
            serde_json::from_value(json!({
                "id":format!("rw-{i:04}"),"checkpoint":"g1-t1","commit":"commit","head":"head",
                "leaf":"leaf","from_generation":i,"stage":"aborted","at":at,"updated":at
            }))
            .unwrap(),
        );
        state.usage_sessions.push(serde_json::from_value(json!({
            "generation":i,"kind":"oracle","session_id":request,
            "session_file":fx.root().join("historical-usage.jsonl"),"request":request,"outcome":"completed"
        })).unwrap());
        state.budget.requests.push(
            serde_json::from_value(json!({
                "request":request,"generation":i,"tokens":100,"usd_nanos":1000,
                "priced":true,"response":request
            }))
            .unwrap(),
        );
    }
    run.agent_host = Some(Box::new(state));
    fx.ctx.store.fixture_put_task(&task).unwrap();
}

pub(super) fn fixture(scale: usize) -> bridge::Fx {
    corral_core::clock::pin_thread(Some(native::data::at(100_000)));
    let fx = bridge::fixture(false, 1);
    fx.ctx
        .store
        .fixture_pin_now(native::data::at(100_000))
        .unwrap();
    super::action_budget_growth::grow(&fx.ctx.store, fx.root(), &fx.id, 2 * scale, true, true);
    for i in 0..2 * scale {
        let mut task = fx
            .ctx
            .store
            .create(
                &format!("Retired {i:04}"),
                Some("corral"),
                corral_core::Status::Done,
                "old",
            )
            .unwrap();
        native::data::grow(&mut task, 2 * scale, 2 * scale, 2 * scale, 2 * scale);
        fx.ctx.store.fixture_put_task(&task).unwrap();
    }
    // A completed tracker/member pair also grows the T-251 history family. It is not
    // reclassified into live work just to make this worker's pipeline select it.
    let mut tracker = fx
        .ctx
        .store
        .create(
            "Retired tracker",
            Some("corral"),
            corral_core::Status::Done,
            "tracking",
        )
        .unwrap();
    tracker.tracker = true;
    tracker.member_gates.insert("T-002".into(), serde_json::from_value(json!({
            "revision":2 * scale + 1,"gate":{"tier":"quick"},"reason":"current rule","by":"owner","at":native::data::at(99),
            "history":(1..=2 * scale).map(|revision| json!({"revision":revision,"gate":{"tier":"quick"},"reason":"previous rule","by":"owner","at":native::data::at(revision as i64)})).collect::<Vec<_>>()
        })).unwrap());
    fx.ctx.store.fixture_put_task(&tracker).unwrap();
    fx.ctx
        .store
        .update("T-002", |member| {
            member.parent = Some(tracker.id.clone());
            Ok(())
        })
        .unwrap();
    assert_eq!(
        fx.ctx.store.get(&tracker.id).unwrap().member_gates["T-002"]
            .history
            .len(),
        2 * scale
    );
    retained(&fx, scale);
    fx
}

const CHILD_ROOT: &str = "CORRAL_NATIVE_TASK_PROCESS_ROOT";
const CHILD_REPORT: &str = "CORRAL_NATIVE_TASK_PROCESS_REPORT";
pub(super) type Cost = [u64; Counter::ALL.len()];

#[test]
fn native_task_sync_fresh_process_child() {
    let Some(root) = std::env::var_os(CHILD_ROOT) else {
        return;
    };
    corral_core::clock::pin_thread(Some(native::data::at(100_000)));
    crate::agent::bridge::liveness::probe_hosts_in_tests();
    let ctx = bridge::ctx_at(std::path::Path::new(&root));
    let cost = observe(&ctx, "T-001");
    let report = json!({"pid":std::process::id(),"cost":cost});
    std::fs::write(
        std::env::var_os(CHILD_REPORT).unwrap(),
        serde_json::to_vec(&report).unwrap(),
    )
    .unwrap();
    corral_core::clock::pin_thread(None);
}

pub(super) fn observe(ctx: &crate::Ctx, id: &str) -> Cost {
    let oracle = corral_core::Store::open(ctx.store.root()).unwrap();
    let events = oracle.events().unwrap();
    let before = work::snapshot();
    let hot = HotScope::enter();
    let output = crate::agent::hooks::sync::task_id(ctx, id, ctx.store.now()).unwrap();
    let delta = work::snapshot().since(&before);
    drop(hot);
    assert_eq!(output.open.len(), 1, "{output:?}");
    assert_eq!(
        output.open[0].container,
        corral_core::agent::ContainerRef::Worker {
            task: id.into(),
            run: "R1".into()
        }
    );
    assert!(output.open[0].healthy, "{output:?}");
    assert_eq!(output.attention, None);
    assert_eq!(
        ctx.store
            .get(id)
            .unwrap()
            .runs
            .last()
            .unwrap()
            .agent_host
            .as_ref()
            .unwrap()
            .binding
            .health,
        corral_core::agent::HostHealth::Healthy
    );
    assert_eq!(
        ctx.store.events().unwrap(),
        events,
        "steady pipeline must not append events"
    );
    assert!(delta.followup_calls.is_empty(), "{delta:?}");
    assert_eq!(delta.get(Counter::EventDecodes), 0, "{delta:?}");
    for counter in Counter::ALL {
        let cells = delta.cells(counter);
        assert_eq!(
            cells.total,
            cells.unattributed
                + cells.own_record
                + cells.emitted
                + cells.followup.values().sum::<u64>()
        );
    }
    assert_eq!(delta.unattributed_hydrations(), 0, "{delta:?}");
    eprintln!(
        "NATIVE owning pipeline pid={}: {delta:?}",
        std::process::id()
    );
    delta.check_identity().unwrap();
    Counter::ALL.map(|counter| delta.get(counter))
}

fn rpc(socket: &bridge::Bridge) {
    let frames = socket.commands();
    assert_eq!(
        frames
            .iter()
            .filter(|frame| frame["command"] == "get_state")
            .count(),
        1,
        "{frames:?}"
    );
    assert_eq!(
        frames
            .iter()
            .filter(|frame| frame["command"] == "events_since")
            .count(),
        1,
        "{frames:?}"
    );
}

#[test]
fn owning_native_sync_pipeline_has_fixed_unattributed_work_with_real_host_probes() {
    let mut costs = Vec::new();
    for scale in [1, 10] {
        let fx = fixture(scale);
        let process = bridge::HostProc::spawn(true);
        let host = fx.bind(&fx.worker_target(), "w0:p1", &process);
        assert_eq!(host.identity.generation, 1000);
        let stored = fx.task();
        let native = stored.runs.last().unwrap().agent_host.as_ref().unwrap();
        assert_eq!(native.generations.len(), 2 * scale + 2);
        assert_eq!(native.receipts.len(), 2 * scale + 1);
        for count in [
            native.turns.len(),
            native.questions.len(),
            native.wakes.len(),
            native.reports.len(),
            native.jobs.len(),
            native.rewinds.len(),
            native.usage_sessions.len(),
            native.budget.requests.len(),
            stored.agent_pm_receipts.len(),
        ] {
            assert_eq!(count, 2 * scale, "actual canonical retained-family growth");
        }
        let socket = bridge::bridge(
            &fx.ctx.store,
            &Recipient::Run {
                task: fx.id.clone(),
                run: "R1".into(),
            },
            1000,
            Some(&host.ctl_cap),
            bridge::quiet,
        );
        crate::agent::bridge::liveness::probe_hosts_in_tests();
        // Actual replay materializes the canonical event identities before either measured pass.
        crate::questions::reconcile_events(&fx.ctx.store, &fx.id).unwrap();
        for fresh in [false, true] {
            let ctx = if fresh {
                bridge::ctx_at(fx.root())
            } else {
                fx.ctx.clone()
            };
            costs.push(observe(&ctx, &fx.id));
            rpc(&socket);
        }
        let report = fx.root().join("native-task-process-report.json");
        assert!(!report.exists());
        let child = crate::test_support::command(std::env::current_exe().unwrap(), fx.root())
            .args([
                "--exact",
                "submission_paths_tests::native_sync_budget::native_task_sync_fresh_process_child",
                "--nocapture",
            ])
            .env(CHILD_ROOT, fx.root())
            .env(CHILD_REPORT, &report)
            .env("RUST_TEST_THREADS", "1")
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(
            child.status.success(),
            "fresh task child: {}\n{}",
            String::from_utf8_lossy(&child.stdout),
            String::from_utf8_lossy(&child.stderr)
        );
        assert!(String::from_utf8_lossy(&child.stdout).contains("1 passed; 0 failed;"));
        let result: serde_json::Value =
            serde_json::from_slice(&std::fs::read(report).unwrap()).unwrap();
        let pid = result["pid"].as_u64().unwrap();
        assert_ne!(pid, u64::from(std::process::id()));
        let cost: Cost = serde_json::from_value(result["cost"].clone()).unwrap();
        costs.push(cost);
        rpc(&socket);
        eprintln!("Fresh native task sync child pid={pid} scale={scale}");
        corral_core::clock::pin_thread(None);
    }
    for cost in &costs[1..] {
        assert_eq!(
            cost, &costs[0],
            "every unattributed cell, no ceiling/mask changes"
        );
    }
}
