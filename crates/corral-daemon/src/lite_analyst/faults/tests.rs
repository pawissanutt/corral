use super::*;
use rustix::net::{
    AddressFamily, SendAncillaryBuffer, SendAncillaryMessage, SocketFlags, SocketType, recv,
    sendmsg, socketpair,
};
use std::{io::IoSlice, mem::MaybeUninit, os::fd::AsFd};

#[test]
fn selected_barrier_is_one_packet_one_use_and_fails_closed() {
    for (action, rights) in [
        (Some(b"drop\n".as_slice()), false),
        (Some(b"continue\n"), false),
        (Some(b"drop\nextra bytes"), false),
        (None, false),
        (Some(b"drop\n"), true),
    ] {
        let (fd, driver) = socketpair(
            AddressFamily::UNIX,
            SocketType::SEQPACKET,
            SocketFlags::CLOEXEC,
            None,
        )
        .unwrap();
        let control = FaultControl::new(Case::StartCommitted, fd);
        let deadline = Instant::now() + Duration::from_secs(1);
        assert!(
            !control
                .reach(Case::AckCommitted, deadline, || Ok(()))
                .unwrap()
        );
        std::thread::scope(|scope| {
            scope.spawn(move || {
                let mut bytes = [0u8; 64];
                let (_, len) = recv(&driver, &mut bytes[..], RecvFlags::empty()).unwrap();
                assert_eq!(&bytes[..len], b"corral.start.committed\n");
                if let Some(action) = action {
                    let mut buffer = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
                    let mut ancillary = SendAncillaryBuffer::new(&mut buffer);
                    let passed = [driver.as_fd()];
                    if rights {
                        assert!(ancillary.push(SendAncillaryMessage::ScmRights(&passed)));
                    }
                    assert_eq!(
                        sendmsg(
                            &driver,
                            &[IoSlice::new(action)],
                            &mut ancillary,
                            SendFlags::NOSIGNAL
                        )
                        .unwrap(),
                        action.len()
                    );
                }
            });
            assert_eq!(
                control
                    .reach(Case::StartCommitted, deadline, || Ok(()))
                    .ok(),
                (action == Some(b"drop\n") && !rights).then_some(true)
            );
        });
        assert!(!control.armed(Case::StartCommitted));
        assert!(
            !control
                .reach(Case::StartCommitted, deadline, || Ok(()))
                .unwrap()
        );
    }
}

#[test]
fn duplicate_requires_matching_receipt_and_replays_the_same_frame() {
    use super::super::{
        Store,
        clock::test_wall_ms,
        tests::{deadline, enrollment, new_session, state_root, work_pair},
        turns::SettleTurn,
        wire::{self, Message},
    };
    use serde_json::{Value, json};

    let vectors: Value = serde_json::from_str(include_str!(
        "../../../../../docs/contracts/lite-analyst-v1/vectors.json"
    ))
    .unwrap();
    let vector = &vectors["settlement"]["completed"];
    let settlement: SettleTurn =
        wire::params(json!({"body":vector["body"],"settle_digest":vector["sha256"]})).unwrap();
    for good in [false, true] {
        let state = state_root();
        let mut store = Store::enroll(state.path(), &enrollment(), test_wall_ms()).unwrap();
        let (work, peer, _life) = work_pair();
        let (fd, driver) = socketpair(
            AddressFamily::UNIX,
            SocketType::SEQPACKET,
            SocketFlags::CLOEXEC,
            None,
        )
        .unwrap();
        let work = work.with_faults(Some(FaultControl::new(Case::SettleDuplicate, fd)));
        let mut session = new_session(&mut store, &work);
        let end = deadline();
        std::thread::scope(|scope| {
            let digest = settlement.settle_digest.clone();
            let peer = scope.spawn(move || {
                let raw = peer.receive(end).unwrap();
                let Message::Request {id, op, ..} = Message::decode(&raw).unwrap() else { panic!("expected settle request"); };
                assert_eq!(op, "settle_turn");
                let reply = json!({"recorded":true,"settle_digest": if good { digest } else { "1".repeat(64) }});
                peer.respond(id.clone(), Ok(reply.clone()), end).unwrap();
                if good {
                    wait(&driver, PollFlags::IN, end, &mut || Ok(())).unwrap();
                    let mut bytes = [0u8; 64];
                    let (_, len) = recv(&driver, &mut bytes[..], RecvFlags::empty()).unwrap();
                    assert_eq!(&bytes[..len], b"corral.settle.duplicate\n");
                    send(&driver, b"continue\n", SendFlags::NOSIGNAL).unwrap();
                    assert_eq!(raw, peer.receive(end).unwrap());
                    peer.respond(id, Ok(reply), end).unwrap();
                }
                peer
            });
            let result = session.duplicate_settlement(&settlement, end);
            assert_eq!(result.is_ok_and(|r| r.is_ok()), good);
            assert_eq!(work.fault_armed(Case::SettleDuplicate), !good);
            assert!(!peer.join().unwrap().readable(Instant::now()).unwrap());
        });
    }
}
