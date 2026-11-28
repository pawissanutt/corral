use std::sync::Mutex;

use super::*;

#[test]
fn pipelined_moves_are_admitted_in_order_without_blocking_ping() {
    let hold = Arc::new(Mutex::new(()));
    let gate = Arc::clone(&hold);
    let fixture = Fixture::with_runner(Arc::new(move || {
        drop(gate.lock().unwrap_or_else(|error| error.into_inner()));
        Box::new(FakeRunner::new())
    }));
    let engine = fixture.shared.engine().unwrap();
    let task = engine
        .new_task(NewTask {
            title: "pipelined moves".into(),
            status: Status::Backlog,
            ..NewTask::default()
        })
        .unwrap();
    let mut client = Client::connect(&fixture.unix_server());
    fixture.shared.late_ops.set_wait(Duration::from_secs(30));

    // Hold operation workers before they open the engine. PING proves the connection has
    // consumed both submits; neither worker can finish or release its permit before then.
    let guard = hold.lock().unwrap();
    for (id, status) in [(2, Status::Ready), (3, Status::Backlog)] {
        send(
            &mut client,
            id,
            wire::op::OP_SUBMIT,
            OpSubmit {
                op: Op::Move {
                    id: task.id.clone(),
                    status,
                },
                force: false,
            },
        );
    }
    send(&mut client, 4, wire::PING, ());
    let _: wire::Pong = receive(&mut client, 4);
    let workers = fixture.shared.lock().op_workers;
    assert_eq!(workers, 1, "the second Move could race the first");
    drop(guard);

    let OpReply::Done { seq: first, .. } = receive(&mut client, 2) else {
        panic!("the first Move should have finished");
    };
    let OpReply::Done { seq: second, .. } = receive(&mut client, 3) else {
        panic!("the second Move should have finished");
    };
    assert!(first < second);
    assert_eq!(
        engine.store.get_task(&task.id).unwrap().unwrap().status,
        Status::Backlog,
        "executing the moves in reverse would leave the task Ready"
    );
}

/// Pipeline ordinary RPCs before releasing the writer that one pending Move needs.
#[test]
fn pending_move_does_not_block_reads_on_the_same_connection() {
    let fixture = Fixture::new();
    let engine = fixture.shared.engine().unwrap();
    let task = engine
        .new_task(NewTask {
            title: "pending move".into(),
            status: Status::Backlog,
            ..NewTask::default()
        })
        .unwrap();
    let before = engine.store.latest_seq().unwrap();
    let path = fixture.unix_server();
    let mut client = Client::connect(&path);
    engine
        .store
        .connection
        .execute_batch("BEGIN IMMEDIATE")
        .unwrap();
    // Reach Running before SQLite's busy timeout so the follow-up wait is tested too.
    // The protocol frame order, not a sleep, decides when the writer is released.
    fixture.shared.late_ops.set_wait(Duration::from_secs(1));
    send(
        &mut client,
        2,
        wire::op::OP_SUBMIT,
        OpSubmit {
            op: Op::Move {
                id: task.id.clone(),
                status: Status::Ready,
            },
            force: false,
        },
    );
    send(&mut client, 3, wire::PING, ());
    send(
        &mut client,
        4,
        wire::SUB_OPEN,
        SubOpen {
            topic: Topic::Tasks,
            since: None,
            scope: None,
        },
    );
    let _: wire::Pong = receive(&mut client, 3);
    receive::<()>(&mut client, 4);
    let notification: rpc::Notification<SubDelta> =
        serde_json::from_value(next(&mut client)).unwrap();
    assert_eq!(notification.method, wire::SUB_DELTA);
    let SubDelta::Tasks { delta } = notification.params else {
        panic!("expected Tasks page");
    };
    assert_eq!(delta.rows[0].task.id, task.id);
    assert_eq!(delta.rows[0].task.status, Status::Backlog);

    let OpReply::Running {
        operation: Some(operation),
        ..
    } = receive(&mut client, 2)
    else {
        panic!("the held move should be running");
    };
    fixture.shared.late_ops.set_wait(Duration::from_secs(5));
    let result = wire::op::OpResult { operation };
    send(&mut client, 5, wire::op::OP_RESULT, &result);
    send(&mut client, 6, wire::PING, ());
    let _: wire::Pong = receive(&mut client, 6);
    engine.store.connection.execute_batch("ROLLBACK").unwrap();

    let done: OpReply = receive(&mut client, 5);
    let events = engine
        .store
        .event_page(Some(&task.id), Some(before), None, false, 8)
        .unwrap();
    assert_eq!(events.len(), 1, "Move should execute once");
    assert_eq!(events[0].kind, EventKind::Moved);
    assert_eq!(
        done,
        OpReply::Done {
            seq: events[0].seq,
            task: Some(task.id)
        }
    );
    // A new authenticated connection can recover exactly the same receipt, not replay Move.
    drop(client);
    let again: OpReply = Client::connect(&path).ok(wire::op::OP_RESULT, result);
    assert_eq!(again, done);
}

fn send(client: &mut Client, id: u64, method: &str, params: impl Serialize) {
    write_frame(
        &mut client.0,
        &Frame {
            kind: FrameKind::Control,
            stream: 0,
            payload: serde_json::to_vec(&rpc::Request {
                jsonrpc: rpc::JSONRPC.into(),
                id: json!(id),
                method: method.into(),
                params,
            })
            .unwrap(),
        },
    )
    .unwrap();
}

fn next(client: &mut Client) -> Value {
    let frame = read_frame(&mut client.0).unwrap().unwrap();
    serde_json::from_slice(&frame.payload).unwrap()
}

fn receive<T: DeserializeOwned>(client: &mut Client, id: u64) -> T {
    let reply: rpc::Response = serde_json::from_value(next(client)).unwrap();
    assert_eq!(
        reply.id,
        json!(id),
        "reply for {id} was blocked by {reply:?}"
    );
    match reply.outcome {
        rpc::ResponseOutcome::Result { result } => serde_json::from_value(result).unwrap(),
        rpc::ResponseOutcome::Error { error } => panic!("unexpected error: {error:?}"),
    }
}
