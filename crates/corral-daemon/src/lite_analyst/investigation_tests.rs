use super::super::{
    clock::wall_ms,
    store::Store,
    tests::{approval, deadline, enrollment, new_session, state_root, work_pair},
    wire::{Fault, Message},
};
use super::*;

#[test]
fn a_forbidden_or_malformed_call_rejects_the_whole_batch_before_dispatch() {
    let mut seen = HashSet::new();
    let item = |id, name, arguments| json!({"id":id,"type":"function","function":{"name":name,"arguments":arguments}});
    let search = item("one", "search_materials", r#"{"query":"public","limit":1}"#);
    for (name, args) in [
        ("read_result", r#"{"result":"known_foreign_handle"}"#),
        ("bash", r#"{"command":"unused"}"#),
        (
            "read_evidence",
            r#"{"source":"ok","start_byte":0,"max_bytes":1,"path":"/state"}"#,
        ),
        (
            "search_materials",
            r#"{"query":"public","limit":1,"limit":2}"#,
        ),
    ] {
        assert!(
            plan_calls(
                &json!({"tool_calls":[search,item("two",name,args)]}),
                &mut seen
            )
            .is_err()
        );
        assert!(seen.is_empty());
    }
    assert!(plan_calls(&json!({"tool_calls":[search]}), &mut seen).is_ok());
    assert!(plan_calls(&json!({"tool_calls":[search]}), &mut seen).is_err());
}

#[test]
fn pi_eof_after_consumption_blocks_provider_and_settles_once_without_replay() {
    let dir = state_root();
    let wall = wall_ms().unwrap();
    let mut enrollment = enrollment();
    let mut store = Store::enroll(dir.path(), &enrollment, wall).unwrap();
    let start = wire::params(approval(wall, "pi_eof", "Public fixture")).unwrap();
    let run = store.start(&start, wall).unwrap().snapshot.run_id;
    store.bind(&run, 1, wall).unwrap();
    let launch = store.submit_launch(&run, 1, wall).unwrap();
    store.attach_launch(&run, 1, &launch, wall).unwrap();
    let ledger = rusqlite::Connection::open_with_flags(
        dir.path().join("lite-analyst.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let (socket, pi) = UnixStream::pair().unwrap();
    let io = PiIo {
        io: socket,
        buffer: Vec::new(),
    };
    let (work, peer, _life) = work_pair();
    let mut session = new_session(&mut store, &work);
    std::thread::scope(|scope| {
        let lite = scope.spawn(move || {
            let Message::Request { id, op, params, .. } = peer.receive_message(deadline()).unwrap()
            else {
                panic!("expected reserve request");
            };
            assert_eq!(op, "reserve_turn");
            peer.respond(
                id,
                Ok(
                    json!({"permit_id":"test_permit", "expires_ms":wall_ms().unwrap()+4000,
                "reserve_digest":params["reserve_digest"]}),
                ),
                deadline(),
            )
            .unwrap();
            let Message::Request {
                id,
                op,
                params: settled,
                ..
            } = peer.receive_message(deadline()).unwrap()
            else {
                panic!("expected settlement request");
            };
            assert_eq!(op, "settle_turn");
            assert_eq!(settled["body"]["reserve_digest"], params["reserve_digest"]);
            peer.respond(
                id,
                Ok(json!({"recorded":true, "settle_digest":settled["settle_digest"]})),
                deadline(),
            )
            .unwrap();
            (peer, settled)
        });
        let body = b"exact fixture body";
        let (reserve, lease) = session
            .reserve_turn(&run, 1, &launch, body)
            .unwrap()
            .unwrap();
        io.check_waiting().unwrap();
        let mut writes = 0;
        let admission = session.send_once(
            &lease,
            body,
            || {
                // A second SQLite connection can see consumption only after the
                // commit. Queue real Pi EOF at that exact point, with Work still live.
                let consumed = ledger
                    .query_row("SELECT consumed FROM turn_ledger", [], |row| {
                        row.get::<_, bool>(0)
                    })
                    .unwrap();
                assert!(consumed);
                drop(pi);
                io.check_waiting()
            },
            |_| {
                writes += 1;
                Ok(())
            },
        );
        let result = session
            .settle_exchange(&reserve, admission, |_| Ok(()))
            .unwrap();
        let (peer, settled) = lite.join().unwrap();
        assert_eq!(writes, 0, "provider write after Pi EOF");
        assert!(result.is_err());
        assert_eq!(settled["body"]["outcome"], "indeterminate");
        assert_eq!(settled["body"]["charged_microusd"], 1000);
        assert!(!peer.readable(Instant::now()).unwrap());
        assert!(matches!(
            session
                .send_once(&lease, body, || Ok(()), |_| panic!("reused consumed lease"))
                .unwrap(),
            SendAdmission::<()>::Refused(Fault::Conflict)
        ));
    });
    drop(ledger);
    drop(store);
    enrollment.prior_channel_epoch = Some(enrollment.channel_epoch.clone());
    enrollment.channel_epoch = "epoch_B".into();
    let mut store = Store::enroll(dir.path(), &enrollment, wall_ms().unwrap()).unwrap();
    let (work, peer, _life) = work_pair();
    let mut session = new_session(&mut store, &work);
    session.replay_settlements().unwrap();
    assert!(matches!(
        session
            .reserve_turn(&run, 1, &launch, b"exact fixture body")
            .unwrap(),
        Err(Fault::Stopped)
    ));
    assert!(!peer.readable(Instant::now()).unwrap());
}

#[test]
fn report_draft_accepts_only_same_generation_citations_at_utf8_boundaries() {
    let issued = HashSet::from(["current".to_owned()]);
    let report = |start, handle| json!({"text":"é!","citations":[{"start_byte":start,"end_byte":2,"handle":handle}]});
    assert!(finish_draft(report(0, "current"), &issued).is_ok());
    assert!(finish_draft(report(1, "current"), &issued).is_err());
    assert!(finish_draft(report(0, "foreign"), &issued).is_err());
}
