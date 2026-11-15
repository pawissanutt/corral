use super::*;
use crate::tests::Fixture;
use corral_protocol::frame::Frame;

#[derive(Default)]
struct Capture(Vec<Frame>);

impl conn::Conn for Capture {
    fn send_frame(&mut self, frame: &Frame) -> Result<()> {
        self.0.push(frame.clone());
        Ok(())
    }
}

fn setup(fixture: &Fixture) -> (Arc<Shared>, Arc<Devices>, DeviceInfo) {
    let shared = Arc::new(Shared::new());
    start(&fixture.ctx, &shared, None).unwrap();
    let devices = shared.devices().unwrap();
    let admin = devices.enrol_native("Owner", "test", "test").unwrap();
    (shared, devices, admin)
}

#[test]
fn web_admin_can_edit_authority_but_cannot_raise_its_own_authority() {
    let fixture = Fixture::new();
    let (shared, devices, admin) = setup(&fixture);
    let web = devices
        .enrol("Browser", "test", Some(&[7; 32]), "test")
        .unwrap();
    let (_, mut authority) = devices.authenticated(&web.id).unwrap();
    authority.admin = false;
    authority.act = false;
    devices
        .set_authority(
            &admin.id,
            DeviceSetAuthority {
                device: web.id.clone(),
                authority: authority.clone(),
            },
        )
        .unwrap();
    authority.admin = true;
    devices
        .set_authority(
            &admin.id,
            DeviceSetAuthority {
                device: web.id.clone(),
                authority: authority.clone(),
            },
        )
        .unwrap();
    let (_, mut target) = devices.authenticated(&admin.id).unwrap();
    target.assistant_act = false;
    devices
        .set_authority(
            &web.id,
            DeviceSetAuthority {
                device: admin.id.clone(),
                authority: target.clone(),
            },
        )
        .unwrap();
    assert_eq!(devices.authenticated(&admin.id).unwrap().1, target);
    authority.act = true;
    assert!(matches!(
        devices.set_authority(
            &web.id,
            DeviceSetAuthority {
                device: web.id.clone(),
                authority,
            }
        ),
        Err(DeviceError::Refused("cannot_raise_own_authority"))
    ));
    assert!(!devices.authenticated(&web.id).unwrap().1.act);
    shared.close();
}

#[test]
fn authority_changes_notify_every_connected_target_session_without_reconnect() {
    let fixture = Fixture::new();
    let (shared, devices, admin) = setup(&fixture);
    let web = devices
        .enrol("Browser", "test", Some(&[9; 32]), "test")
        .unwrap();
    let (_, first) = devices.attach_key(&[9; 32]).unwrap();
    let (_, second) = devices.attach_key(&[9; 32]).unwrap();
    let (_, other) = devices.attach(&admin.id).unwrap();
    let mut frames = Capture::default();
    first.flush_authority(&mut frames).unwrap();
    assert!(frames.0.is_empty());
    let (_, mut authority) = devices.authenticated(&web.id).unwrap();
    for (index, act) in [false, true].into_iter().enumerate() {
        authority.act = act;
        devices
            .set_authority(
                &admin.id,
                DeviceSetAuthority {
                    device: web.id.clone(),
                    authority: authority.clone(),
                },
            )
            .unwrap();
        for session in [&first, &second] {
            frames.0.clear();
            session.flush_authority(&mut frames).unwrap();
            assert_eq!(frames.0.len(), 1);
            let notification: Value = serde_json::from_slice(&frames.0[0].payload).unwrap();
            assert_eq!(notification["method"], SESSION_AUTHORITY_CHANGED);
            assert_eq!(notification["params"]["device"], web.id);
            assert_eq!(notification["params"]["revision"], (index + 1) as u64);
            session.flush_authority(&mut frames).unwrap();
            assert_eq!(frames.0.len(), 1);
        }
        frames.0.clear();
        other.flush_authority(&mut frames).unwrap();
        assert!(frames.0.is_empty());
        assert_eq!(devices.authenticated(&web.id).unwrap().1.act, act);
    }
    shared.close();
}
