use super::clock::wall_ms;
use super::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::os::unix::net::UnixStream;

const VECTORS: &str = include_str!("../../../../docs/contracts/lite-analyst-v1/vectors.json");

fn enrollment() -> bootstrap::Enrollment {
    bootstrap::Enrollment {
        link_id: "link_A".into(),
        lite_instance: "lite_A".into(),
        corral_instance: "corral_A".into(),
        channel_epoch: "epoch_A".into(),
        prior_channel_epoch: None,
        profile_sha256: "1".repeat(64),
        corral_manifest_sha256: "2".repeat(64),
        joint_lock_sha256: "3".repeat(64),
    }
}

fn approval(now: u64, request_id: &str, question: &str) -> Value {
    let mut body = serde_json::from_str::<Value>(VECTORS).unwrap()["request"]["body"].clone();
    body["deadline_ms"] = json!(now + 60_000);
    body["request_id"] = json!(request_id);
    body["question"] = json!(question);
    let bytes = wire::canonical(&body).unwrap();
    json!({"request_b64":STANDARD.encode(&bytes),"request_digest":wire::sha256(&bytes)})
}

fn work_pair() -> (Work, Work, std::os::fd::OwnedFd) {
    let (a, b) = UnixStream::pair().unwrap();
    let (lifetime, keep_alive) = rustix::pipe::pipe().unwrap();
    let server = Work::new(a, lifetime.try_clone().unwrap()).unwrap();
    let peer = Work::new(b, lifetime).unwrap();
    (server, peer, keep_alive)
}

fn deadline() -> Instant {
    Instant::now() + FRAME_TIMEOUT
}

fn new_session<'a>(store: &'a mut Store, work: &'a Work) -> Session<'a> {
    Session {
        store,
        work,
        clock: Clock::new().unwrap(),
        pending_bind: None,
        pending_fence: None,
        fixture: None,
    }
}

fn state_root() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt as _;
    tempfile::Builder::new()
        .permissions(std::fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}

#[test]
fn fixture_vectors_and_strict_framing() {
    let values = serde_json::from_str::<Value>(VECTORS).unwrap();
    let start: wire::Start = wire::params(values["start"]["params"].clone()).unwrap();
    let body = start.body().unwrap();
    assert_eq!(
        wire::canonical(&body).unwrap(),
        values["request"]["canonical_utf8"]
            .as_str()
            .unwrap()
            .as_bytes()
    );
    let hello = wire::hello(
        "corral_A",
        wire::params(values["hello"]["params"].clone()).unwrap(),
    )
    .unwrap();
    assert_eq!(hello, values["hello"]["result"]);
    for malformed in [
        r#"{"v":1,"type":"request","id":"a","op":"hello","params":{"nested":{"x":1,"x":2}}}"#,
        r#"{"v":1,"type":"request","id":"a","op":"hello","params":{"integer":9007199254740992}}"#,
        r#"{"v":1,"type":"request","id":"a","op":"hello","params":{"float":1.0}}"#,
        r#"{"v":1,"type":"request","id":"a","op":"hello","params":{"bad":"\ud800"}}"#,
        r#"{"v":1,"type":"response","id":"a","error":"invalid","result":{}}"#,
        r#"{"v":1,"type":"request","id":"a","op":"hello","params":{},"link_id":"link_B"}"#,
        r#"{"v":1,"type":"request","id":"a","op":"hello","params":null}"#,
    ] {
        assert!(
            Message::decode(malformed.as_bytes()).is_err(),
            "accepted {malformed}"
        );
    }
    assert!(matches!(
        Message::decode(br#"{"v":1,"type":"response","id":"a","error":"stopped"}"#),
        Ok(Message::Failure {
            error: Fault::Stopped,
            ..
        })
    ));
}

#[test]
fn dropped_start_reply_reopen_keeps_one_intent_and_rejects_conflict() {
    let dir = state_root();
    let mut e = enrollment();
    let wall = wall_ms().unwrap();
    let params = approval(wall, "start", "Public fixture");
    let mut store = Store::enroll(dir.path(), &e, wall).unwrap();
    let (work, peer, _life) = work_pair();
    let mut session = new_session(&mut store, &work);
    // Failure after the transactional handler, at the actual reply handoff boundary.
    peer.send(
        &Message::Request {
            v: 1,
            id: "request".into(),
            op: "start_investigation".into(),
            params: params.clone(),
        },
        deadline(),
    )
    .unwrap();
    let Message::Request {
        id,
        op,
        params: received,
        ..
    } = work.receive_message(deadline()).unwrap()
    else {
        panic!("expected start");
    };
    let committed = session.apply(&op, received, (wall, wall));
    assert!(committed.is_ok());
    drop(peer);
    assert!(work.respond(id, committed, deadline()).is_err());
    let id = session.pending_bind.take().unwrap().id;
    assert_eq!(session.store.read(&id).unwrap().state, "admitted");
    // Simulate crash without clean wind-down, then a trusted launcher re-exec.
    drop(store);
    e.prior_channel_epoch = Some(e.channel_epoch.clone());
    e.channel_epoch = "epoch_B".into();
    let mut store = Store::enroll(dir.path(), &e, wall + 61_000).unwrap();
    let replay = store
        .start(&wire::params(params).unwrap(), wall + 61_000)
        .unwrap();
    assert!(!replay.fresh);
    assert_eq!(replay.snapshot.run_id, id);
    assert_eq!(
        (replay.snapshot.seq, replay.snapshot.state.as_str()),
        (3, "cancelled")
    );
    let conflict = approval(wall, "start", "Different body");
    assert!(matches!(
        store.start(&wire::params(conflict).unwrap(), wall + 61_000),
        Err(Fault::Conflict)
    ));
    let unknown = approval(wall + 61_000, "unknown", "Public fixture");
    assert!(matches!(
        store.start(&wire::params(unknown).unwrap(), wall + 61_000),
        Err(Fault::Stopped)
    ));
    let count: u64 = store
        .db
        .query_row("SELECT count(*) FROM runs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
    drop(store);
    assert!(
        Store::enroll(dir.path(), &e, wall + 61_000).is_err(),
        "reused epoch"
    );
}

#[test]
fn launch_fence_holds_slot_until_verified_cleanup_and_hides_foreign_runs() {
    let dir = state_root();
    let mut e = enrollment();
    let wall = wall_ms().unwrap();
    let mut store = Store::enroll(dir.path(), &e, wall).unwrap();
    let start = |id| wire::params(approval(wall, id, "Public fixture")).unwrap();
    let beyond_cap = wire::params(approval(wall + 60_001, "too_late", "Public fixture")).unwrap();
    assert!(matches!(
        store.start_with(&beyond_cap, (wall, wall + 100), || Ok(())),
        Err(Fault::Invalid)
    ));
    let id = store.start(&start("a"), wall).unwrap().snapshot.run_id;
    store.bind(&id, 1, wall).unwrap();
    let launch = store.submit_launch(&id, 1, wall).unwrap();
    assert!(matches!(
        store.submit_launch(&id, 1, wall),
        Err(Fault::Stopped)
    ));
    assert_eq!(
        store.stop(&id, 2, FenceReason::Stopped),
        Err(Fault::StaleGeneration)
    );
    let stop = store.stop(&id, 1, FenceReason::StaleScope).unwrap();
    assert_eq!(stop.reason.as_deref(), Some("stale_scope"));
    assert_eq!(stop.state, "stop_requested");
    assert_eq!(
        store.attach_launch(&id, 1, &launch, wall),
        Err(Fault::Stopped)
    );
    store.maintenance(wall).unwrap();
    assert!(matches!(store.start(&start("b"), wall), Err(Fault::Busy)));
    assert_eq!(
        store.confirm_teardown(&id, 1, "foreign_launch", None),
        Err(Fault::Stopped)
    );
    store.confirm_teardown(&id, 1, &launch, None).unwrap();
    let terminal = store.stop(&id, 1, FenceReason::Stopped).unwrap();
    assert_eq!((terminal.seq, terminal.state.as_str()), (3, "cancelled"));
    drop(store);
    e.link_id = "foreign".into();
    let mut foreign = Store::enroll(dir.path(), &e, wall).unwrap();
    assert_eq!(foreign.read(&id), Err(Fault::NotFound));
    assert_eq!(
        foreign.stop(&id, 2, FenceReason::Stopped),
        Err(Fault::NotFound)
    );
    let other = foreign.start(&start("a"), wall).unwrap().snapshot;
    assert_ne!(id, other.run_id);
    assert_eq!(
        foreign
            .stop(&other.run_id, 1, FenceReason::Stopped)
            .unwrap()
            .state,
        "stop_requested"
    );
    assert_eq!(
        foreign.submit_launch(&other.run_id, 1, wall),
        Err(Fault::Stopped)
    );
    foreign.maintenance(wall).unwrap();
    assert_eq!(foreign.read(&other.run_id).unwrap().state, "cancelled");
}

#[test]
fn local_cancel_advances_while_reverse_permit_is_in_flight() {
    let dir = state_root();
    let wall = wall_ms().unwrap();
    let mut store = Store::enroll(dir.path(), &enrollment(), wall).unwrap();
    let (work, peer, _life) = work_pair();
    let mut session = new_session(&mut store, &work);
    let start = session
        .apply(
            "start_investigation",
            approval(wall, "race", "Public fixture"),
            (wall, wall),
        )
        .unwrap();
    let run = start["run_id"].as_str().unwrap();
    std::thread::scope(|s| {
        let reply = s.spawn(move || {
            let Message::Request { id, op, params, .. } = peer.receive_message(deadline()).unwrap() else { panic!("expected bind"); };
            assert_eq!((op.as_str(), params["run_id"].as_str()), ("bind_session", Some(run)));
            peer.respond(id, Ok(json!({"bound":true})), deadline()).unwrap();
            let Message::Request { id, op, params, .. } = peer.receive_message(deadline()).unwrap() else { panic!("expected reserve"); };
            assert_eq!(op, "reserve_turn");
            assert_eq!(params["body"]["input_digest"], wire::sha256(b"final bytes"));
            peer.send(&Message::Request {v:1,id:"cancel".into(),op:"cancel_investigation".into(),params:json!({"run_id":run,"generation":1}) }, deadline()).unwrap();
            let Message::Success { id: cancel, result, .. } = peer.receive_message(deadline()).unwrap() else { panic!("expected cancel result"); };
            assert_eq!((cancel.as_str(), result["state"].as_str()), ("cancel", Some("stop_requested")));
            peer.respond(id, Ok(json!({"permit_id":"permit","expires_ms":wall_ms().unwrap()+1500,"reserve_digest":params["reserve_digest"]})), deadline()).unwrap();
            // Keep the socket alive until the deciding thread gets its reply.
            peer
        });
        session.bind_at_lite(run).unwrap();
        let launch = session.store.submit_launch(run, 1, wall).unwrap();
        session.store.attach_launch(run, 1, &launch, wall).unwrap();
        assert!(matches!(
            session
                .reserve_turn(run, 1, &launch, b"final bytes")
                .unwrap(),
            Err(Fault::Stopped)
        ));
        let _peer = reply.join().unwrap();
    });
}

#[test]
fn conflicting_permit_fences_bound_lite_after_local_stop_without_waiting_for_ack() {
    let dir = state_root();
    let wall = wall_ms().unwrap();
    let mut store = Store::enroll(dir.path(), &enrollment(), wall).unwrap();
    let start = wire::params(approval(wall, "conflicting_permit", "Public fixture")).unwrap();
    let run = store.start(&start, wall).unwrap().snapshot.run_id;
    let (work, peer, _life) = work_pair();
    let mut session = new_session(&mut store, &work);
    std::thread::scope(|s| {
        let run = run.as_str();
        let exchange = s.spawn(move || {
            let Message::Request { id, op, params, .. } = peer.receive_message(deadline()).unwrap() else { panic!("expected bind"); };
            assert_eq!((op.as_str(), params["run_id"].as_str()), ("bind_session", Some(run)));
            peer.respond(id, Ok(json!({"bound":true})), deadline()).unwrap();
            let Message::Request { id, op, .. } = peer.receive_message(deadline()).unwrap() else { panic!("expected reserve"); };
            assert_eq!(op, "reserve_turn");
            peer.respond(id, Ok(json!({"permit_id":"permit","expires_ms":wall_ms().unwrap()+1500,"reserve_digest":"0".repeat(64)})), deadline()).unwrap();
            let Message::Request { id, op, params, .. } = peer.receive_message(deadline()).unwrap() else { panic!("expected fence"); };
            assert_eq!(op, "fence_session");
            assert_eq!(params, json!({"run_id":run,"generation":1,"reason":"stopped"}));
            // The reverse call must service this read before its fence receipt;
            // it cannot wait for Lite to confirm before committing our stop.
            peer.send(&Message::Request { v:1, id:"before_ack".into(), op:"read_investigation".into(), params:json!({"run_id":run}) }, deadline()).unwrap();
            let Message::Success { id: read, result, .. } = peer.receive_message(deadline()).unwrap() else { panic!("expected snapshot before ack"); };
            assert_eq!(read, "before_ack");
            assert_eq!(result["state"], "stop_requested");
            peer.respond(id, Err(Fault::Unavailable), deadline()).unwrap();
            peer
        });
        session.bind_at_lite(run).unwrap();
        let launch = session.store.submit_launch(run, 1, wall).unwrap();
        session.store.attach_launch(run, 1, &launch, wall).unwrap();
        assert!(
            session
                .reserve_turn(run, 1, &launch, b"final body")
                .is_err()
        );
        let peer = exchange.join().unwrap();
        assert!(matches!(
            session
                .reserve_turn(run, 1, &launch, b"final body")
                .unwrap(),
            Err(Fault::Stopped)
        ));
        assert!(!peer.readable(Instant::now()).unwrap());
    });
}

#[test]
fn authenticated_turn_send_is_once_and_dropped_settlement_ack_replays_only_receipt() {
    let dir = state_root();
    let wall = wall_ms().unwrap();
    let mut e = enrollment();
    let mut store = Store::enroll(dir.path(), &e, wall).unwrap();
    let start = store
        .start(
            &wire::params(approval(wall, "send", "Public fixture")).unwrap(),
            wall,
        )
        .unwrap();
    let run = start.snapshot.run_id;
    store.bind(&run, 1, wall).unwrap();
    let launch = store.submit_launch(&run, 1, wall).unwrap();
    store.attach_launch(&run, 1, &launch, wall).unwrap();
    let (work, peer, _life) = work_pair();
    let mut session = new_session(&mut store, &work);
    let recorded = std::thread::scope(|s| {
        let reply = s.spawn(move || {
            let Message::Request { id, op, params, .. } = peer.receive_message(deadline()).unwrap() else { panic!("expected reserve"); };
            assert_eq!(op, "reserve_turn");
            peer.respond(id, Ok(json!({"permit_id":"permit","expires_ms":wall_ms().unwrap()+1500,"reserve_digest":params["reserve_digest"]})), deadline()).unwrap();
            let Message::Request { op, params: settled, .. } = peer.receive_message(deadline()).unwrap() else { panic!("expected settlement"); };
            assert_eq!(op, "settle_turn");
            assert_eq!(settled["body"]["reserve_digest"], params["reserve_digest"]);
            assert_eq!(settled["body"]["charged_microusd"], 760);
            // Lite records this exact payload, but the ACK never reaches Corral.
            drop(peer);
            settled
        });
        let (request, lease) = session
            .reserve_turn(&run, 1, &launch, b"final body")
            .unwrap()
            .unwrap();
        assert_eq!(
            session
                .send_once(&lease, b"changed", |_| panic!("different digest sent"))
                .unwrap(),
            SendAdmission::<()>::Refused(Fault::Conflict)
        );
        assert_eq!(
            session
                .send_once(&lease, b"final body", |bytes| assert_eq!(
                    bytes,
                    b"final body"
                ))
                .unwrap(),
            SendAdmission::HandedOff(())
        );
        assert_eq!(
            session
                .send_once(&lease, b"final body", |_| panic!(
                    "consumed permit sent again"
                ))
                .unwrap(),
            SendAdmission::<()>::Refused(Fault::Conflict)
        );
        assert!(session.settle_turn(&request, Outcome::Completed).is_err());
        reply.join().unwrap()
    });
    drop(store);
    e.prior_channel_epoch = Some(e.channel_epoch.clone());
    e.channel_epoch = "epoch_B".into();
    let mut store = Store::enroll(dir.path(), &e, wall).unwrap();
    let (work, peer, _life) = work_pair();
    let mut session = new_session(&mut store, &work);
    std::thread::scope(|s| {
        let ack = s.spawn(move || {
            let Message::Request { id, op, params, .. } = peer.receive_message(deadline()).unwrap()
            else {
                panic!("expected replay");
            };
            assert_eq!(op, "settle_turn");
            assert_eq!(params, recorded);
            peer.respond(
                id,
                Ok(json!({"recorded":true,"settle_digest":params["settle_digest"]})),
                deadline(),
            )
            .unwrap();
            peer
        });
        session.replay_settlements().unwrap();
        let peer = ack.join().unwrap();
        assert!(matches!(
            session
                .reserve_turn(&run, 1, &launch, b"final body")
                .unwrap(),
            Err(Fault::Stopped)
        ));
        assert!(!peer.readable(Instant::now()).unwrap());
        assert!(session.store.pending_settlements(wall).unwrap().is_empty());
    });
}

#[test]
fn broker_loss_between_read_and_commit_fences_bootstrap_and_start() {
    let dir = state_root();
    let wall = wall_ms().unwrap();
    let (work, peer, _life) = work_pair();
    let (broker, bootstrap_peer) = UnixStream::pair().unwrap();
    let work = work.with_broker(broker.into());
    let e = enrollment();
    assert!(
        Store::enroll_with(
            dir.path(),
            &e,
            |_| {
                drop(bootstrap_peer);
                Ok(((), wall))
            },
            || work.check_live()
        )
        .is_err()
    );
    // A fresh enrollment under the same identities proves the earlier transaction did not commit.
    let mut store = Store::enroll(dir.path(), &e, wall).unwrap();
    let (broker, steady_peer) = UnixStream::pair().unwrap();
    let work = work.with_broker(broker.into());
    let mut session = new_session(&mut store, &work);
    peer.send(
        &Message::Request {
            v: 1,
            id: "start".into(),
            op: "start_investigation".into(),
            params: approval(wall, "start", "Public fixture"),
        },
        deadline(),
    )
    .unwrap();
    let start = work.receive_message(deadline()).unwrap();
    drop(steady_peer);
    assert!(session.handle(start, deadline()).is_err());
    assert_eq!(
        session
            .store
            .db
            .query_row("SELECT count(*) FROM runs", [], |r| r.get::<_, u64>(0))
            .unwrap(),
        0
    );
    assert!(!peer.readable(Instant::now()).unwrap());
}

#[test]
fn work_rejects_oversized_and_ancillary_frames() {
    use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags, sendmsg};
    use std::io::{ErrorKind, IoSlice, Read as _, Write as _};
    use std::os::fd::AsFd as _;
    let (a, mut b) = UnixStream::pair().unwrap();
    let (life, _hold) = rustix::pipe::pipe().unwrap();
    let work = Work::new(a, life).unwrap();
    b.set_nonblocking(true).unwrap();
    assert!(work.send(&json!({}), Instant::now()).is_err());
    assert_eq!(b.read(&mut [0]).unwrap_err().kind(), ErrorKind::WouldBlock);
    b.write_all(&((wire::MAX_FRAME + 1) as u32).to_be_bytes())
        .unwrap();
    assert!(work.receive(deadline()).is_err());
    let mut buffer = [std::mem::MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut control = SendAncillaryBuffer::new(&mut buffer);
    let extra = [b.as_fd()];
    assert!(control.push(SendAncillaryMessage::ScmRights(&extra)));
    sendmsg(
        &b,
        &[IoSlice::new(b"\0\0\0\x02{}")],
        &mut control,
        SendFlags::NOSIGNAL,
    )
    .unwrap();
    assert!(work.receive(deadline()).is_err());
}
