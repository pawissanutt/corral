use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::Migrate;
use crate::backend::{Clock, ManualClock};

struct AdvancingClock {
    clock: ManualClock,
    calls: AtomicU64,
}

impl Clock for AdvancingClock {
    fn now(&self) -> jiff::Timestamp {
        let now = self.clock.now();
        self.clock.advance(Duration::from_secs(121));
        self.calls.fetch_add(1, Ordering::SeqCst);
        now
    }
}

#[test]
fn import_verification_can_outlive_normal_view_age() {
    let dir = tempfile::tempdir().unwrap();
    let corral = dir.path().join(".corral");
    std::fs::create_dir_all(corral.join("tasks")).unwrap();
    std::fs::write(
        corral.join("corral.toml"),
        crate::Config::default_for(dir.path()).to_toml(),
    )
    .unwrap();
    std::fs::write(corral.join("tasks/T-001-test.md"), "---\nid: T-001\ntitle: Test\nstatus: idea\npriority: P3\ncreated: 2026-09-01T00:00:00Z\nupdated: 2026-09-01T00:00:00Z\n---\nBody.\n").unwrap();
    let clock = Arc::new(AdvancingClock {
        clock: ManualClock::new("2026-11-02T00:00:00Z".parse().unwrap()),
        calls: AtomicU64::new(0),
    });
    let mut migrate = Migrate::new(dir.path());
    migrate.candidate_clock = Some(clock.clone());
    assert_eq!(migrate.apply().unwrap().state, "published");
    assert!(clock.calls.load(Ordering::SeqCst) >= 2);
}
