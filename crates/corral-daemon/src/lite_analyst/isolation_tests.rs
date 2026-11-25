use super::*;
use rustix::{
    io::Errno,
    net::{self, AddressFamily, SocketFlags, SocketType, ipproto},
};

#[test]
fn namespace_init_must_be_pid_one_and_a_direct_wrapper_child() {
    let status = "Name:\tbwrap\nPPid:\t83\nNSpid:\t84\t1\n";
    assert!(is_direct_init(status, 83, 84));
    assert!(!is_direct_init(status, 82, 84));
    assert!(!is_direct_init(status, 83, 85));
    for unexpected in ["84", "84 2", "84 1 1", "84 1 rubbish"] {
        assert!(!is_direct_init(
            &format!("PPid:\t83\nNSpid:\t{unexpected}\n"),
            83,
            84
        ));
    }
}

#[test]
fn interrupted_info_read_preserves_evidence_for_cleanup() {
    let (reader, writer) = pipe_with(PipeFlags::CLOEXEC).unwrap();
    fcntl_setfl(&reader, OFlags::NONBLOCK).unwrap();
    let mut info = InfoPipe {
        reader: reader.into(),
        raw: Vec::new(),
    };
    let mut writer = File::from(writer);
    writer.write_all(b"{\"child-pid\":84,").unwrap();
    let mut calls = 0;
    let deadline = Instant::now() + Duration::from_secs(1);
    assert!(
        info.read(deadline, &mut || {
            calls += 1;
            ensure!(calls == 1, "fixture cancelled");
            Ok(())
        })
        .is_err()
    );
    writer
        .write_all(b"\"pid-namespace\":1,\"mnt-namespace\":2,\"net-namespace\":3}")
        .unwrap();
    drop(writer);
    let parsed = info.read(deadline, &mut || Ok(())).unwrap();
    assert_eq!((parsed.child_pid, parsed.net_namespace), (84, 3));
}

fn exited_wrapper() -> Sandbox {
    let mut wrapper = Command::new("/bin/true").spawn().unwrap();
    assert!(wrapper.wait().unwrap().success());
    let (reader, writer) = pipe_with(PipeFlags::CLOEXEC).unwrap();
    drop(writer);
    Sandbox {
        wrapper,
        init: None,
        gate: None,
        info: InfoPipe {
            reader: reader.into(),
            raw: Vec::new(),
        },
    }
}

#[test]
fn successful_wrapper_exit_without_verified_init_quarantines() {
    let mut sandbox = exited_wrapper();
    assert!(sandbox.reap(Instant::now()).is_err());
}

#[test]
fn successful_wrapper_exit_does_not_prove_exit_of_retained_init() {
    // Stand in a waitable test child for the already-verified namespace init.
    let mut sandbox = exited_wrapper();
    let mut child = Command::new("/bin/sleep").arg("4").spawn().unwrap();
    let pid = Pid::from_raw(child.id().try_into().unwrap()).unwrap();
    let result = pidfd_open(pid, PidfdFlags::empty());
    if result.is_ok() {
        sandbox.init = result.ok();
        assert!(!sandbox.wait_teardown(Instant::now()));
    }
    let reaped = sandbox.reap(Instant::now());
    let init_exited = sandbox
        .init
        .as_ref()
        .is_some_and(|fd| pidfd_exited(fd).unwrap());
    let _ = child.kill();
    child.wait().unwrap();
    // Retained-handle force escalation is accepted only after independent
    // exit observations. Do not mask unsupported pidfds in this fixture.
    assert!(reaped.is_ok());
    assert!(init_exited);
}

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
