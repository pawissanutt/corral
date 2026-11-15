use super::*;
use crate::model::daemon::{Feed, Link};
use corral_protocol::app::{ActClass, ActPreview, AppSession, DeviceAuthority, DeviceSetAuthority};
use corral_protocol::device::DeviceInfo;

#[test]
fn authority_notification_retires_unsent_confirmations_and_recomputes_gates() {
    let authority = DeviceAuthority {
        reply: true,
        act: true,
        assistant: true,
        assistant_act: true,
        admin: true,
    };
    let device: DeviceInfo = serde_json::from_value(serde_json::json!({
        "id": "device-1", "name": "Desk", "platform": "native",
        "app_version": "1", "enrolled_at": "2026-11-15T00:00:00Z", "revoked_at": null
    }))
    .unwrap();
    let mut feed = Feed {
        authority_revision: 7,
        transport_ready: true,
        projection_ready: true,
        link: Link::Up,
        session: Some(AppSession {
            device,
            authority: authority.clone(),
            seen: Default::default(),
            server_time: "2026-11-15T00:00:00Z".into(),
        }),
        ..Feed::default()
    };
    let subject = Confirmable::DeviceAuthority {
        key: Id::new("change-1"),
        title: "Grant admin".into(),
        change: DeviceSetAuthority {
            device: "device-2".into(),
            authority,
        },
    };
    let mut pending = Some(Confirmation {
        subject: subject.clone(),
        preview: Id::new("preview-op"),
        ready: Some(ActPreview {
            preview_id: "proof-1".into(),
            digest: "digest-1".into(),
            class: ActClass::Admin,
            effects: vec!["Grant admin to device-2".into()],
            expires_at: "2026-11-15T00:01:00Z".into(),
            operation_token: None,
        }),
    });
    let mut watching = BTreeMap::from([
        (subject.key(), (subject.clone(), Watch::Confirming)),
        (
            Id::new("another-preview"),
            (subject.clone(), Watch::Confirming),
        ),
        (Id::new("sent"), (subject.clone(), Watch::Sent)),
        (Id::new("confirmed"), (subject, Watch::Confirmed)),
    ]);
    assert!(super::super::live::granted(&feed, Authority::Act));
    feed.session.as_mut().unwrap().authority.act = false;
    assert!(super::super::live::granted(&feed, Authority::Admin));
    assert!(!super::super::live::granted(&feed, Authority::Act));
    assert_eq!(super::super::devices::admin_refusal(&feed), None);
    retire_authority(7, feed.authority_revision, &mut pending, &mut watching);
    assert!(pending.is_some());
    assert_eq!(watching.len(), 4);

    // The notification advances the revision; APP_SESSION supplies the new authority.
    feed.authority_revision = 8;
    feed.session.as_mut().unwrap().authority = DeviceAuthority::default();
    retire_authority(7, feed.authority_revision, &mut pending, &mut watching);
    assert!(pending.is_none());
    assert_eq!(watching.len(), 2);
    assert_eq!(watching.get("sent").unwrap().1, Watch::Sent);
    assert_eq!(watching.get("confirmed").unwrap().1, Watch::Confirmed);
    assert!(!super::super::live::granted(&feed, Authority::Act));
    assert!(!super::super::live::granted(&feed, Authority::Reply));
    assert!(!super::super::live::granted(&feed, Authority::Admin));
    assert!(super::super::devices::admin_refusal(&feed).is_some());
}
