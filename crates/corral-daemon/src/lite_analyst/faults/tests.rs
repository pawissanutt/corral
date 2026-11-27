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
