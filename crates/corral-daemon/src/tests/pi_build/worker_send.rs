use super::*;

#[test]
fn a_worker_send_records_the_reply_event() {
    let fixture = Fixture::with_repository();
    let socket = fixture.unix_server();
    let mut owner = Client::connect(&socket);
    let task = owner.new_task("record a worker reply", Vec::new());
    owner.op(dispatch(&task), false);
    let engine = fixture.shared().engine().unwrap();
    let mut run = engine.store.list_runs(&task).unwrap().pop().unwrap();
    let dir = fixture
        .root()
        .join(crate::STATE_DIR)
        .join("runs")
        .join(&task)
        .join(&run.id);
    let id = format!("{task}_{}", run.id);
    let host = Host::start(
        fixture.root().join("worker.sock"),
        id.clone(),
        "worker",
        &dir,
        Script::default(),
    );
    run.session = Some(SessionRef::Pi {
        socket: host.socket.clone(),
        inner: Box::new(run.session.take().unwrap()),
        dir,
    });
    engine.store.update_run(&run).unwrap();
    let ack: SessionSent = owner.ok(SESSION_SEND, json!({
        "id": format!("{task}/{}", run.id), "cmd_id": "worker:1", "text": "continue the work", "policy": "steer",
    }));
    assert_eq!(ack.session, id);
    assert_eq!(ack.entry, format!("{id}/input/worker:1"));
    let events = engine.store.list_events(&task).unwrap();
    assert!(events.iter().any(|event| event.kind == EventKind::Replied
        && event.run.as_deref() == Some(run.id.as_str())
        && event.actor == Actor::Owner
        && event.detail["text"] == "continue the work"));
}
