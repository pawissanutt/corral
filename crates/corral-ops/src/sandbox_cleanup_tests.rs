use super::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn noop_sweep_reuses_snapshot_without_task_metadata_work() {
    use corral_core::{
        Status, Store,
        work::{Work, count},
    };
    let tmp = tempfile::tempdir().unwrap();
    let ctx = crate::resources::FixedResourceProbe::ctx(Store::init(tmp.path()).unwrap());
    for _ in 0..32 {
        ctx.store
            .create("Historical task", None, Status::Done, "")
            .unwrap();
    }
    let dir = ctx.store.scratch_path("tmp").unwrap().join("launch-exits");
    for phase in 0..3 {
        if phase == 1 {
            std::fs::create_dir_all(&dir).unwrap();
        }
        if phase == 2 {
            std::fs::write(dir.join("T-001-R1.exited"), "0").unwrap();
        }
        let counters = [Work::TaskList, Work::TaskStat, Work::SessionBindingRead];
        let before = counters.map(count);
        sweep(&ctx).unwrap();
        assert_eq!(counters.map(count), before, "no-owner phase {phase}");
    }
}

#[test]
fn receipts_of_one_run_read_its_task_head_once() {
    use corral_core::work::{self, Counter, HotScope};
    use corral_core::{Status, Store};
    // Given a finished task whose run left worker, reviewer and gate owner receipts.
    let tmp = tempfile::tempdir().unwrap();
    let ctx = crate::resources::FixedResourceProbe::ctx(Store::init(tmp.path()).unwrap());
    let task = ctx
        .store
        .create("Finished", None, Status::Done, "")
        .unwrap();
    let arm = |attempt: &str| {
        let receipt = crate::launch_exit::receipt(&ctx, &task.id, attempt).unwrap();
        crate::launch_exit::arm(&ctx, "true", &receipt).unwrap();
    };
    let decodes = || {
        let _scope = HotScope::enter();
        let before = work::snapshot();
        terminal_owners(&ctx).unwrap();
        work::snapshot()
            .since(&before)
            .get(Counter::TaskHeadDecodes)
    };
    arm("R1");
    let one = decodes();
    arm("R1-review1");
    arm("R1-gate1");
    // When the owners are classified, then the shared task is decoded once, not per receipt.
    assert!(one > 0);
    assert_eq!(decodes(), one);
}

#[test]
fn owner_listing_uses_portable_podman_template() {
    let tmp = tempfile::tempdir().unwrap();
    let ctx =
        crate::resources::FixedResourceProbe::ctx(corral_core::Store::init(tmp.path()).unwrap());
    let shim = tmp.path().join("podman");
    std::fs::write(
        &shim,
        r##"#!/bin/sh
[ "$1" = ps ] && [ "$2" = -a ] && [ "$3" = --filter ] &&
[ "$4" = label=corral.load-sandbox=true ] && [ "$5" = --format ] &&
[ "$6" = '{{index .Labels "corral.run-owner"}}' ]
"##,
    )
    .unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    reap_scope_with(&ctx, "T-001", None, shim.to_str().unwrap()).unwrap();
}

#[test]
fn only_matching_run_sandboxes_are_removed() {
    let tmp = tempfile::tempdir().unwrap();
    let shim = tmp.path().join("podman");
    let log = tmp.path().join("removed");
    let marker = "a".repeat(64);
    std::fs::write(&shim, format!(r##"#!/bin/sh
if [ "$1" = ps ]; then
  case "$*" in *"label=corral.load-sandbox=true"*"label=corral.run-owner={marker}"*) echo 123456789abc;; esac
elif [ "$1" = rm ]; then
  echo "$5" >> '{}'
fi
"##, log.display())).unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    reap_with(shim.to_str().unwrap(), &marker).unwrap();
    reap_with(shim.to_str().unwrap(), &"b".repeat(64)).unwrap();
    assert_eq!(std::fs::read_to_string(log).unwrap(), "123456789abc\n");
}

fn terminal_owner(ctx: &Ctx) {
    let task = ctx
        .store
        .create(
            "Finished sandbox owner",
            None,
            corral_core::Status::Done,
            "",
        )
        .unwrap();
    let run: corral_core::Run = serde_json::from_value(json!({
        "id": "R1", "agent": "worker", "worktree": ctx.store.root(),
        "branch": "main", "base": "main", "base_sha": "abc",
        "started": "2026-10-15T00:00:00Z", "ended": "2026-10-15T00:01:00Z"
    }))
    .unwrap();
    ctx.store
        .update(&task.id, |task| {
            task.runs.push(run);
            Ok(())
        })
        .unwrap();
    let receipt = crate::launch_exit::receipt(ctx, &task.id, "R1").unwrap();
    crate::launch_exit::arm(ctx, "true", &receipt).unwrap();
}

fn empty_sweep_fixture() -> (tempfile::TempDir, Ctx, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let ctx =
        crate::resources::FixedResourceProbe::ctx(corral_core::Store::init(tmp.path()).unwrap());
    terminal_owner(&ctx);
    assert_eq!(terminal_owners(&ctx).unwrap().len(), 1);
    let shim = tmp.path().join("podman");
    let log = tmp.path().join("listings");
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\n[ \"$1\" = ps ] || exit 1\necho listed >> '{}'\necho '[]'\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    (tmp, ctx, shim, log)
}

#[test]
fn unchanged_terminal_owners_do_not_repeat_empty_listing() {
    let (_tmp, ctx, shim, log) = empty_sweep_fixture();
    sweep_with(&ctx, shim.to_str().unwrap()).unwrap();
    sweep_with(&ctx, shim.to_str().unwrap()).unwrap();
    assert_eq!(std::fs::read_to_string(log).unwrap(), "listed\n");
}

#[test]
fn new_terminal_owner_invalidates_empty_listing() {
    let (_tmp, ctx, shim, log) = empty_sweep_fixture();
    sweep_with(&ctx, shim.to_str().unwrap()).unwrap();
    terminal_owner(&ctx);
    assert_eq!(terminal_owners(&ctx).unwrap().len(), 2);
    sweep_with(&ctx, shim.to_str().unwrap()).unwrap();
    assert_eq!(std::fs::read_to_string(log).unwrap(), "listed\nlisted\n");
}

#[test]
fn expired_empty_listing_rechecks_for_late_containers() {
    let (_tmp, ctx, shim, log) = empty_sweep_fixture();
    sweep_with(&ctx, shim.to_str().unwrap()).unwrap();
    {
        let mut memos = SWEEP_MEMOS.lock().unwrap();
        let memo = memos.get_mut(ctx.store.dir()).unwrap();
        assert!(!memo.reusable(&memo.owners, memo.listed + RECHECK));
        memo.listed = Instant::now() - RECHECK - Duration::from_secs(1);
    }
    sweep_with(&ctx, shim.to_str().unwrap()).unwrap();
    assert_eq!(std::fs::read_to_string(log).unwrap(), "listed\nlisted\n");
}
