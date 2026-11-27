use super::*;
use rustix::net::{SocketFlags, socketpair};
use std::io::{Write, pipe};
use std::os::unix::net::UnixListener;

const REVIEWED_PROFILE: &[u8] =
    include_bytes!("../../../../docs/contracts/lite-analyst-launcher-v1/launcher-v1.json");
const TEMPLATE: &[u8] = include_bytes!(
    "../../../../docs/contracts/lite-analyst-launcher-v1/build-manifest.example.json"
);

fn enrollment_record() -> Value {
    json!({"v": 1, "type": "enrollment", "role": "corral",
        "link_id": URL_SAFE_NO_PAD.encode([1; 16]), "lite_instance": URL_SAFE_NO_PAD.encode([2; 16]),
        "corral_instance": URL_SAFE_NO_PAD.encode([3; 16]), "channel_epoch": URL_SAFE_NO_PAD.encode([4; 16]),
        "prior_channel_epoch": null, "profile_sha256": PROFILE_SHA256,
        "corral_manifest_sha256": sha256(b"manifest"), "joint_lock_sha256": sha256(b"joint lock")})
}

fn enrollment(value: &Value) -> Result<Enrollment> {
    parse_enrollment(&serde_json::to_vec(value).unwrap())
}

#[test]
fn enrollment_is_closed_and_requires_the_prior_epoch_field() {
    let value = enrollment_record();
    assert!(enrollment(&value).unwrap().prior_channel_epoch.is_none());
    let mut changed = value.clone();
    changed["prior_channel_epoch"] = URL_SAFE_NO_PAD.encode([5; 16]).into();
    assert!(enrollment(&changed).unwrap().prior_channel_epoch.is_some());
    changed["prior_channel_epoch"] = value["channel_epoch"].clone();
    assert!(enrollment(&changed).is_err());
    changed
        .as_object_mut()
        .unwrap()
        .remove("prior_channel_epoch");
    assert!(enrollment(&changed).is_err());
    for (key, bad) in [
        ("unexpected", Value::Null),
        ("role", "lite".into()),
        ("link_id", "example_link".into()),
        ("joint_lock_sha256", "3".repeat(64).into()),
    ] {
        let mut changed = value.clone();
        changed[key] = bad;
        assert!(enrollment(&changed).is_err(), "accepted {key}");
    }
    let duplicate = format!("{{\"v\":1,{}", &value.to_string()[1..]);
    assert!(parse_enrollment(duplicate.as_bytes()).is_err());
}

#[test]
fn peer_link_ready_must_echo_the_private_binding_not_the_local_role() {
    let expected = enrollment(&enrollment_record()).unwrap();
    let local = expected.link_ready();
    assert_eq!(local["role"], "corral");
    assert!(expected.check_link_ready(local.clone()).is_err());
    let mut counterpart = local;
    counterpart["role"] = "lite".into();
    expected.check_link_ready(counterpart.clone()).unwrap();
    let mut wrong_pin = counterpart.clone();
    wrong_pin["joint_lock_sha256"] = sha256(b"other lock").into();
    assert!(expected.check_link_ready(wrong_pin).is_err());
    counterpart["unexpected"] = true.into();
    assert!(expected.check_link_ready(counterpart).is_err());
}

fn runnable_metadata() -> Value {
    let mut manifest: Value = decode(TEMPLATE).unwrap();
    manifest["status"] = "runnable".into();
    manifest["corral_revision"] = "0123456789012345678901234567890123456789".into();
    manifest["host_requirements"]["sysroot_abi"] = "pinned-test-abi".into();
    for (name, component) in manifest["components"].as_object_mut().unwrap() {
        if component["sha256"].as_str().unwrap() == "0".repeat(64) {
            component["sha256"] = sha256(name.as_bytes()).into();
        }
    }
    #[cfg(feature = "lite-analyst-faults")]
    {
        use super::super::faults::{CONTRACT, MANIFEST_PATH};
        manifest["components"][MANIFEST_PATH] = json!({
            "role":"asset", "executable":false, "bytes": CONTRACT.len(), "sha256":sha256(CONTRACT)
        });
    }
    manifest
}

#[test]
fn matching_raw_digests_do_not_make_an_unreviewed_profile_or_bad_build_valid() {
    let mut expected = enrollment(&enrollment_record()).unwrap();
    let manifest = runnable_metadata();
    let verify = |enrollment: &mut Enrollment, bytes: &[u8]| {
        enrollment.corral_manifest_sha256 = sha256(bytes);
        check_pins(enrollment, REVIEWED_PROFILE, bytes)
    };
    let bytes = serde_json::to_vec(&manifest).unwrap();
    assert!(check_pins(&expected, REVIEWED_PROFILE, &bytes).is_err());
    verify(&mut expected, &bytes).unwrap();
    let mut other_variant = manifest.clone();
    let components = other_variant["components"].as_object_mut().unwrap();
    let marker = "provenance/corral-fault-v1.md";
    if cfg!(feature = "lite-analyst-faults") {
        components.remove(marker);
    } else {
        components.insert(
            marker.into(),
            json!({
                "role": "asset", "executable": false, "bytes": 8, "sha256": sha256(b"contract")
            }),
        );
    }
    assert!(verify(&mut expected, &serde_json::to_vec(&other_variant).unwrap()).is_err());
    assert!(verify(&mut expected, TEMPLATE).is_err());
    for (entry, field, changed) in [
        ("bin/corrald-next", "executable", false.into()),
        ("bin/corrald-next", "sha256", "0".repeat(64).into()),
        ("bin/corrald-next", "other", true.into()),
        (
            "assets/tool-policy.json",
            "sha256",
            TOOL_POLICY_SHA256.into(),
        ),
    ] {
        let mut changed_manifest = manifest.clone();
        changed_manifest["components"][entry][field] = changed;
        assert!(
            verify(
                &mut expected,
                &serde_json::to_vec(&changed_manifest).unwrap()
            )
            .is_err()
        );
    }
    let mut other_profile = REVIEWED_PROFILE.to_vec();
    other_profile.push(b'\n');
    expected.profile_sha256 = sha256(&other_profile);
    expected.corral_manifest_sha256 = sha256(&bytes);
    assert!(check_pins(&expected, &other_profile, &bytes).is_err());
}

fn broker_pair() -> (OwnedFd, OwnedFd) {
    socketpair(
        AddressFamily::UNIX,
        SocketType::SEQPACKET,
        SocketFlags::CLOEXEC,
        None,
    )
    .unwrap()
}

#[test]
fn descriptor_purposes_require_distinct_live_unnamed_endpoints_and_pipe_directions() {
    let (work, _peer) = UnixStream::pair().unwrap();
    let work = OwnedFd::from(work);
    let (broker, _peer) = broker_pair();
    check_socket(&work, SocketType::STREAM).unwrap();
    check_socket(&broker, SocketType::SEQPACKET).unwrap();
    assert!(check_socket(&broker, SocketType::STREAM).is_err());
    let alias = work.try_clone().unwrap();
    check_distinct(&[&work, &broker]).unwrap();
    assert!(check_distinct(&[&work, &alias]).is_err());

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("peer.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let client = UnixStream::connect(&path).unwrap();
    assert!(check_socket(&OwnedFd::from(client), SocketType::STREAM).is_err());
    assert!(check_socket(&OwnedFd::from(listener), SocketType::STREAM).is_err());

    let (read, write) = pipe().unwrap();
    let (read, write) = (OwnedFd::from(read), OwnedFd::from(write));
    check_pipe(&read, OFlags::RDONLY).unwrap();
    check_pipe(&write, OFlags::WRONLY).unwrap();
    assert!(check_pipe(&read, OFlags::WRONLY).is_err());
    assert!(check_pipe(&work, OFlags::RDONLY).is_err());
}

fn wait_for_pipe_hangup(reader: &impl AsFd) {
    // A concurrent fork can retain the CLOEXEC writer until exec. Wait for
    // actual EOF, not just readable bytes, without consuming the record.
    let mut fds = [PollFd::new(reader, PollFlags::empty())];
    let timeout = Timespec {
        tv_sec: BOOTSTRAP_TIMEOUT.as_secs() as _,
        tv_nsec: BOOTSTRAP_TIMEOUT.subsec_nanos().into(),
    };
    poll(&mut fds, Some(&timeout)).unwrap();
    assert!(fds[0].revents().contains(PollFlags::HUP));
}

#[test]
fn pipe_records_require_bounded_single_lf_and_live_driver() {
    let (life, driver) = pipe().unwrap();
    let life = OwnedFd::from(life);
    let (broker, _peer) = broker_pair();
    let scope = Scope {
        lifetime: &life,
        broker: &broker,
        deadline: Instant::now() + BOOTSTRAP_TIMEOUT,
    };
    let line = |bytes: &[u8]| {
        let (read, mut write) = pipe().unwrap();
        write.write_all(bytes).unwrap();
        drop(write);
        let read = OwnedFd::from(read);
        check_pipe(&read, OFlags::RDONLY).unwrap();
        scope.line(read, CREDENTIAL_LEN + 1)
    };
    let credential = format!("{}\n", sha256(b"credential"));
    check_credential(&line(credential.as_bytes()).unwrap()).unwrap();
    assert!(check_credential(b"invalid").is_err());
    assert!(line(credential.trim_end().as_bytes()).is_err());
    assert!(line(format!("{credential}\n").as_bytes()).is_err());
    assert!(line(b"credential\ntrailing\n").is_err());
    let expired = Scope {
        deadline: Instant::now(),
        ..scope
    };
    let (unwritten, _writer) = pipe().unwrap();
    assert!(expired.line(OwnedFd::from(unwritten), PIPE_MAX).is_err());

    drop(driver);
    wait_for_pipe_hangup(&life);
    assert!(line(credential.as_bytes()).is_err());
}

#[test]
fn ready_writes_once_then_closes_and_keeps_broker_fenced() {
    let expected = enrollment(&enrollment_record()).unwrap();
    let (broker, peer) = broker_pair();
    let (lifetime, _driver) = pipe().unwrap();
    let (mut driver_ready, ready) = pipe().unwrap();
    let ready = OwnedFd::from(ready);
    check_pipe(&ready, OFlags::WRONLY).unwrap();
    let mut resources = Resources {
        broker,
        credential: vec![],
        deadline: Instant::now() + BOOTSTRAP_TIMEOUT,
        pending_ready: Some(PendingReady {
            pipe: ready,
            lifetime: lifetime.into(),
        }),
    };
    resources.check_broker().unwrap();
    resources.ready(&expected, || Ok(())).unwrap();
    assert!(resources.ready(&expected, || Ok(())).is_err());
    wait_for_pipe_hangup(&driver_ready);
    let mut bytes = Vec::new();
    driver_ready.read_to_end(&mut bytes).unwrap();
    let raw = bytes.strip_suffix(b"\n").unwrap();
    assert!(!raw.contains(&b'\n'));
    assert_eq!(
        decode::<Value>(raw).unwrap(),
        json!({"v":1,"type":"ready","role":"corral",
        "link_id":expected.link_id,"channel_epoch":expected.channel_epoch,
        "profile_sha256":expected.profile_sha256,"corral_manifest_sha256":expected.corral_manifest_sha256,
        "joint_lock_sha256":expected.joint_lock_sha256})
    );
    let (mut driver_ready, ready) = pipe().unwrap();
    let (lifetime, _driver) = pipe().unwrap();
    resources.pending_ready = Some(PendingReady {
        pipe: ready.into(),
        lifetime: lifetime.into(),
    });
    assert!(
        resources
            .ready(&expected, || anyhow::bail!("work EOF before ready"))
            .is_err()
    );
    wait_for_pipe_hangup(&driver_ready);
    let mut bytes = Vec::new();
    driver_ready.read_to_end(&mut bytes).unwrap();
    assert!(bytes.is_empty());
    net::shutdown(&peer, net::Shutdown::Both).unwrap();
    assert!(resources.check_broker().is_err());
}
