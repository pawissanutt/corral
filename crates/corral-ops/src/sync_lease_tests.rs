use super::*;
use corral_core::backend::{BackendConfig, ManualClock, ViewError, fake::FakeBackend};
use std::{sync::Arc, time::Duration};

#[test]
fn obligation_phase_renews_after_external_work_but_not_explicit_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let clock = Arc::new(ManualClock::new(Timestamp::UNIX_EPOCH));
    let store = corral_core::Store::init(dir.path())
        .unwrap()
        .with_backend(Box::new(FakeBackend::new(
            BackendConfig::default(),
            clock.clone(),
            Arc::default(),
        )));
    let ctx = Ctx::from_store(store);
    let task = ctx
        .store
        .create("Waiting", None, Status::Running, "")
        .unwrap();
    ctx.store.update(&task.id, |task| {
        let mut run: corral_core::Run = serde_json::from_value(json!({
            "id":"R1", "agent":"worker", "worktree":dir.path(), "branch":"main",
            "base":"main", "base_sha":"base", "started":Timestamp::UNIX_EPOCH,
            "obligations":{"waits":[{
                "request":{"version":1,"id":"W1","run":"R1","condition":"external",
                    "owner":"worker","source":"worker","deadline":null,
                    "handle":{"kind":"channel","channel_id":"fixture","capability":"wait_v1"}},
                "registered_at":Timestamp::UNIX_EPOCH,
                "observation":{"at":Timestamp::UNIX_EPOCH,"state":"pending","detail":"waiting for a result"}
            }]}
        }))?;
        let receipt = serde_json::from_value(json!({
            "version":1,"id":"reply-W1","run":"R1","outcome":"answered",
            "delivery":"acknowledged","observed_at":Timestamp::UNIX_EPOCH
        }))?;
        run.obligations.waits[0].observation =
            crate::obligations::receipt_observation(&receipt, Timestamp::UNIX_EPOCH);
        run.obligations.waits[0].receipt = Some(receipt);
        task.runs.push(run);
        Ok(())
    }).unwrap();
    let head = ctx.store.head(&task.id).unwrap();
    let frozen = Ctx {
        store: ctx.store.read_only_snapshot().unwrap(),
        ..ctx.clone()
    };
    // Model the unbounded work between collecting heads and the obligation read phase.
    clock.advance(Duration::from_secs(31));
    let result = crate::obligations_sync::reconcile_head(&ctx, &head, Timestamp::UNIX_EPOCH, None);
    assert!(result.unwrap().is_none());
    assert_eq!(ctx.store.head(&task.id).unwrap().rev, head.rev);
    let error =
        crate::obligations_sync::reconcile_head(&frozen, &head, Timestamp::UNIX_EPOCH, None)
            .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<ViewError>(),
        Some(ViewError::Expired { .. })
    ));
}

#[cfg(unix)]
#[test]
fn foreign_loop_lease_prevents_sync_effects_until_released() {
    use corral_core::Store;
    use corral_herdr::Herdr;
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("herdr");
    std::fs::write(&bin, r#"#!/bin/sh
if [ "$1" = "--session" ]; then shift 2; fi
case "$1 $2" in
  "workspace list") echo '{"result":{"workspaces":[]}}' ;;
  "pane list") echo '{"result":{"panes":[{"pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1","agent":"fixture","agent_status":"working"}]}}' ;;
  "pane process-info") echo '{"result":{"process_info":{"shell_pid":10,"foreground_processes":[]}}}' ;;
  "pane read") echo 'working' ;;
  "notification show") echo '{"result":{}}' ;;
  *) exit 1 ;;
esac
"#).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let herdr = Herdr::with_bin(bin, Some("corral-test".into()));
    let store = Store::init(dir.path()).unwrap();
    let task = store
        .create_initialized("Working again", None, Status::NeedsMe, "", |task| {
            task.runs.push(serde_json::from_value(json!({
                "id":"R1", "agent":"fixture", "worktree":dir.path(), "branch":"main",
                "base":"main", "base_sha":"base", "started":Timestamp::now(),
                "agent_state":"blocked",
                "herdr":{"workspace_id":"w1","pane_id":"w1:p1","socket":herdr.socket_path()?}
            }))?);
            Ok(())
        })
        .unwrap();
    let owner = Store::open(dir.path()).unwrap();
    assert!(owner.own_loop().unwrap());
    let ctx = Ctx {
        store,
        herdr,
        resource_probe: crate::resources::FixedResourceProbe::arc(),
    };
    let before = serde_json::to_value(&task).unwrap();
    let (changes, attention) = sync_with_attention(&ctx).unwrap();
    assert!(changes.is_empty());
    assert!(attention.is_none());
    assert_eq!(
        serde_json::to_value(ctx.store.get(&task.id).unwrap()).unwrap(),
        before
    );
    owner.release_loop();
    let legacy_pass = owner.loop_pass().unwrap().unwrap();
    assert!(!ctx.store.loop_owned_elsewhere().unwrap());
    let (changes, _) = sync_with_attention(&ctx).unwrap();
    assert!(
        changes
            .iter()
            .any(|change| change.task == task.id && change.to == Status::Running)
    );
    assert_eq!(ctx.store.get(&task.id).unwrap().status, Status::Running);
    drop(legacy_pass);
    assert!(owner.own_loop().unwrap());
}

/// A quiet daemon pass leaves nothing in the change feed but read leases.
#[cfg(unix)]
mod quiet_pass {
    use super::*;
    use corral_core::Store;
    use corral_core::backend::{ChangeScope, ChangeSeq};
    use corral_herdr::Herdr;
    use std::os::unix::fs::PermissionsExt;

    /// One daemon pass: the sync, then the supervisor that `corral daemon run` follows it with.
    fn pass(ctx: &Ctx) {
        sync_with_attention(ctx).unwrap();
        crate::pm_supervisor::supervise(ctx).unwrap();
    }

    /// Every change entry after `after` that is not only read-lease bookkeeping.
    fn writes_since(ctx: &Ctx, after: ChangeSeq) -> Vec<String> {
        let view = ctx.store.view().unwrap();
        let mut cursor = after;
        let mut writes = Vec::new();
        loop {
            let page = view.changes(cursor, 256).unwrap();
            for entry in &page.entries {
                cursor = entry.seq;
                let lease_only = !entry.scopes.is_empty()
                    && entry.scopes.iter().all(
                        |scope| matches!(scope, ChangeScope::Record { area, .. } if area == "leases"),
                    );
                if !lease_only {
                    writes.push(format!("{:?} {:?}", entry.op, entry.scopes));
                }
            }
            if !page.more {
                return writes;
            }
        }
    }

    #[test]
    fn consecutive_quiet_passes_append_only_lease_records() {
        // Given an idle store: one finished task, a reachable herdr with an unowned pane.
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("herdr");
        std::fs::write(&bin, r#"#!/bin/sh
if [ "$1" = "--session" ]; then shift 2; fi
case "$1 $2" in
  "workspace list") echo '{"result":{"workspaces":[]}}' ;;
  "pane list") echo '{"result":{"panes":[{"pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1","agent":"fixture","agent_status":"idle"}]}}' ;;
  "pane process-info") echo '{"result":{"process_info":{"shell_pid":10,"foreground_processes":[]}}}' ;;
  "pane read") echo 'idle' ;;
  "notification show") echo '{"result":{}}' ;;
  *) exit 1 ;;
esac
"#).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let store = Store::init(dir.path()).unwrap();
        store.create("Finished", None, Status::Done, "").unwrap();
        let mut ctx = Ctx {
            store,
            herdr: Herdr::with_bin(bin, Some("corral-test".into())),
            resource_probe: crate::resources::FixedResourceProbe::arc(),
        };
        for supervise in [false, true] {
            ctx.store.config.pm.supervise = supervise;
            ctx.store.save_config().unwrap();
            pass(&ctx);
            // When the next pass finds nothing changed.
            let before = ctx.store.view().unwrap().seq();
            pass(&ctx);
            // Then it appended nothing but read leases.
            let writes = writes_since(&ctx, before);
            assert!(
                writes.is_empty(),
                "quiet pass (supervise={supervise}) wrote: {writes:#?}"
            );
        }
    }
}
