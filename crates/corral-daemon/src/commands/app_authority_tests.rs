use super::*;
use crate::tests::Fixture;
use corral_protocol::{app::DeviceSetAuthority, device::DeviceAuthorityChange};

#[test]
fn web_admin_grants_require_an_existing_admins_device_bound_confirmation() {
    let fixture = Fixture::new();
    let shared = Arc::new(Shared::new());
    crate::devices::start(&fixture.ctx, &shared, None).unwrap();
    let devices = shared.devices().unwrap();
    let owner = devices.enrol_native("Owner", "test", "test").unwrap();
    let web = devices.enrol_native("Browser", "test", "test").unwrap();
    let mut record = fixture
        .ctx
        .store
        .record::<serde_json::Value>("devices", &web.id)
        .unwrap()
        .unwrap();
    record["value"]["platform"] = serde_json::json!("web");
    record["value"]["authority"]["admin"] = serde_json::json!(false);
    fixture
        .ctx
        .store
        .save_record("devices", &web.id, &record)
        .unwrap();
    let (jobs, _) = mpsc::sync_channel(QUEUE);
    *lock(&shared.commands) = Some(Arc::new(Service {
        jobs,
        flights: Mutex::default(),
        index: Mutex::new(Index::default()),
        journal: Mutex::new(Store::open_unobserved(fixture.ctx.store.root()).unwrap()),
        previews: Mutex::default(),
    }));
    let (_, mut authority) = devices.authenticated(&web.id).unwrap();
    authority.admin = true;
    let change = DeviceSetAuthority {
        device: web.id.clone(),
        authority,
    };
    assert_eq!(
        authority_preview(&shared, &web.id, change.clone())
            .unwrap_err()
            .0,
        ErrorCode::Refused
    );
    let request = DeviceAuthorityChange {
        change: change.clone(),
        confirm: None,
    };
    assert_eq!(
        set_authority(&shared, &owner.id, request).unwrap_err().0,
        ErrorCode::ConfirmRequired
    );
    let preview = authority_preview(&shared, &owner.id, change.clone()).unwrap();
    assert_eq!(preview.class, ActClass::Confirm);
    let request = DeviceAuthorityChange {
        change,
        confirm: Some(ConfirmProof {
            preview_id: preview.preview_id,
            digest: preview.digest,
        }),
    };
    set_authority(&shared, &owner.id, request.clone()).unwrap();
    assert!(devices.authenticated(&web.id).unwrap().1.admin);
    assert_eq!(
        set_authority(&shared, &owner.id, request).unwrap_err().0,
        ErrorCode::ConfirmRequired
    );
    let (_, mut authority) = devices.authenticated(&owner.id).unwrap();
    authority.reply = false;
    let change = DeviceSetAuthority {
        device: owner.id.clone(),
        authority,
    };
    let preview = authority_preview(&shared, &web.id, change.clone()).unwrap();
    set_authority(
        &shared,
        &web.id,
        DeviceAuthorityChange {
            change,
            confirm: Some(ConfirmProof {
                preview_id: preview.preview_id,
                digest: preview.digest,
            }),
        },
    )
    .unwrap();
    assert!(!devices.authenticated(&owner.id).unwrap().1.reply);
    shared.close();
}
