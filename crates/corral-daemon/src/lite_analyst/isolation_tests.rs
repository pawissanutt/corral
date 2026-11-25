use super::*;
use rustix::{
    io::Errno,
    net::{self, AddressFamily, SocketFlags, SocketType, ipproto},
};
use std::io::Read;

#[test]
fn supervisor_denies_internet_udp_raw_and_other_families_and_payload_denies_all_sockets() {
    // The seccomp modes layer only on this thread. No packet is sent to an
    // address on the host or to an external network.
    thread::spawn(|| {
        let (mut client, mut server) = UnixStream::pair().unwrap();
        rustix::thread::set_no_new_privs(true).unwrap();
        install_filter(&socket_filter(true)).unwrap();
        let open = |domain, kind, protocol| {
            net::socket_with(domain, kind, SocketFlags::CLOEXEC, protocol).map(drop)
        };
        assert_eq!(
            open(AddressFamily::INET, SocketType::STREAM, Some(ipproto::TCP)),
            Ok(())
        );
        for (domain, kind, protocol) in [
            (AddressFamily::INET, SocketType::DGRAM, Some(ipproto::UDP)),
            (AddressFamily::INET, SocketType::RAW, Some(ipproto::TCP)),
            (AddressFamily::INET6, SocketType::STREAM, Some(ipproto::TCP)),
            (AddressFamily::VSOCK, SocketType::STREAM, None),
        ] {
            assert_eq!(open(domain, kind, protocol), Err(Errno::PERM));
        }
        install_filter(&socket_filter(false)).unwrap();
        assert_eq!(
            open(AddressFamily::INET, SocketType::STREAM, None),
            Err(Errno::PERM)
        );
        assert_eq!(
            open(AddressFamily::UNIX, SocketType::DGRAM, None),
            Err(Errno::PERM)
        );
        assert_eq!(
            UnixStream::pair().unwrap_err().raw_os_error(),
            Some(libc::EPERM)
        );
        client.write_all(b"x").unwrap();
        let mut received = [0];
        server.read_exact(&mut received).unwrap();
        assert_eq!(received, *b"x");
    })
    .join()
    .unwrap();
}
