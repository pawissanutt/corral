use super::*;

#[test]
fn pinned_readings_preserve_inventory_and_count_real_walks() {
    // Given a real store, project directories and the validated default thresholds.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    assert_eq!(store.config.resources, corral_core::ResourcesCfg::default());
    store.config.validate(store.root()).unwrap();
    let real = SystemResourceProbe.sample(&store).unwrap();
    let probe = PinnedReadingsProbe::new(8.0, 123, 456);
    assert_eq!(probe.samples(), 0);
    // When numeric readings are pinned, the real inventory still runs on every call.
    let pinned = probe.sample(&store).unwrap();
    probe.sample(&store).unwrap();
    // Then only the requested numbers change; paths and core inventory are preserved.
    assert_eq!(probe.samples(), 2);
    assert_eq!(pinned.cores, real.cores);
    assert_eq!(pinned.load_one, 8.0);
    assert_eq!(pinned.load_per_core, 8.0 / f64::from(real.cores));
    assert_eq!(pinned.mem_available, 123);
    assert_eq!(
        pinned.disks.iter().map(|d| &d.path).collect::<Vec<_>>(),
        real.disks.iter().map(|d| &d.path).collect::<Vec<_>>()
    );
    assert!(pinned.disks.iter().all(|d| d.free_bytes == 456));
    assert_eq!(pinned.cpu_psi.is_some(), real.cpu_psi.is_some());
    assert_eq!(pinned.mem_psi.is_some(), real.mem_psi.is_some());
}

#[test]
fn admitting_pinned_readings_clear_every_persisted_latch() {
    for (held, severe) in [(false, false), (true, false), (true, true)] {
        // Given a persisted latch and unchanged, valid resource defaults.
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::init(tmp.path()).unwrap();
        store.config.validate(store.root()).unwrap();
        let probe = Arc::new(PinnedReadingsProbe::admitting());
        let ctx = Ctx {
            resource_probe: probe.clone(),
            ..Ctx::from_store(store)
        };
        let mut previous = read_status(&ctx).unwrap();
        previous.held = held;
        previous.severe = severe;
        previous.reason = held.then(|| "previous pressure".into());
        OBSERVATIONS.save(&ctx.store, "state", &previous).unwrap();
        // When the real walk finishes and admitting numbers reach the evaluator.
        let state = observe(&ctx, previous.observed_at).unwrap();
        // Then the evaluator clears either latch using its normal recovery margins.
        assert!(!state.held && !state.severe && state.reason.is_none());
        assert_eq!(state.snapshot.load_one, 0.0);
        assert_eq!(state.snapshot.load_per_core, 0.0);
        assert_eq!(state.snapshot.mem_available, u64::MAX);
        assert!(
            state
                .snapshot
                .disks
                .iter()
                .all(|d| d.free_bytes == u64::MAX)
        );
        assert_eq!(probe.samples(), 2);
    }
}

fn fixture() -> (tempfile::TempDir, Ctx) {
    let tmp = tempfile::tempdir().unwrap();
    let mut ctx = FixedResourceProbe::ctx(Store::init(tmp.path()).unwrap());
    ctx.store.config.resources.min_mem_available = "100B".into();
    ctx.store.config.resources.min_disk_free = "100B".into();
    (tmp, ctx)
}

fn sample(ctx: &mut Ctx, load: f64, mem: u64, disk: u64) -> ResourceState {
    ctx.resource_probe = Arc::new(FixedResourceProbe(ResourceSnapshot {
        load_per_core: load,
        load_one: load * 4.0,
        cores: 4,
        mem_available: mem,
        cpu_psi: None,
        mem_psi: None,
        disks: vec![DiskSpace {
            path: "/fixture".into(),
            free_bytes: disk,
        }],
    }));
    observe(ctx, "2026-10-15T00:00:00Z".parse().unwrap()).unwrap()
}

#[test]
fn cpu_hold_survives_restart_and_requires_recovery_margin() {
    let (_tmp, mut ctx) = fixture();
    assert!(sample(&mut ctx, 1.01, 200, 200).held);
    assert!(!status(&ctx).unwrap().severe);
    let mut restarted = Ctx {
        store: Store::open(ctx.store.root()).unwrap(),
        ..ctx.clone()
    };
    restarted.store.config.resources = ctx.store.config.resources.clone();
    assert!(sample(&mut restarted, 0.99, 200, 200).held);
    assert!(sample(&mut restarted, 0.91, 200, 200).held);
    assert!(!sample(&mut restarted, 0.9, 200, 200).held);
    assert!(!sample(&mut restarted, 1.0, 200, 200).held);
}

#[test]
fn memory_and_disk_latch_until_both_recover() {
    let (_tmp, mut ctx) = fixture();
    let state = sample(&mut ctx, 0.1, 99, 200);
    assert!(state.held && state.severe);
    assert!(sample(&mut ctx, 0.1, 109, 200).severe);
    assert!(sample(&mut ctx, 0.1, 110, 99).severe);
    assert!(sample(&mut ctx, 0.1, 110, 109).held);
    let state = sample(&mut ctx, 0.1, 110, 110);
    assert!(!state.held && !state.severe && state.reason.is_none());
}

#[test]
fn admission_uses_same_state_and_machine_busy_prefix() {
    let (_tmp, mut ctx) = fixture();
    sample(&mut ctx, 2.0, 200, 200);
    let reason = admission(&ctx).unwrap().unwrap();
    assert!(reason.starts_with("waiting: machine busy (load/core 2.00"));
    assert!(reason.ends_with(')'));
    let stored = OBSERVATIONS.load(&ctx.store, "state").unwrap().unwrap();
    assert!(stored.held);
    assert!(!stored.severe);
}

#[test]
fn proc_parsers_handle_kernel_units_and_optional_pressure_fields() {
    assert!((system::parse_load("4.25 3.0 2.0 1/123 5\n").unwrap() - 4.25).abs() < 0.001);
    assert_eq!(
        system::parse_mem("MemTotal: 100 kB\nMemAvailable: 42 kB\n").unwrap(),
        43008
    );
    assert!(
        (system::parse_psi("some avg10=12.34 avg60=9.0 total=123\nfull avg10=1.0\n").unwrap()
            - 12.34)
            .abs()
            < 0.001
    );
    for input in ["", "NaN", "-1", "inf"] {
        assert!(system::parse_load(input).is_err());
    }
    assert!(system::parse_mem("MemFree: 42 kB").is_err());
    assert!(system::parse_mem("MemAvailable: 42 GB").is_err());
    assert!(system::parse_psi("some avg10=NaN").is_err());
}

#[test]
fn read_only_status_does_not_create_state_or_locks() {
    let (_tmp, ctx) = fixture();
    assert!(!read_status(&ctx).unwrap().held);
    assert!(read_admission(&ctx).unwrap().is_none());
    assert!(OBSERVATIONS.load(&ctx.store, "state").unwrap().is_none());
    assert!(!ctx.store.dir().join("lock/resources.lock").exists());
}

#[test]
fn read_only_admission_keeps_persisted_latch_unchanged() {
    let (_tmp, mut ctx) = fixture();
    sample(&mut ctx, 1.1, 200, 200);
    let before = ctx.store.record_row("resources", "state").unwrap();
    let mut snapshot = ctx.resource_probe.sample(&ctx.store).unwrap();
    snapshot.load_per_core = 0.95;
    ctx.resource_probe = Arc::new(FixedResourceProbe(snapshot));
    assert!(read_admission(&ctx).unwrap().is_some());
    assert_eq!(ctx.store.record_row("resources", "state").unwrap(), before);
}

#[test]
fn deleting_caches_preserves_resource_latches_and_alerts_after_restart() {
    // Given durable severe pressure and one resource-owned attention card.
    let (_tmp, mut ctx) = fixture();
    ctx.store.config.storage.cache_root = Some(ctx.store.root().join("cache"));
    ctx.store.save_config().unwrap();
    ctx.herdr = corral_herdr::Herdr::with_bin(ctx.store.root().join("unused-herdr"), None);
    let task = ctx
        .store
        .create("Running", None, corral_core::Status::Running, "")
        .unwrap();
    let now: Timestamp = "2026-10-15T00:00:00Z".parse().unwrap();
    let state = sample(&mut ctx, 0.1, 99, 200);
    crate::resource_escalation::reconcile(&ctx, &state, now).unwrap();
    let root = ctx.store.cache_root().unwrap();
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("disposable"), "cache").unwrap();
    let latch = ctx.store.record_row("resources", "state").unwrap().unwrap();
    let alert = ctx
        .store
        .record_row("resource-alert", "state")
        .unwrap()
        .unwrap();
    assert_eq!(latch.doc["doc_version"], 1);
    assert_eq!(alert.doc["doc_version"], 1);

    // When the cache root is removed and the store reopens with pressure inside the margin.
    fs::remove_dir_all(&root).unwrap();
    ctx.store = Store::open(ctx.store.root()).unwrap();
    let held = sample(&mut ctx, 0.1, 109, 200);
    let changes = crate::resource_escalation::reconcile(&ctx, &held, now).unwrap();

    // Then the latch and attention survive, with no duplicate alert or legacy mirrors.
    assert!(held.held && held.severe);
    assert!(changes.is_empty());
    assert!(
        crate::resource_escalation::holds_attention(&ctx, &ctx.store.get(&task.id).unwrap())
            .unwrap()
    );
    assert_eq!(
        ctx.store.record_row("resource-alert", "state").unwrap(),
        Some(alert)
    );
    assert!(
        !ctx.store
            .dir()
            .join("state")
            .join("resources.json")
            .exists()
    );
    assert!(
        !ctx.store
            .dir()
            .join("state")
            .join("resource-alert.json")
            .exists()
    );

    // Recovery clears both the persisted alert and its attention card.
    let healthy = sample(&mut ctx, 0.1, 110, 110);
    let changes = crate::resource_escalation::reconcile(&ctx, &healthy, now).unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(
        ctx.store.get(&task.id).unwrap().status,
        corral_core::Status::Running
    );
    assert!(
        ctx.store
            .record_row("resource-alert", "state")
            .unwrap()
            .is_none()
    );
}

#[test]
fn malformed_resource_records_fail_instead_of_resetting_hysteresis() {
    // Given a malformed canonical observation and an otherwise healthy probe.
    let (_tmp, ctx) = fixture();
    ctx.store
        .save_record(
            "resources",
            "state",
            &serde_json::json!({"doc_version": 2, "value": {}}),
        )
        .unwrap();
    // When reading the admission latch, then malformed state is not treated as missing.
    assert!(read_status(&ctx).is_err());
}

#[test]
fn alert_raise_and_recovery_commit_task_record_and_event_together() {
    use corral_core::Status;

    for initial in [Status::Running, Status::Ready] {
        let (_tmp, mut ctx) = fixture();
        ctx.herdr = corral_herdr::Herdr::with_bin(ctx.store.root().join("unused-herdr"), None);
        let task = ctx.store.create("Pressure", None, initial, "").unwrap();
        let now: Timestamp = "2026-10-15T00:00:00Z".parse().unwrap();
        let severe = sample(&mut ctx, 0.1, 99, 200);
        let before = ctx.store.view().unwrap().seq();

        // A task attention transition, its record and its event publish once.
        let changes = crate::resource_escalation::reconcile(&ctx, &severe, now).unwrap();
        assert_eq!(ctx.store.view().unwrap().seq().0, before.0 + 1);
        assert_eq!(changes.len(), usize::from(initial == Status::Running));
        assert_eq!(
            ctx.store.get(&task.id).unwrap().status,
            if initial == Status::Running {
                Status::NeedsMe
            } else {
                initial
            }
        );
        assert!(
            ctx.store
                .record_row("resource-alert", "state")
                .unwrap()
                .is_some()
        );
        assert_eq!(ctx.store.events_of(None, &["escalated"]).unwrap().len(), 1);

        let healthy = sample(&mut ctx, 0.1, 110, 110);
        let before = ctx.store.view().unwrap().seq();

        // Recovery publishes the restoration, record removal and event together too.
        let changes = crate::resource_escalation::reconcile(&ctx, &healthy, now).unwrap();
        assert_eq!(ctx.store.view().unwrap().seq().0, before.0 + 1);
        assert_eq!(changes.len(), usize::from(initial == Status::Running));
        assert_eq!(ctx.store.get(&task.id).unwrap().status, initial);
        assert!(
            ctx.store
                .record_row("resource-alert", "state")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            ctx.store
                .events_of(None, &["resources_cleared"])
                .unwrap()
                .len(),
            1
        );
    }
}
