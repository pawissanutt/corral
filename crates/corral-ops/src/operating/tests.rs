use super::*;

fn refusal(reason: &str) -> anyhow::Error {
    AuthorizationRefused(reason.into()).into()
}

#[test]
fn capacity_and_snapshot_refusals_are_transient() {
    for reason in [
        "submission precondition refused: cannot request changes for T-1: waiting: machine busy (load/core 1.40)",
        "submission precondition refused: cannot request changes for T-1: waiting for a slot (1/1 tasks)",
        "submission precondition refused: T-1 or dispatch configuration changed; retry with the current state",
    ] {
        assert!(transient_refusal(&refusal(reason)), "{reason}");
    }
}

#[test]
fn other_refusals_and_ordinary_errors_are_not_transient() {
    assert!(!transient_refusal(&refusal(
        "automatic Repair is not authorized for T-1: paused"
    )));
    assert!(!transient_refusal(&refusal(
        "submission precondition refused: run changed after submitting changes"
    )));
    // A plain error that merely mentions a marker is not a submission refusal.
    assert!(!transient_refusal(&anyhow::anyhow!(
        "agent pane is gone: waiting: machine busy"
    )));
    assert!(!transient_refusal(&anyhow::anyhow!("waiting for a slot")));
}

fn queued_owner_fixture() -> (tempfile::TempDir, Ctx, Task) {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("app")).unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.defaults.profile = Some("generic".into());
    store.config.projects.insert(
        "app".into(),
        corral_core::Project {
            dispatch: Default::default(),
            path: "app".into(),
            default_profile: None,
            base: None,
            review: Default::default(),
            artifact_retention: None,
            post_land: Vec::new(),
        },
    );
    store.save_config().unwrap();
    let ctx = Ctx::from_store(store);
    let task = ctx
        .store
        .create(
            "Queued",
            Some("app"),
            corral_core::Status::Ready,
            "Contract",
        )
        .unwrap();
    crate::queue::enqueue(&ctx, &task.id, &crate::DispatchOpts::default()).unwrap();
    let task = ctx.store.get(&task.id).unwrap();
    (tmp, ctx, task)
}

fn edit_config(store: &Store, edit: impl FnOnce(&mut corral_core::Config)) {
    let mut fresh = Store::open(store.root()).unwrap();
    edit(&mut fresh.config);
    fresh.save_config().unwrap();
}

#[test]
fn limits_change_keeps_queued_owner_consent_and_owner_only() {
    // Given an owner-queued start.
    let (_tmp, ctx, task) = queued_owner_fixture();
    let action = start_identity(&task, false);
    let consent = load_start(&ctx, &action).unwrap().unwrap();
    assert!(!is_automatic(&consent));
    // When the owner changes slot capacity.
    edit_config(&ctx.store, |config| config.limits.running += 1);
    // Then the queued consent is still current: capacity is admission, not consent (T-560).
    assert!(!ctx.store.operating_authorization_stale(&consent).unwrap());
    let queued = authorize_start(&ctx, action.clone(), Origin::Owner, true).unwrap();
    assert_eq!(queued, consent);
    // And an automatic caller still cannot use the owner's consent.
    assert!(authorize_start(&ctx, action, Origin::Automatic, true).is_err());
    let error = authorize(&ctx, consent.action.clone(), Origin::Automatic).unwrap_err();
    assert!(format!("{error:#}").contains("automatic caller cannot use owner approval"));
    // A meaningful review-policy change still stales the consent (T-204 is unchanged).
    edit_config(&ctx.store, |config| config.review.max_rounds = Some(9));
    assert!(ctx.store.operating_authorization_stale(&consent).unwrap());
}
