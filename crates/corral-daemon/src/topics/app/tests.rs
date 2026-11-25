use std::fs;

use corral_engine::{NewTask, Op, SessionStatus, Status, Usage};
use corral_protocol::{
    rpc,
    subscription::{SubAck, SubDelta},
};

use super::*;
use crate::{conn::Conn, topics::Subscriptions, views::tests::Fixture};

#[derive(Default)]
struct Receiver(Vec<Frame>);

impl Conn for Receiver {
    fn send_frame(&mut self, frame: &Frame) -> Result<()> {
        self.0.push(frame.clone());
        Ok(())
    }
}

fn receive(subs: &mut Subscriptions<'_>) -> Option<SubDelta> {
    let mut receiver = Receiver::default();
    subs.flush(&mut receiver).unwrap();
    assert!(receiver.0.len() < 2);
    receiver.0.pop().map(|frame| {
        assert!(frame.payload.len() <= APP_CONTROL_MAX);
        let message: rpc::Notification<SubDelta> = serde_json::from_slice(&frame.payload).unwrap();
        assert_eq!(message.method, corral_protocol::SUB_DELTA);
        message.params
    })
}

fn start(fixture: &Fixture, topic: Topic) -> (Subscriptions<'_>, SubDelta) {
    let mut subs = Subscriptions::new(&fixture.shared);
    subs.open(SubOpen {
        topic,
        scope: None,
        since: None,
    })
    .unwrap();
    let first = receive(&mut subs).unwrap();
    (subs, first)
}

fn ack(subs: &mut Subscriptions<'_>, topic: Topic, cursor: BoardCursor) {
    subs.ack(SubAck {
        topic,
        cursor,
        scope: None,
    })
    .unwrap();
}

#[test]
fn the_retired_thread_topic_is_not_accepted() {
    let fixture = Fixture::new();
    let mut subs = Subscriptions::new(&fixture.shared);
    let open: SubOpen = serde_json::from_value(serde_json::json!({
        "topic": "thread", "scope": "quick",
    }))
    .unwrap();
    assert_eq!(open.topic, Topic::Other);
    assert!(subs.open(open).is_err());
}

#[test]
fn health_delta_tracks_sync_result() {
    let fixture = Fixture::new();
    let (mut subs, SubDelta::Health { cursor, rows, .. }) = start(&fixture, Topic::Health) else {
        panic!("expected health snapshot");
    };
    assert!(rows[0].last_sync_at.is_none());
    ack(&mut subs, Topic::Health, cursor);
    fixture.apply(Op::Sync);
    fixture.shared.finish_sync(true, None);
    let Some(SubDelta::Health { rows, .. }) = receive(&mut subs) else {
        panic!("expected sync health delta");
    };
    assert!(rows[0].runner_reachable);
    assert!(rows[0].last_sync_at.is_some());
}

#[test]
fn tasks_delta_carries_engine_blocked_and_requires_exact_ack() {
    let fixture = Fixture::new();
    fixture.project();
    let id = fixture.named_task("Needs input", Some("demo"));
    let run = fixture.dispatch(&id);
    let (mut subs, SubDelta::Tasks { delta }) = start(&fixture, Topic::Tasks) else {
        panic!("expected tasks");
    };
    assert_eq!(delta.rows[0].task.status, Status::Running);
    assert_eq!(delta.rows[0].run.as_ref().unwrap().id, run.id);
    let excerpt = "Which branch should I target?";
    fixture.runner.set_status(
        run.session.as_ref().unwrap(),
        SessionStatus::Blocked {
            excerpt: excerpt.into(),
        },
    );
    fixture.apply(Op::Sync);
    assert!(receive(&mut subs).is_none());
    assert!(
        subs.ack(SubAck {
            topic: Topic::Tasks,
            cursor: BoardCursor::default(),
            scope: None
        })
        .is_err()
    );
    ack(&mut subs, Topic::Tasks, delta.cursor.clone());
    assert!(
        subs.ack(SubAck {
            topic: Topic::Tasks,
            cursor: delta.cursor,
            scope: None
        })
        .is_err()
    );
    let Some(SubDelta::Tasks { delta }) = receive(&mut subs) else {
        panic!("expected blocked tasks delta");
    };
    let task = &delta.rows[0].task;
    assert_eq!(task.status, Status::Blocked);
    let blocked = task.blocked.as_ref().unwrap();
    assert!(!blocked.reason.is_empty());
    assert_eq!(blocked.detail.as_deref(), Some(excerpt));
    assert!(matches!(&blocked.next[0].op, Op::Reply { id: target, .. } if target == &id));
    assert!(!blocked.next[0].force);
    ack(&mut subs, Topic::Tasks, delta.cursor);
    fixture.apply(Op::Drop { id: id.clone() });
    let Some(SubDelta::Tasks { delta }) = receive(&mut subs) else {
        panic!("expected task removal");
    };
    assert!(delta.rows.is_empty());
    assert_eq!(delta.left, [id]);
}

#[test]
fn plans_delta_groups_children_by_status() {
    let fixture = Fixture::new();
    let id = fixture.task(NewTask {
        title: "Plan".into(),
        body: "# Markdown".into(),
        ..Default::default()
    });
    let (mut subs, SubDelta::Plans { delta }) = start(&fixture, Topic::Plans) else {
        panic!("expected plans");
    };
    assert!(delta.rows.is_empty());
    ack(&mut subs, Topic::Plans, delta.cursor);
    fixture.task(NewTask {
        title: "Child".into(),
        parent: Some(id.clone()),
        status: Status::Ready,
        ..Default::default()
    });
    let Some(SubDelta::Plans { delta }) = receive(&mut subs) else {
        panic!("expected parent after its first child");
    };
    let plan = &delta.rows[0];
    assert_eq!(plan.parent.id, id);
    assert_eq!(plan.parent.body, "# Markdown");
    assert_eq!(plan.child_counts[0].status, Status::Ready);
    assert_eq!(plan.child_counts[0].count, 1);
}

#[test]
fn projects_delta_counts_only_open_tasks() {
    let fixture = Fixture::new();
    fixture.project();
    let id = fixture.named_task("Open", Some("demo"));
    let (mut subs, SubDelta::Projects { delta }) = start(&fixture, Topic::Projects) else {
        panic!("expected projects");
    };
    assert_eq!(delta.rows[0].open, 1);
    ack(&mut subs, Topic::Projects, delta.cursor);
    fixture.apply(Op::Drop { id });
    let Some(SubDelta::Projects { delta }) = receive(&mut subs) else {
        panic!("expected project count delta");
    };
    assert_eq!(delta.rows[0].id, "demo");
    assert_eq!(delta.rows[0].open, 0);
}

#[test]
fn agents_delta_includes_runner_status_and_persisted_quiet_since() {
    let fixture = Fixture::new();
    fixture.project();
    let id = fixture.named_task("Active", Some("demo"));
    let run = fixture.dispatch(&id);
    let (mut subs, SubDelta::Agents { delta }) = start(&fixture, Topic::Agents) else {
        panic!("expected agents");
    };
    assert_eq!(delta.rows[0].status, SessionStatus::Working);
    ack(&mut subs, Topic::Agents, delta.cursor);
    fixture
        .runner
        .set_status(run.session.as_ref().unwrap(), SessionStatus::Idle);
    fixture.apply(Op::Sync);
    let Some(SubDelta::Agents { delta }) = receive(&mut subs) else {
        panic!("expected idle run update");
    };
    assert_eq!(delta.rows[0].status, SessionStatus::Idle);
    assert!(delta.rows[0].run.quiet_since.is_some());
}

#[test]
fn usage_delta_observes_usage_without_a_new_event() {
    let fixture = Fixture::new();
    let (mut subs, SubDelta::Usage { delta }) = start(&fixture, Topic::Usage) else {
        panic!("expected usage");
    };
    assert_eq!(delta.rows[0].total.requests, 0);
    assert_eq!(delta.rows[0].total.input_requests, Some(0));
    let mut cursor = delta.cursor;
    for (model, billing, usd, requests) in [
        ("anthropic-api/claude-sonnet-4-6", "api", Some(0.02), 1),
        ("openai-codex/gpt-6.2-sol", "subscription", None, 2),
    ] {
        ack(&mut subs, Topic::Usage, cursor);
        let seq = fixture
            .apply(Op::UsageAdd {
                usage: Usage {
                    at: jiff::Timestamp::now(),
                    model: model.into(),
                    input: 100,
                    output: 20,
                    usd,
                    billing: Some(billing.into()),
                    ..Usage::default()
                },
            })
            .seq;
        assert_eq!(seq, 0);
        let Some(SubDelta::Usage { delta }) = receive(&mut subs) else {
            panic!("expected usage delta");
        };
        let total = &delta.rows[0].total;
        assert_eq!(
            (total.input, total.input_requests),
            (100 * requests, Some(requests))
        );
        assert_eq!(
            (total.output, total.output_requests),
            (20 * requests, Some(requests))
        );
        assert_eq!((total.cache_read, total.cache_read_requests), (0, Some(0)));
        assert_eq!(
            (total.cache_write, total.cache_write_requests),
            (0, Some(0))
        );
        assert_eq!(total.requests, requests);
        assert_eq!(total.estimate_missing_token_requests, requests);
        assert_eq!(total.estimated_requests, 0);
        assert_eq!(total.estimate_usd, None);
        assert!(total.estimate_sources.is_empty());
        assert_eq!((total.usd, total.priced_requests), (Some(0.02), 1));
        cursor = delta.cursor;
    }
}

#[test]
fn settings_delta_contains_parsed_config_listener_and_devices() {
    let fixture = Fixture::new();
    let (mut subs, SubDelta::Settings { delta }) = start(&fixture, Topic::Settings) else {
        panic!("expected settings");
    };
    assert!(delta.rows[0].listener.is_none());
    let observed_hostname = delta.rows[0].hostname.as_deref();
    let uname = rustix::system::uname();
    let expected_hostname = uname
        .nodename()
        .to_str()
        .ok()
        .filter(|name| !name.is_empty());
    assert_eq!(observed_hostname, expected_hostname);
    ack(&mut subs, Topic::Settings, delta.cursor);
    let devices = fixture.shared.devices();
    let device = devices.enrol_native("Native test", "test").unwrap();
    let bound = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = bound.local_addr().unwrap();
    let origins = ["http://localhost", "https://corral.example"];
    devices.set_listener(
        address,
        &origins.map(str::to_owned),
        Some("wss://corral.example"),
    );
    fs::write(
        fixture.shared.root().join(".corral-next/corral.toml"),
        "[limits]\nrunning = 7\n",
    )
    .unwrap();
    fixture.apply(Op::Sync);
    let Some(SubDelta::Settings { delta }) = receive(&mut subs) else {
        panic!("expected new settings");
    };
    let row = &delta.rows[0];
    assert_eq!(row.config["limits"]["running"].as_u64(), Some(7));
    assert_eq!(row.devices[0].id, device.id);
    let listener = row.listener.as_ref().unwrap();
    assert_eq!(listener.port, address.port());
    assert_eq!(listener.address, Some(address.to_string()));
    assert_eq!(listener.origins, origins);
}

#[test]
fn host_topic_needs_its_grant_and_serves_one_shared_snapshot() {
    let fixture = Fixture::new();
    let mut plain = Subscriptions::new(&fixture.shared);
    let open = SubOpen {
        topic: Topic::Host,
        scope: None,
        since: None,
    };
    assert!(plain.open(open.clone()).is_err());

    let mut subs = Subscriptions::new(&fixture.shared);
    subs.set_capabilities(&[corral_protocol::host::HOST_V1.into()]);
    subs.open(open).unwrap();
    let Some(SubDelta::Host { delta }) = receive(&mut subs) else {
        panic!("expected a host page");
    };
    assert_eq!(delta.rows.len(), 1);
    assert_eq!(
        delta.rows[0].interval_ms,
        corral_protocol::host::HOST_INTERVAL_MS
    );
    let root = fixture.shared.root();
    let (a, b) = (
        fixture.shared.host.latest(root),
        fixture.shared.host.latest(root),
    );
    assert!(Arc::ptr_eq(&a, &b), "subscribers share one sampler");
    fixture.shared.stop();
}
