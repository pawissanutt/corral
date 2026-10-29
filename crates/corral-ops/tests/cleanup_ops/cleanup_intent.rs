//! Lifecycle GC reads the approval's recorded cleanup intent (T-334 review F8): a requested
//! removal that failed is retried once the worktree is idle; only a requested keep is retained.

use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Child;

use corral_core::backend::LifecycleOutcome;

use super::*;

struct Holder(Child);

impl Holder {
    /// A process named like a protected harness, with its working directory in `dir`.
    fn start(dir: &Path) -> Holder {
        let mut child = support::command("python3", dir)
            .args([
                "-c",
                "import signal; print('ready', flush=True); signal.pause()",
            ])
            .arg0("codex")
            .current_dir(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), "ready");
        Holder(child)
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn intent(fx: &Fx) -> Value {
    let approved = fx.ctx.store.events_of(Some(&fx.id), &["approved"]).unwrap();
    approved.last().unwrap().detail["cleanup"].clone()
}

/// The lifecycle outcome and reason sync records for the task's run.
fn lifecycle(fx: &Fx) -> (LifecycleOutcome, Option<String>) {
    corral_ops::storage_gc::lifecycle_pass(&fx.ctx, corral_core::clock::now()).unwrap();
    let row = fx
        .ctx
        .store
        .lifecycle_row(&fx.id, &fx.run().id, "worktree")
        .unwrap()
        .unwrap();
    (row.outcome, row.reason)
}

/// Approves with a protected resident in the worktree, so the requested removal fails, then
/// stops the resident: sync removes the worktree and keeps the branch.
fn failed_removal_is_retried(strategy: ApproveStrategy, keep_branch: bool) {
    let fx = Fx::new();
    fx.owner_ready();
    let run = fx.run();
    let holder = Holder::start(&run.worktree);
    let line = corral_ops::approve(&fx.ctx, &fx.id, strategy, true, keep_branch).unwrap();
    assert!(line.contains("protected process"), "{line}");
    assert!(run.worktree.is_dir());
    assert_eq!(intent(&fx), json!({"v": 1, "remove_worktree": true}));
    drop(holder);
    let (outcome, reason) = lifecycle(&fx);
    assert_eq!(outcome, LifecycleOutcome::Reclaimed, "{reason:?}");
    assert!(!run.worktree.exists());
    assert!(
        corral_git::rev_sha(&fx.app, &run.branch).is_ok(),
        "branch kept"
    );
}

#[test]
fn a_failed_keep_branch_strategy_removal_is_retried() {
    failed_removal_is_retried(ApproveStrategy::KeepBranch, false);
}

#[test]
fn a_failed_removal_with_the_keep_branch_flag_is_retried() {
    failed_removal_is_retried(ApproveStrategy::FastForward, true);
}

#[test]
fn a_successful_default_cleanup_is_recorded_as_reclaimed() {
    let fx = Fx::new();
    fx.owner_ready();
    let run = fx.run();
    corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, true, false).unwrap();
    assert!(!run.worktree.exists());
    assert_eq!(intent(&fx), json!({"v": 1, "remove_worktree": true}));
    assert_eq!(lifecycle(&fx).0, LifecycleOutcome::Reclaimed);
}

#[test]
fn an_explicit_keep_worktree_is_never_removed() {
    let fx = Fx::new();
    fx.owner_ready();
    let run = fx.run();
    corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, false, false).unwrap();
    assert_eq!(intent(&fx), json!({"v": 1, "remove_worktree": false}));
    let (outcome, reason) = lifecycle(&fx);
    assert_eq!(outcome, LifecycleOutcome::Retained);
    assert_eq!(reason.as_deref(), Some("kept: approve kept the worktree"));
    assert!(run.worktree.is_dir());
}

#[test]
fn policy_disabled_removal_records_a_keep() {
    let mut fx = Fx::new();
    fx.ctx.store.config.review.remove_worktree = Some(false);
    fx.owner_ready();
    let run = fx.run();
    corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::FastForward, true, false).unwrap();
    assert_eq!(intent(&fx), json!({"v": 1, "remove_worktree": false}));
    assert_eq!(lifecycle(&fx).0, LifecycleOutcome::Retained);
    assert!(run.worktree.is_dir());
}
