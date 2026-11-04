use super::*;
use crate::resources::{FixedResourceProbe, ResourceProbe};
use corral_core::backend::{BackendConfig, ManualClock, ViewError, fake::FakeBackend};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

struct SlowProbe {
    clock: Arc<ManualClock>,
    calls: AtomicUsize,
}
impl std::fmt::Debug for SlowProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SlowProbe")
    }
}
impl ResourceProbe for SlowProbe {
    fn sample(&self, _: &Store) -> Result<corral_core::ResourceSnapshot> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.clock.advance(Duration::from_secs(31));
        Ok(FixedResourceProbe::healthy().0)
    }
}

#[test]
fn sync_attention_opens_its_read_phase_after_the_host_probe() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::new(Timestamp::UNIX_EPOCH));
    let store = Store::init(dir.path())
        .unwrap()
        .with_backend(Box::new(FakeBackend::new(
            BackendConfig::default(),
            clock.clone(),
            Arc::default(),
        )));
    let probe = Arc::new(SlowProbe {
        clock,
        calls: AtomicUsize::new(0),
    });
    let ctx = Ctx {
        resource_probe: probe.clone(),
        ..Ctx::from_store(store)
    };
    let task = ctx
        .store
        .create("Review", None, Status::Review, "")
        .unwrap();
    ctx.store
        .update(&task.id, |task| {
            task.runs.push(serde_json::from_value(serde_json::json!({
                "id":"R1","agent":"worker","worktree":dir.path(),"branch":"main",
                "base":"main","base_sha":"base","started":Timestamp::UNIX_EPOCH,
                "outcome":"done"
            }))?);
            Ok(())
        })
        .unwrap();
    let frozen = Ctx {
        store: ctx.store.read_only_snapshot().unwrap(),
        ..ctx.clone()
    };
    let (_, board) = reconcile_snapshot(&ctx, Timestamp::UNIX_EPOCH).unwrap();
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    assert_eq!(board.tasks.len(), 1);
    assert_eq!(board.tasks[0].task, task.id);
    let error = reconcile_snapshot(&frozen, Timestamp::UNIX_EPOCH).unwrap_err();
    assert!(matches!(
        error.downcast_ref::<ViewError>(),
        Some(ViewError::Expired { .. })
    ));
}
