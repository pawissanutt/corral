use super::*;
use crate::tests::Fixture;
use corral_protocol::{app::DeviceSetAuthority, cmd::task::TaskCommand};

fn setup(fixture: &Fixture) -> (Arc<Shared>, Arc<Service>, String) {
    let shared = Arc::new(Shared::new());
    crate::devices::start(&fixture.ctx, &shared, None).unwrap();
    let device = shared
        .devices()
        .unwrap()
        .enrol_native("Owner phone", "test", "test")
        .unwrap();
    let (jobs, _) = mpsc::sync_channel(QUEUE);
    let service = Arc::new(Service {
        jobs,
        flights: Mutex::default(),
        index: Mutex::new(Index::default()),
        journal: Mutex::new(Store::open_unobserved(fixture.ctx.store.root()).unwrap()),
        previews: Mutex::default(),
    });
    *lock(&shared.commands) = Some(Arc::clone(&service));
    (shared, service, device.id)
}

fn comment() -> Command {
    Command::Task(TaskCommand::Comment {
        id: "T-1".into(),
        text: "Owner note".into(),
        author: "pm".into(),
        send: false,
    })
}

fn queued_submit(device: &str) -> CmdSubmit {
    let issued = corral_core::clock::now();
    CmdSubmit {
        cmd_id: "queued".into(),
        caller: caller(device),
        client_build: None,
        expect: None,
        issued_at: issued.to_string(),
        expires_at: issued
            .checked_add(SignedDuration::from_secs(120))
            .unwrap()
            .to_string(),
        command: comment(),
    }
}

#[test]
fn app_wire_cannot_spoof_caller_or_cli_device_principal() {
    let fixture = Fixture::new();
    let (shared, _, device) = setup(&fixture);
    let mut wire = serde_json::to_value(ActSubmit {
        cmd_id: "spoof".into(),
        expect: None,
        issued_at: "2026-01-01T00:00:00Z".into(),
        expires_at: "2026-01-01T00:02:00Z".into(),
        command: comment(),
        confirm: None,
    })
    .unwrap();
    wire["caller"] = serde_json::json!({"principal":{"kind":"device","id":"victim"},"actor":"pm"});
    wire["client_build"] = serde_json::json!({"exe":"attacker","build":"attacker"});
    let decoded: ActSubmit = serde_json::from_value(wire).unwrap();
    assert_eq!(decoded.cmd_id, "spoof");
    let invoker = execution_invoker(&shared, caller(&device), &decoded.command).unwrap();
    assert_eq!(invoker.caller.principal, Principal::Device { id: device });
    assert_eq!(invoker.caller.actor, Some(ActorName::Owner));
    assert_eq!(invoker.caller.via, Via::App);
    assert!(invoker.caller.cwd.is_none());
    assert!(invoker.caller.env.corral_pm.is_none());
    corral_ops::pm_read::set_build_identity("test");
    let (exe, build) = corral_ops::pm_read::client_identity().unwrap();
    let mut cli = queued_submit("victim");
    cli.client_build = Some(corral_protocol::cmd::ClientBuild { exe, build });
    cli.caller.principal = Principal::Device {
        id: "victim".into(),
    };
    assert!(matches!(
        super::super::admit(&cli),
        Err((ErrorCode::Unauthenticated, _))
    ));
    shared.close();
}

#[test]
fn proof_is_device_expectation_bound_single_use_and_expires() {
    let fixture = Fixture::new();
    let (shared, service, device) = setup(&fixture);
    let expect = Expect::TaskRevision {
        task: "T-1".into(),
        revision: "r1".into(),
    };
    let digest = action_digest(&device, &comment(), Some(&expect)).unwrap();
    let preview = issue(&service, &device, digest.clone(), ActClass::Confirm, vec![]).unwrap();
    let proof = ConfirmProof {
        preview_id: preview.preview_id,
        digest: digest.clone(),
    };
    assert!(consume(&service, "another-device", &digest, Some(&proof)).is_err());
    let changed = action_digest(&device, &comment(), None).unwrap();
    assert!(consume(&service, &device, &changed, Some(&proof)).is_err());
    consume(&service, &device, &digest, Some(&proof)).unwrap();
    assert!(consume(&service, &device, &digest, Some(&proof)).is_err());
    let expired = issue(&service, &device, digest.clone(), ActClass::Confirm, vec![]).unwrap();
    lock(&service.previews)
        .get_mut(&expired.preview_id)
        .unwrap()
        .deadline = Instant::now();
    assert!(
        consume(
            &service,
            &device,
            &digest,
            Some(&ConfirmProof {
                preview_id: expired.preview_id,
                digest: expired.digest,
            })
        )
        .is_err()
    );
    shared.close();
}

#[test]
fn queued_revocation_or_authority_removal_prevents_reservation_and_execution() {
    let fixture = Fixture::new();
    let (shared, service, device) = setup(&fixture);
    let submit = queued_submit(&device);
    authorize(&shared, &submit).unwrap(); // The connection has admitted the queued work.
    let devices = shared.devices().unwrap();
    let (_, mut authority) = devices.authenticated(&device).unwrap();
    authority.reply = false;
    devices
        .set_authority(
            &device,
            DeviceSetAuthority {
                device: device.clone(),
                authority,
            },
        )
        .unwrap();
    let mut worker = Worker {
        base: fixture.ctx.clone(),
        timing: corral_ops::timing::snapshot(),
    };
    let key = journal_key(&device, &submit.cmd_id);
    let (reply, executed) = service.accept(&mut worker, &shared, submit, &key, "digest", None);
    assert!(matches!(
        reply,
        Reply::Refuse(ErrorCode::Refused, "authority_reply")
    ));
    assert!(!executed);
    assert!(
        fixture
            .ctx
            .store
            .record::<Journal>(COMMANDS_AREA, &key)
            .unwrap()
            .is_none()
    );
    devices.revoke(&device, "test").unwrap();
    let (reply, executed) = service.accept(
        &mut worker,
        &shared,
        queued_submit(&device),
        &key,
        "digest",
        None,
    );
    assert!(matches!(reply, Reply::Refuse(ErrorCode::Revoked, _)));
    assert!(!executed);
    assert!(
        fixture
            .ctx
            .store
            .record::<Journal>(COMMANDS_AREA, &key)
            .unwrap()
            .is_none()
    );
    shared.close();
}

#[test]
fn app_tombstones_retain_recovery_seven_days_and_never_announce_data() {
    let fixture = Fixture::new();
    let (shared, service, device) = setup(&fixture);
    let key = journal_key(&device, "absent");
    let answer = service.query(&key, Instant::now(), true).unwrap();
    let result = control_result("absent".into(), answer);
    assert_eq!(result.state, CmdState::Abandoned);
    assert!(result.output.is_none());
    let journal = fixture
        .ctx
        .store
        .record::<Journal>(COMMANDS_AREA, &key)
        .unwrap()
        .unwrap();
    let deadline = journal::eviction_time(&key, &journal);
    assert!(deadline.as_second() >= corral_core::clock::now().as_second() + 7 * 24 * 60 * 60 - 1);
    let day = corral_core::clock::now()
        .checked_add(SignedDuration::from_secs(24 * 60 * 60))
        .unwrap();
    assert!(
        lock(&service.index)
            .evictable(principal(&key), day)
            .is_empty()
    );
    shared.close();
}

#[test]
fn web_authority_defaults_and_explicit_restrictions_are_preserved() {
    let fixture = Fixture::new();
    let (shared, _, native) = setup(&fixture);
    let mut web = fixture
        .ctx
        .store
        .record::<Value>("devices", &native)
        .unwrap()
        .unwrap();
    web["value"]["platform"] = serde_json::json!("web");
    web["value"].as_object_mut().unwrap().remove("authority");
    fixture
        .ctx
        .store
        .save_record("devices", "dev-web", &web)
        .unwrap();
    let devices = shared.devices().unwrap();
    let (info, mut authority) = devices.authenticated("dev-web").unwrap();
    assert!(authority.admin);
    assert!(authority.assistant_act);
    assert_eq!(info.authority, Some(authority.clone()));
    authority.admin = false;
    authority.assistant_act = false;
    devices
        .set_authority(
            &native,
            DeviceSetAuthority {
                device: "dev-web".into(),
                authority: authority.clone(),
            },
        )
        .unwrap();
    assert_eq!(devices.authenticated("dev-web").unwrap().1, authority);
    assert!(
        devices
            .set_authority(
                "dev-web",
                DeviceSetAuthority {
                    device: native,
                    authority,
                }
            )
            .is_err()
    );
    shared.close();
}

#[test]
fn seen_cursor_persists_per_device_and_never_regresses() {
    let fixture = Fixture::new();
    let (shared, _, first) = setup(&fixture);
    let devices = shared.devices().unwrap();
    let second = devices.enrol_native("Second", "test", "test").unwrap();
    assert_eq!(devices.advance_seen(&first, 9).unwrap().through, 9);
    assert_eq!(devices.advance_seen(&first, 3).unwrap().through, 9);
    assert_eq!(devices.seen(&second.id).unwrap().through, 0);
    let reopened = Arc::new(Shared::new());
    crate::devices::start(&fixture.ctx, &reopened, None).unwrap();
    assert_eq!(reopened.devices().unwrap().seen(&first).unwrap().through, 9);
    shared.close();
    reopened.close();
}

#[test]
fn prompt_answer_replays_settlement_before_requiring_another_proof() {
    let fixture = Fixture::new();
    let (shared, service, device) = setup(&fixture);
    let request = PromptAnswer {
        cmd_id: "prompt-id".into(),
        key: "A-1/P1".into(),
        rev: 1,
        choice: PromptChoice::Deny,
        confirm: None,
    };
    let key = journal_key(&device, &request.cmd_id);
    let digest = prompt_digest(&device, &request.key, request.rev, &request.choice).unwrap();
    let mut worker = Worker {
        base: fixture.ctx.clone(),
        timing: corral_ops::timing::snapshot(),
    };
    let (first, _) = accept_prompt(
        &service,
        &mut worker,
        &shared,
        &device,
        request.clone(),
        &key,
        &digest,
    );
    let Reply::Answer(first) = first else {
        panic!("expected settled canonical outcome");
    };
    let (again, executed) = accept_prompt(
        &service,
        &mut worker,
        &shared,
        &device,
        request,
        &key,
        &digest,
    );
    let Reply::Answer(again) = again else {
        panic!("expected replayed canonical outcome");
    };
    assert_eq!(first.state, again.state);
    assert!(again.replayed);
    assert!(!executed);
    assert!(again.output.is_none());
    shared.close();
}
