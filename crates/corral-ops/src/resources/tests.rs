use super::*;

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
    let stored: ResourceState =
        serde_json::from_slice(&fs::read(ctx.store.state_dir().join("resources.json")).unwrap())
            .unwrap();
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
    assert!(!ctx.store.state_dir().join("resources.json").exists());
    assert!(!ctx.store.dir().join("lock/resources.lock").exists());
}

#[test]
fn read_only_admission_keeps_persisted_latch_unchanged() {
    let (_tmp, mut ctx) = fixture();
    sample(&mut ctx, 1.1, 200, 200);
    let path = ctx.store.state_dir().join("resources.json");
    let before = fs::read(&path).unwrap();
    let mut snapshot = ctx.resource_probe.sample(&ctx.store).unwrap();
    snapshot.load_per_core = 0.95;
    ctx.resource_probe = Arc::new(FixedResourceProbe(snapshot));
    assert!(read_admission(&ctx).unwrap().is_some());
    assert_eq!(fs::read(&path).unwrap(), before);
}
