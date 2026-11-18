use super::*;

#[test]
fn paired_device_keeps_its_key_reach_and_seen_cursor_across_reopen() {
    let root = tempfile::tempdir().unwrap();
    let state_root = root.path().join(".corral-next");
    let key = [7; 32];
    let (attached, seen, public) = {
        let devices = Arc::new(Devices::open(&state_root).unwrap());
        devices.set_listener("127.0.0.1:7841".parse().unwrap(), &[], None);
        let now = Instant::now();
        let offer = devices.pair_begin(now).unwrap();
        devices
            .pairings
            .handshaken(&offer.pair_id, key, "123456".into(), now)
            .unwrap();
        devices
            .pairings
            .claim(&offer.pair_id, "Browser".into(), "test".into(), now)
            .unwrap();
        let paired = devices.pair_confirm(&offer.pair_id).unwrap();
        let (attached, _) = devices.attach_key(&key, DeviceReach::Lan).unwrap();
        assert_eq!(paired.id, attached.id);
        let seen = devices.advance_seen(&paired.id, 42).unwrap();
        assert_eq!(devices.advance_seen(&paired.id, 38).unwrap(), seen);
        (
            attached,
            seen,
            PairCode::decode(&offer.code).unwrap().daemon_public,
        )
    };

    let reopened = Arc::new(Devices::open(&state_root).unwrap());
    assert_eq!(reopened.identity().unwrap().public, public);
    assert_eq!(reopened.list().unwrap(), vec![attached.clone()]);
    assert_eq!(reopened.seen(&attached.id).unwrap(), seen);
    let (device, guard) = reopened.attach_key(&key, DeviceReach::Lan).unwrap();
    assert_eq!(device.id, attached.id);
    assert_eq!(guard.reach, DeviceReach::Lan);
}

#[test]
fn revocation_marks_all_active_sessions_for_close_and_prevents_reattach() {
    let root = tempfile::tempdir().unwrap();
    let devices = Arc::new(Devices::open(root.path()).unwrap());
    let native = devices.enrol_native("Native", "test").unwrap();
    let (_, other) = devices.attach(&native.id).unwrap();
    let key = [9; 32];
    let web = devices.enrol("Web", "test", Some(&key)).unwrap();
    let (_, first) = devices.attach_key(&key, DeviceReach::Lan).unwrap();
    let (_, second) = devices.attach_key(&key, DeviceReach::Loopback).unwrap();

    let revoked = devices.revoke(&web.id).unwrap();
    assert!(revoked.revoked_at.is_some());
    assert!(first.revoked.load(Ordering::Acquire));
    assert!(second.revoked.load(Ordering::Acquire));
    assert!(!other.revoked.load(Ordering::Acquire));
    assert!(matches!(
        devices.authenticated(&web.id),
        Err(DeviceError::Revoked)
    ));
    drop((first, second, other, devices));

    let reopened = Arc::new(Devices::open(root.path()).unwrap());
    assert!(matches!(
        reopened.authenticated(&web.id),
        Err(DeviceError::Revoked)
    ));
    assert!(reopened.attach_key(&key, DeviceReach::Lan).is_err());
}
