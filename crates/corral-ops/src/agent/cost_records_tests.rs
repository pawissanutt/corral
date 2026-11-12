use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;

use corral_core::agent::{ContainerRef, TurnOrigin};
use corral_core::inbox::{MessageBinding, Post, SendOutcome, Urgency};
use serde_json::json;

use super::*;
use crate::agent::BindTarget;
use crate::agent::delivery::{ADAPTER, CorralPiAdapter, INPUT_MAX_BYTES};
use crate::agent::launch::{
    SOCKET_FILE, ensure_private_dir, generation_dir, runtime_dir, runtime_root, write_ctl_cap,
};
use crate::agent::test_support as ts;
use crate::delivery::{BindOutcome, DeliveryAdapter};

fn pm(fx: &ts::Fx) -> ts::Host {
    let host = ts::bind(
        fx,
        &BindTarget::Pm {
            project: "corral".into(),
        },
        "pm-pane",
        "pm-cost",
    );
    ts::committed(&ts::hello(fx, &host));
    host
}

fn wake(inbox: &Inbox<'_>, task: &str, project: &str) {
    inbox.post(Post {
        kind: "question_wake", urgency: Urgency::Wake, author: "corral",
        body: "Task-looking body T-999 must not affect attribution.",
        binding: MessageBinding::default(),
        subject: Some(json!({"key":format!("q:{task}/R1/Q1:1"), "question":format!("{task}/R1/Q1"), "project":project})),
        max_payload: INPUT_MAX_BYTES,
    }).unwrap();
}

fn binding(fx: &ts::Fx) -> corral_core::inbox::SessionBinding {
    match CorralPiAdapter.bind(&fx.ctx, &Recipient::Pm).unwrap() {
        BindOutcome::Bound(binding) => binding,
        _ => panic!("PM must be bound"),
    }
}

#[test]
fn question_envelope_cause_is_durable_before_bridge_submit_and_binds_at_turn_start() {
    let fx = ts::fx();
    let id = ts::worker_task(&fx, "worker");
    let host = pm(&fx);
    let inbox = Inbox::new(&fx.ctx.store, Recipient::Pm).unwrap();
    wake(&inbox, &id, "corral");
    let binding = binding(&fx);
    let mut envelope = inbox
        .publish(&binding, ADAPTER, INPUT_MAX_BYTES, false)
        .unwrap()
        .unwrap();
    inbox
        .begin_attempt(&mut envelope, corral_core::clock::now())
        .unwrap();
    let dir = generation_dir(&fx.ctx.store, &Recipient::Pm, host.identity.generation).unwrap();
    ensure_private_dir(&runtime_root(), &dir).unwrap();
    write_ctl_cap(&dir, &host.ctl_cap).unwrap();
    let listener = UnixListener::bind(dir.join(SOCKET_FILE)).unwrap();
    let store = fx.ctx.store.clone();
    let generation = host.identity.generation;
    let observed = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut line)
            .unwrap();
        let frame: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(frame["command"], "submit_input");
        let records = pm_cost::load(&store).unwrap();
        let key = frame["args"]["idempotency_key"].as_str().unwrap();
        assert_eq!(records.pending.len(), 1);
        assert_eq!(records.pending[0].envelope_key, key);
        let mut stream = stream;
        writeln!(stream, "{}", json!({"version":1,"id":frame["id"],"generation":generation,"ok":true,
            "result":{"request_id":frame["id"],"idempotency_key":key,"disposition":"started","turn":1,"replayed":false}})).unwrap();
    });
    assert!(matches!(
        CorralPiAdapter.send(&fx.ctx, &binding, &envelope),
        SendOutcome::Admitted { .. }
    ));
    observed.join().unwrap();
    let input = ts::envelope(
        &host,
        "event",
        &crate::agent::event_request_id(generation, 1),
        json!({
            "kind":"turn_started", "turn":1, "origin":{"kind":"bridge", "idempotency_key":super::super::delivery::wire_key(&envelope.idempotency_key)}
        }),
        false,
    );
    ts::committed(&ts::send(&fx, &input, "event"));
    let bound = pm_cost::attribution(&fx.ctx.store, generation, 1)
        .unwrap()
        .unwrap();
    assert_eq!(bound.class, CostClass::Item);
    assert_eq!(bound.task.as_deref(), Some(id.as_str()));
    assert_eq!(bound.project.as_deref(), Some("corral"));
    assert!(pm_cost::load(&fx.ctx.store).unwrap().pending.is_empty());
    std::fs::remove_dir_all(runtime_dir(&fx.ctx.store, &Recipient::Pm).unwrap()).unwrap();
}

#[test]
fn envelope_spanning_two_projects_is_shared_without_guessing_a_task() {
    let fx = ts::fx();
    let first = ts::worker_task(&fx, "a");
    let second = ts::worker_task(&fx, "b");
    fx.ctx
        .store
        .update(&second, |task| {
            task.project = Some("other".into());
            Ok(())
        })
        .unwrap();
    pm(&fx);
    let inbox = Inbox::new(&fx.ctx.store, Recipient::Pm).unwrap();
    wake(&inbox, &first, "corral");
    wake(&inbox, &second, "other");
    let envelope = inbox
        .publish(&binding(&fx), ADAPTER, INPUT_MAX_BYTES, false)
        .unwrap()
        .unwrap();
    prepare_envelope(&fx.ctx, &envelope).unwrap();
    let records = pm_cost::load(&fx.ctx.store).unwrap();
    assert_eq!(records.pending[0].cause.class, CostClass::Shared);
    assert_eq!(records.pending[0].cause.task, None);
    assert_eq!(records.pending[0].cause.project, None);
}

#[test]
fn report_reconciles_container_usage_and_keeps_legacy_and_unmatched_requests_unallocated() {
    let fx = ts::fx();
    let host = pm(&fx);
    let id = ts::worker_task(&fx, "worker");
    pm_cost::prepare(
        &fx.ctx.store,
        PendingCause {
            envelope_key: "pm:env1".into(),
            cause: Cause {
                class: CostClass::Item,
                project: Some("corral".into()),
                task: Some(id.clone()),
                ..Cause::default()
            },
        },
    )
    .unwrap();
    pm_cost::bind(
        &fx.ctx.store,
        1,
        1,
        &TurnOrigin {
            kind: TurnOriginKind::Bridge,
            idempotency_key: Some("pm:env1".into()),
        },
        None,
    )
    .unwrap();
    let assistant = |id: &str, input: u64| {
        json!({"type":"message", "id":id, "message":{
            "role":"assistant", "model":"gpt-5", "responseId":id,
            "usage":{"input":input,"cacheRead":2,"cacheWrite":3,"output":4,"cost":{"total":0.25}}
        }})
    };
    let lines = vec![
        assistant("legacy", 1),
        json!({"type":"custom","customType":"corral_cost_turn","data":{"generation":1,"turn":1,"phase":"start","key":"pm:env1"}}),
        assistant("item", 10),
        json!({"type":"custom","customType":"corral_cost_turn","data":{"generation":1,"turn":1,"phase":"end"}}),
        assistant("outside", 100),
        json!({"type":"custom","customType":"corral_cost_turn","data":{"generation":1,"turn":2,"phase":"start"}}),
        assistant("unmatched", 1000),
    ];
    std::fs::create_dir_all(&host.sessions).unwrap();
    std::fs::write(
        host.sessions.join("session.jsonl"),
        format!(
            "{}\n",
            lines
                .into_iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        ),
    )
    .unwrap();
    let total = super::super::usage::collect_container(&ContainerRef::Pm, &fx.ctx.store).unwrap();
    let report = by_item(&fx.ctx, CostWindow::default()).unwrap();
    assert_eq!(report.total, total);
    assert_eq!(report.coverage.total_requests, 4);
    assert_eq!(report.coverage.attributed_requests, 1);
    assert_eq!(report.coverage.legacy, 2);
    let unallocated = report
        .rows
        .iter()
        .find(|r| r.class == CostClass::Unallocated)
        .unwrap();
    assert_eq!(unallocated.requests, 3);
    assert_eq!(unallocated.input, 1101);
    assert_eq!(
        report.rows.iter().map(|r| r.input).sum::<u64>(),
        total.input
    );
    assert_eq!(
        report.rows.iter().map(|r| r.output).sum::<u64>(),
        total.output
    );
    assert_eq!(
        report.rows.iter().map(|r| r.cache_read).sum::<u64>(),
        total.cache_read
    );
    assert_eq!(
        report.rows.iter().map(|r| r.cache_write).sum::<u64>(),
        total.cache_write
    );
    assert!(
        (report.rows.iter().filter_map(|r| r.usd).sum::<f64>()
            - total.estimated_usd.unwrap_or(0.0))
        .abs()
            < 1e-12
    );
    assert!(
        report
            .rows
            .iter()
            .any(|r| r.class == CostClass::Overhead && r.requests == 0)
    );
}
