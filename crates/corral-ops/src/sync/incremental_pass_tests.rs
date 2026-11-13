//! The daemon's planner-gated pass (incremental contract section 9, tests 1-5) on a small store
//! with a fake herdr whose pane list a test rewrites.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;

use corral_core::Store;
use corral_core::work::{self, Counter};
use corral_herdr::Herdr;

use super::*;

const HERDR: &str = r#"#!/bin/sh
if [ "$1" = "--session" ]; then shift 2; fi
case "$1 $2" in
  "workspace list") echo '{"result":{"workspaces":[]}}' ;;
  "pane list") printf '{"result":{"panes":[%s]}}\n' "$(cat "$(dirname "$0")/panes")" ;;
  "pane process-info") echo '{"result":{"process_info":{"shell_pid":10,"foreground_processes":[]}}}' ;;
  "pane read") echo 'idle' ;;
  "notification show") echo '{"result":{}}' ;;
  *) exit 1 ;;
esac
"#;

/// An unowned pane, so the inventory is never empty (an empty one is UNKNOWN).
const OTHER: &str = r#"{"pane_id":"w9:p9","workspace_id":"w9","tab_id":"w9:t1","agent":"fixture","agent_status":"idle"}"#;

/// Obligations, the head loop and plan autopilot; no native or Review heads exist here.
const PER_TASK_STEPS: usize = 3;

struct Fx {
    dir: tempfile::TempDir,
    ctx: Ctx,
    memory: PassMemory,
    audit: bool,
    t0: Timestamp,
    open: Vec<String>,
    worker_id: Option<String>,
}

impl Fx {
    /// `open` Backlog and `terminal` Done tasks, plus a Running worker on pane `w1:p1` whose
    /// herdr status is `pane`.
    fn new(open: usize, terminal: usize, pane: Option<&str>, audit: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("herdr");
        std::fs::write(&bin, HERDR).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let herdr = Herdr::with_bin(bin, Some("corral-test".into()));
        let store = Store::init(dir.path()).unwrap();
        let open = (0..open)
            .map(|i| {
                let title = format!("Open {i}");
                store.create(&title, None, Status::Backlog, "").unwrap().id
            })
            .collect();
        for i in 0..terminal {
            store
                .create(&format!("Done {i}"), None, Status::Done, "")
                .unwrap();
        }
        let t0 = Timestamp::now();
        let worker_id = pane.map(|_| {
            // A watchable worktree: an unwatchable one counts as moved on every pass.
            let worktree = dir.path().join("wt");
            std::fs::create_dir_all(worktree.join(".git/logs")).unwrap();
            std::fs::write(worktree.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
            store
                .create_initialized("Worker", None, Status::Running, "", |task| {
                    task.runs.push(serde_json::from_value(json!({
                        "id":"R1", "agent":"fixture", "worktree":worktree, "branch":"main",
                        "base":"main", "base_sha":"base", "started":t0,
                        "seen_working":true, "seen_pane":true, "agent_state":"working",
                        "herdr":{"workspace_id":"w1","pane_id":"w1:p1","socket":herdr.socket_path()?}
                    }))?);
                    Ok(())
                })
                .unwrap()
                .id
        });
        let mut memory = PassMemory::new();
        memory.set_audit(audit);
        let fx = Self {
            ctx: Ctx {
                store,
                herdr,
                resource_probe: crate::resources::FixedResourceProbe::arc(),
            },
            dir,
            memory,
            audit,
            t0,
            open,
            worker_id,
        };
        fx.set_panes(pane);
        fx
    }

    /// The worker's pane (absent when `None`) and the unowned pane.
    fn set_panes(&self, worker: Option<&str>) {
        let mut panes = vec![OTHER.to_owned()];
        panes.extend(worker.map(|status| {
            format!(
                r#"{{"pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1","agent":"fixture","agent_status":"{status}"}}"#
            )
        }));
        std::fs::write(self.dir.path().join("panes"), panes.join(",")).unwrap();
    }

    /// One daemon pass at `t0 + secs`; no pass may skip a step that then acts.
    fn pass(&mut self, secs: i64) -> PassReport {
        let now = self.t0 + SignedDuration::from_secs(secs);
        let (_, _, report) =
            sync_incremental_at(&self.ctx, &mut self.memory, PassRequest::Paced, now).unwrap();
        assert!(
            report.wrong_skips.is_empty(),
            "pass at {secs}s skipped steps that acted: {:?}",
            report.wrong_skips
        );
        report
    }

    /// Passes one second apart from `from` until one is quiet (audit passes are always full,
    /// so three of them); returns the second after the last.
    fn settle(&mut self, from: i64) -> i64 {
        if self.audit {
            for secs in from..from + 3 {
                self.pass(secs);
            }
            return from + 3;
        }
        for secs in from..from + 6 {
            if self.pass(secs).quiet {
                return secs + 1;
            }
        }
        panic!("passes from {from}s never became quiet");
    }

    /// The worker's status, agent state, idle start and idle badge.
    fn worker(&self) -> (Status, Option<String>, Option<Timestamp>, bool) {
        let id = self.worker_id.as_deref().unwrap();
        let head = self.ctx.store.head(id).unwrap();
        let run = head.current_run.as_deref().unwrap();
        (
            head.status,
            run.agent_state.clone(),
            run.idle_since,
            run.idle_no_report,
        )
    }
}

#[test]
fn quiet_incremental_pass_skips_every_gated_step() {
    let quiet_work = |open, terminal| {
        let mut fx = Fx::new(open, terminal, None, false);
        let at = fx.settle(1);
        let before = work::snapshot();
        let report = fx.pass(at);
        let spent = work::snapshot().since(&before);
        assert!(report.quiet && report.full.is_none(), "{report:?}");
        Counter::ALL
            .iter()
            .map(|counter| (counter.name(), spent.total(*counter)))
            .collect::<Vec<_>>()
    };
    // Given 2 vs 4 open tasks and 1 vs 3 terminal ones, a quiet pass does the same work.
    assert_eq!(quiet_work(2, 1), quiet_work(4, 3));
    let mut audited = Fx::new(4, 3, None, true);
    let at = audited.settle(1);
    audited.pass(at);
}

#[test]
fn incremental_pass_converges_after_own_writes() {
    for audit in [false, true] {
        let mut fx = Fx::new(1, 1, Some("working"), audit);
        let at = fx.settle(1);
        // When the worker's pane turns idle, the next pass records it.
        fx.set_panes(Some("idle"));
        let wrote = fx.pass(at);
        assert_eq!(fx.worker().1.as_deref(), Some("idle"), "audit={audit}");
        let echo = fx.pass(at + 1);
        let after = fx.pass(at + 2);
        // Then at most one echo pass follows before passes are quiet again.
        if !audit {
            assert!(!wrote.quiet, "{wrote:?}");
            assert!(after.quiet, "echo {echo:?}, then {after:?}");
        }
    }
}

#[test]
fn changed_task_alone_reruns_head_steps() {
    let changed = |open, audit| {
        let mut fx = Fx::new(open, 1, None, audit);
        let at = fx.settle(1);
        let id = fx.open[0].clone();
        fx.ctx
            .store
            .update(&id, |task| {
                task.title = "Renamed".into();
                Ok(())
            })
            .unwrap();
        fx.pass(at)
    };
    let (small, large) = (changed(2, false), changed(4, false));
    assert!(
        small.full.is_none() && large.full.is_none(),
        "{small:?} {large:?}"
    );
    // Then only the changed task's per-task steps ran, whatever the number of open tasks.
    assert_eq!(small.ran, large.ran, "{small:?} {large:?}");
    assert_eq!(large.skipped - small.skipped, 2 * PER_TASK_STEPS);
    changed(4, true);
}

#[test]
fn idle_threshold_acts_without_store_change() {
    for audit in [false, true] {
        let mut fx = Fx::new(0, 0, Some("idle"), audit);
        let at = fx.settle(1);
        let (_, _, since, flagged) = fx.worker();
        assert_eq!(
            since,
            Some(fx.t0 + SignedDuration::from_secs(1)),
            "audit={audit}"
        );
        assert!(!flagged);
        // When the clock passes the idle threshold with nothing written since.
        let after = i64::try_from(fx.ctx.store.config.sync.idle_after_secs).unwrap();
        assert!(1 + after > at);
        let report = fx.pass(1 + after);
        // Then the threshold's due time runs the worker's reconciliation.
        assert!(fx.worker().3, "audit={audit} {report:?}");
        if !audit {
            assert!(report.full.is_none(), "{report:?}");
        }
    }
}

#[test]
fn pane_miss_reaches_second_sample_without_inventory_change() {
    for audit in [false, true] {
        let mut fx = Fx::new(0, 0, Some("working"), audit);
        let at = fx.settle(1);
        // When the worker's pane vanishes and two passes see the same inventory.
        fx.set_panes(None);
        let first = fx.pass(at);
        assert_eq!(fx.worker().0, Status::Running, "audit={audit}");
        let second = fx.pass(at + 1);
        // Then the second consecutive miss moves the task to Needs me.
        assert_eq!(fx.worker().0, Status::NeedsMe, "audit={audit}");
        if !audit {
            assert!(first.full.is_none() && second.full.is_none());
        }
    }
}
