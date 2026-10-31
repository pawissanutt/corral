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
fn retained_branch_cleanup_refuses_missing_changed_or_unpreserved_approval_after_restart() {
    use corral_core::records::RecordArea;
    const APPROVALS: RecordArea<Value> = RecordArea::new("worktree-cleanup-approvals");
    for fault in [
        "missing",
        "changed_intent",
        "changed_branch",
        "changed_worktree",
        "lost_commit",
        "moved_worktree",
    ] {
        let mut fx = Fx::new();
        fx.owner_ready();
        let run = fx.run();
        let holder = Holder::start(&run.worktree);
        let line =
            corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::KeepBranch, true, false).unwrap();
        assert!(line.contains("protected process"), "{fault}: {line}");
        drop(holder);
        let key = format!("{}/{}", fx.id, run.id);
        let mut approval = APPROVALS.load(&fx.ctx.store, &key).unwrap().unwrap();
        assert_eq!(approval["remove_worktree"], true);
        assert_eq!(approval["keep_branch_strategy"], true);
        let approved = approval["sha"].as_str().unwrap().to_owned();
        match fault {
            "missing" => {
                fx.ctx
                    .store
                    .delete_record("worktree-cleanup-approvals", &key)
                    .unwrap();
            }
            "changed_intent" => {
                approval["remove_worktree"] = json!(false);
                APPROVALS.save(&fx.ctx.store, &key, &approval).unwrap();
            }
            "changed_branch" => {
                approval["branch"] = json!("not-the-approved-branch");
                APPROVALS.save(&fx.ctx.store, &key, &approval).unwrap();
            }
            "changed_worktree" => {
                approval["worktree"] = json!(fx.app.join(".worktrees/not-the-approved-worktree"));
                APPROVALS.save(&fx.ctx.store, &key, &approval).unwrap();
            }
            "lost_commit" | "moved_worktree" => {
                let tree = git(&run.worktree, &["rev-parse", "HEAD^{tree}"]);
                // Same tree, clean checkout: refusing cannot be attributed to dirt.
                let replacement = if fault == "lost_commit" {
                    git(
                        &fx.app,
                        &["commit-tree", &tree, "-m", "unrelated same-tree root"],
                    )
                } else {
                    git(
                        &fx.app,
                        &[
                            "commit-tree",
                            &tree,
                            "-p",
                            &approved,
                            "-m",
                            "unapproved descendant",
                        ],
                    )
                };
                git(
                    &fx.app,
                    &[
                        "update-ref",
                        &format!("refs/heads/{}", run.branch),
                        &replacement,
                    ],
                );
                assert!(git(&run.worktree, &["status", "--porcelain"]).is_empty());
                assert_eq!(
                    corral_git::is_ancestor(&fx.app, &approved, &run.branch).unwrap(),
                    fault == "moved_worktree"
                );
            }
            _ => unreachable!(),
        }
        // Restart the consumer: only canonical recorded authority may permit the retry.
        fx.ctx.store = Store::open(fx.ctx.store.root()).unwrap();
        let (outcome, reason) = lifecycle(&fx);
        assert_ne!(outcome, LifecycleOutcome::Reclaimed, "{fault}: {reason:?}");
        let reason = reason.unwrap();
        let expected = match fault {
            "missing" => "missing recorded",
            "changed_intent" | "changed_branch" | "changed_worktree" => "approval changed",
            "lost_commit" => "no longer preserves",
            "moved_worktree" => "HEAD differs",
            _ => unreachable!(),
        };
        assert!(reason.contains(expected), "{fault}: {reason}");
        assert!(run.worktree.is_dir(), "{fault}: worktree retained");
        assert!(
            corral_git::rev_sha(&fx.app, &run.branch).is_ok(),
            "{fault}: named branch retained"
        );
    }
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
