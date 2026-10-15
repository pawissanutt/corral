use super::*;

use crate::test_support as support;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Barrier, mpsc};
use std::time::Duration;

use corral_core::{AgentCmd, Status, Store, Verdict};
use corral_herdr::Herdr;
use serde_json::json;

const FAKE: &str = r#"#!/usr/bin/env bash
export CORRAL_ROOT="$(cd "$(dirname "$0")" && pwd)"
unset CORRAL_GATE_ATTEMPT CORRAL_CANDIDATE_HEAD CORRAL_INTEGRATION_BASE HERDR_PANE_ID HERDR_WORKSPACE_ID HERDR_TAB_ID
cd "$(dirname "$0")" || exit 1
if [ "${1:-}" = "--session" ]; then shift 2; fi
ok() { printf '{"id":"fake","result":%s}\n' "$1"; }
case "$1 $2" in
  "workspace create"|"tab create")
    n=1
    while [ -f "created-$n" ]; do n=$((n+1)); done
    touch "created-$n" "w1:p$n"
    ok "{\"workspace\":{\"workspace_id\":\"w1\"},\"root_pane\":{\"workspace_id\":\"w1\",\"tab_id\":\"w1:t$n\",\"pane_id\":\"w1:p$n\"}}"
    ;;
  "workspace list") ok '{"workspaces":[]}' ;;
  "pane list")
    panes='{"pane_id":"w0:p1"}'
    for pane in w1:*; do
      [ -f "$pane" ] || continue
      [ -z "$panes" ] || panes="$panes,"
      panes="$panes{\"pane_id\":\"$pane\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t1\"}"
    done
    ok "{\"panes\":[$panes]}" ;;
  "pane get")
    if [ -f "$3" ]; then
      ok "{\"pane\":{\"pane_id\":\"$3\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t1\"}}"
    else ok '{}'; fi
    ;;
  "pane close")
    if [ -f fail-close ]; then printf '{"error":{"code":"failed","message":"close failed"}}\n'; exit 1; fi
    rm -f "$3"; ok '{}'
    ;;
  "pane run")
    if [ -f fail-run ]; then printf '{"error":{"code":"failed","message":"run failed"}}\n'; exit 1; fi
    ok '{}'
    ;;
  *) ok '{}' ;;
esac
"#;

struct Fixture {
    tmp: tempfile::TempDir,
    ctx: Ctx,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = support::command("git", dir)
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().into()
}

impl Fixture {
    fn new(limit: usize) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        std::fs::create_dir(&app).unwrap();
        git(&app, &["init", "-q", "-b", "main"]);
        git(&app, &["config", "user.name", "test"]);
        git(&app, &["config", "user.email", "test@example.invalid"]);
        std::fs::write(app.join("tracked"), "initial\n").unwrap();
        git(&app, &["add", "."]);
        git(&app, &["commit", "-qm", "initial"]);
        let bin = tmp.path().join("herdr");
        std::fs::write(&bin, FAKE).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut store = Store::init(tmp.path()).unwrap();
        store.config.limits.review = limit;
        store.config.limits.running = 1;
        // These tests isolate the additional review budget; shared capacity is
        // exercised by the dispatch queue integration tests with defaults on.
        store.config.limits.count_reviews = false;
        store.config.limits.count_gates = false;
        store.config.dispatch.paused = true;
        store.config.review.gate = Some(vec!["true".into()]);
        store.config.agents.insert(
            "rev".into(),
            AgentCmd {
                command: "true".into(),
            },
        );
        store.save_config().unwrap();
        let ctx = Ctx {
            store,
            herdr: Herdr::with_bin(bin, None),
        };
        Self { tmp, ctx }
    }

    fn task(&self) -> String {
        let task = self
            .ctx
            .store
            .create("Review", None, Status::Review, "")
            .unwrap();
        let app = self.tmp.path().join("app");
        let run: Run = serde_json::from_value(json!({
            "id": "R1", "agent": "rev", "worktree": app,
            "branch": "main", "base": "main",
            "base_sha": git(&app, &["rev-parse", "HEAD"]),
            "started": "2026-10-15T00:00:00Z"
        }))
        .unwrap();
        self.ctx
            .store
            .update(&task.id, |t| {
                t.runs.push(run);
                Ok(())
            })
            .unwrap();
        task.id
    }

    fn created(&self) -> usize {
        std::fs::read_dir(self.tmp.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("created-")
            })
            .count()
    }

    fn mark(&self, name: &str) {
        std::fs::write(self.tmp.path().join(name), "").unwrap();
    }

    fn inventory(&self) -> std::collections::HashMap<String, corral_herdr::PaneInfo> {
        self.ctx
            .herdr
            .pane_list()
            .unwrap()
            .into_iter()
            .map(|pane| (pane.pane_id.clone(), pane))
            .collect()
    }
}

#[test]
fn mixed_services_share_budget_and_verdict_releases_capacity() {
    // Given two active review processes despite a single coding slot.
    let fx = Fixture::new(2);
    let ids: Vec<_> = (0..3).map(|_| fx.task()).collect();
    crate::gate::gate_start(&fx.ctx, &ids[0]).unwrap();
    crate::reviewer::review_run(&fx.ctx, &ids[1], Some("rev")).unwrap();

    // When a third service tries to launch.
    let error = crate::gate::gate_start(&fx.ctx, &ids[2]).unwrap_err();

    // Then no pane is created until the reviewer gives its verdict.
    assert!(error.to_string().contains("2/2"), "{error:#}");
    assert_eq!(fx.created(), 2);
    crate::reviewer::verdict(&fx.ctx, &ids[1], Verdict::Approve, "Reviewed.").unwrap();
    crate::gate::gate_start(&fx.ctx, &ids[2]).unwrap();
    assert_eq!(fx.created(), 3);
}

#[test]
fn same_task_gate_and_reviewer_each_consume_budget() {
    // Given an active gate with spare review capacity.
    let fx = Fixture::new(2);
    let id = fx.task();
    crate::gate::gate_start(&fx.ctx, &id).unwrap();

    // When the same task requests a reviewer.
    crate::reviewer::review_run(&fx.ctx, &id, Some("rev")).unwrap();

    // Then both processes count, leaving no capacity for another task.
    let other = fx.task();
    let error = crate::gate::gate_start(&fx.ctx, &other).unwrap_err();
    assert!(error.to_string().contains("2/2"), "{error:#}");
    assert_eq!(fx.created(), 2);
}

#[test]
fn admission_is_atomic_across_threads_and_service_kinds() {
    // Given one reservation held until the competing thread reports its result.
    let fx = Fixture::new(2);
    let first = fx.task();
    let second = fx.task();
    let barrier = Barrier::new(2);
    let (tx, rx) = mpsc::channel();

    // When another kind tries admission concurrently for this task and another task.
    std::thread::scope(|scope| {
        let launch = ReviewLaunch::reserve(&fx.ctx, &first, Kind::Gate).unwrap();
        scope.spawn(|| {
            barrier.wait();
            let denied = ReviewLaunch::reserve(&fx.ctx, &first, Kind::Reviewer).is_err()
                && ReviewLaunch::reserve(&fx.ctx, &second, Kind::Reviewer).is_err();
            tx.send(denied).unwrap();
        });
        barrier.wait();
        assert!(rx.recv_timeout(Duration::from_secs(5)).unwrap());
        drop(launch);
    });

    // Then release makes the same reservation available without a stale slot.
    ReviewLaunch::reserve(&fx.ctx, &second, Kind::Reviewer)
        .unwrap()
        .commit()
        .unwrap();
    assert_eq!(fx.created(), 0);
}

#[test]
fn concurrent_mixed_launches_never_exceed_the_bound() {
    // Given independent gate and reviewer requests sharing a two-process budget.
    let fx = Fixture::new(2);
    let ids: Vec<_> = (0..8).map(|_| fx.task()).collect();
    let barrier = Barrier::new(ids.len());

    // When both services launch together.
    let successes = std::thread::scope(|scope| {
        let handles: Vec<_> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let barrier = &barrier;
                let ctx = &fx.ctx;
                scope.spawn(move || {
                    barrier.wait();
                    if i % 2 == 0 {
                        crate::gate::gate_start(ctx, id).is_ok()
                    } else {
                        crate::reviewer::review_run(ctx, id, Some("rev")).is_ok()
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|success| *success)
            .count()
    });

    // Then every created pane corresponds to an admitted launch, within the bound.
    assert!((1..=2).contains(&successes), "{successes}");
    assert_eq!(fx.created(), successes);
}

#[test]
fn failed_launch_releases_record_and_reservation() {
    // Given a pane that refuses launch.
    let fx = Fixture::new(1);
    let first = fx.task();
    let second = fx.task();
    fx.mark("fail-run");

    // When the gate launch fails.
    assert!(crate::gate::gate_start(&fx.ctx, &first).is_err());

    // Then its record is terminal, its pane is closed and another task can launch.
    let task = fx.ctx.store.get(&first).unwrap();
    let gate = task.current_run().unwrap().gate.as_ref().unwrap();
    assert_eq!(gate.result, GateResult::Running);
    assert!(gate.process_lost);
    assert!(!fx.tmp.path().join("w1:p1").exists());
    std::fs::remove_file(fx.tmp.path().join("fail-run")).unwrap();
    crate::reviewer::review_run(&fx.ctx, &second, Some("rev")).unwrap();
}

#[test]
fn missing_pane_reconciles_record_before_admission() {
    // Given a reviewer whose pane disappeared without a verdict.
    let fx = Fixture::new(1);
    let first = fx.task();
    let second = fx.task();
    let record = crate::reviewer::review_run(&fx.ctx, &first, Some("rev")).unwrap();
    let panes = fx
        .ctx
        .herdr
        .pane_list()
        .unwrap()
        .into_iter()
        .map(|pane| (pane.pane_id.clone(), pane))
        .collect();
    reconcile(&fx.ctx, &panes, record.at).unwrap();
    std::fs::remove_file(fx.tmp.path().join(record.pane.unwrap())).unwrap();

    // When two successive inventories miss the previously listed pane.
    let now = Timestamp::now();
    let absent = fx.inventory();
    reconcile(&fx.ctx, &absent, now).unwrap();
    reconcile(&fx.ctx, &absent, now + jiff::SignedDuration::from_secs(1)).unwrap();
    // Then a different task can request a gate.
    crate::gate::gate_start(&fx.ctx, &second).unwrap();

    // Then capacity is released while the review identity remains available for recovery.
    let task = fx.ctx.store.get(&first).unwrap();
    let run = task.current_run().unwrap();
    let retained = run.review.as_ref().unwrap();
    assert!(retained.process_lost);
    assert!(!retained.holds_slot());
    assert!(retained.verdict.is_none());
    assert_eq!(retained.identity, record.identity);
    assert_eq!(retained.sha, record.sha);
    assert!(run.review_runs.last().unwrap().ended.is_some());
}

#[test]
fn crash_journal_closes_uncertain_process_before_reusing_capacity() {
    // Given a launcher that died after pane submission but before committing its journal.
    let fx = Fixture::new(1);
    let first = fx.task();
    let second = fx.task();
    let record = crate::gate::gate_start(&fx.ctx, &first).unwrap();
    let pane = record.pane.unwrap();
    save(
        &fx.ctx,
        &Pending {
            task: first.clone(),
            run: "R1".into(),
            kind: Kind::Gate,
            pane: Some(pane.clone()),
            close_pane: true,
            started: true,
        },
    )
    .unwrap();

    // When another task requests capacity.
    crate::reviewer::review_run(&fx.ctx, &second, Some("rev")).unwrap();

    // Then recovery closes the uncertain pane and clears the journal.
    assert!(!fx.tmp.path().join(pane).exists());
    assert!(!journal(&fx.ctx).exists());
    let task = fx.ctx.store.get(&first).unwrap();
    let gate = task.current_run().unwrap().gate.as_ref().unwrap();
    assert_eq!(gate.result, GateResult::Running);
    assert!(gate.process_lost);
}

#[test]
fn failed_cleanup_keeps_journal_and_blocks_new_processes() {
    // Given a failed launch whose pane cannot be closed.
    let fx = Fixture::new(1);
    let first = fx.task();
    let second = fx.task();
    fx.mark("fail-run");
    fx.mark("fail-close");
    assert!(crate::gate::gate_start(&fx.ctx, &first).is_err());

    // When a second task tries admission before cleanup succeeds.
    assert!(crate::reviewer::review_run(&fx.ctx, &second, Some("rev")).is_err());

    // Then the uncertain process still owns its slot and no second pane exists.
    assert!(journal(&fx.ctx).exists());
    assert_eq!(fx.created(), 1);
}

#[test]
fn completed_gate_result_releases_capacity_with_pane_still_open() {
    // Given a completed gate not yet collected by sync.
    let fx = Fixture::new(1);
    let first = fx.task();
    let second = fx.task();
    crate::gate::gate_start(&fx.ctx, &first).unwrap();
    let output = support::command("bash", fx.tmp.path())
        .arg(crate::gate::gate_paths(&fx.ctx, &first, "R1").script)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");

    // When another service requests capacity.
    crate::reviewer::review_run(&fx.ctx, &second, Some("rev")).unwrap();

    // Then the finished gate is collected and does not consume a slot.
    assert_eq!(
        fx.ctx
            .store
            .get(&first)
            .unwrap()
            .current_run()
            .unwrap()
            .gate
            .as_ref()
            .unwrap()
            .result,
        GateResult::Pass
    );
    assert!(fx.tmp.path().join("w1:p1").exists());
}

#[test]
fn failed_reviewer_launch_closes_its_accounting_window() {
    // Given a reviewer pane that refuses launch.
    let fx = Fixture::new(1);
    let first = fx.task();
    let second = fx.task();
    fx.mark("fail-run");

    // When the reviewer fails to launch.
    assert!(crate::reviewer::review_run(&fx.ctx, &first, Some("rev")).is_err());

    // Then it has no active review or window and leaves capacity for a gate.
    let task = fx.ctx.store.get(&first).unwrap();
    let run = task.current_run().unwrap();
    assert!(run.review.is_none());
    assert!(run.review_runs.last().unwrap().ended.is_some());
    std::fs::remove_file(fx.tmp.path().join("fail-run")).unwrap();
    crate::gate::gate_start(&fx.ctx, &second).unwrap();
}

#[test]
fn shared_pm_crash_recovery_retains_capacity_without_closing_pm() {
    // Given a prompt that may have reached the shared PM before a launcher crash.
    let mut fx = Fixture::new(1);
    fx.ctx.store.config.pm.agent = "rev".into();
    fx.ctx.store.save_config().unwrap();
    let first = fx.task();
    let second = fx.task();
    fx.mark("w1:pm");
    std::fs::write(
        fx.ctx.store.state_dir().join("pm.json"),
        r#"{"pane_id":"w1:pm"}"#,
    )
    .unwrap();
    crate::reviewer::review_run(&fx.ctx, &first, Some("pm")).unwrap();
    save(
        &fx.ctx,
        &Pending {
            task: first.clone(),
            run: "R1".into(),
            kind: Kind::Reviewer,
            pane: Some("w1:pm".into()),
            close_pane: false,
            started: true,
        },
    )
    .unwrap();

    // When another task requests capacity while delivery is uncertain.
    let error = crate::gate::gate_start(&fx.ctx, &second).unwrap_err();

    // Then PM remains alive and budgeted until the verdict releases its slot.
    assert!(error.to_string().contains("1/1"), "{error:#}");
    assert!(fx.tmp.path().join("w1:pm").exists());
    assert!(!journal(&fx.ctx).exists());
    crate::reviewer::verdict(&fx.ctx, &first, Verdict::Approve, "Reviewed.").unwrap();
    crate::gate::gate_start(&fx.ctx, &second).unwrap();
}

#[test]
fn invalid_replacement_does_not_release_existing_review() {
    // Given an active reviewer.
    let fx = Fixture::new(1);
    let id = fx.task();
    let original = crate::reviewer::review_run(&fx.ctx, &id, Some("rev")).unwrap();

    // When an invalid reviewer replacement is requested.
    assert!(crate::reviewer::review_run(&fx.ctx, &id, Some("missing")).is_err());

    // Then validation has not consumed or released its existing record or pane.
    assert_eq!(
        fx.ctx
            .store
            .get(&id)
            .unwrap()
            .current_run()
            .unwrap()
            .review
            .as_ref(),
        Some(&original)
    );
    assert!(fx.tmp.path().join(original.pane.unwrap()).exists());
    assert_eq!(fx.created(), 1);
}

#[test]
fn seen_gate_releases_and_escalates_only_after_two_inventory_misses() {
    // Given a running gate seen by sync, with its pane still open.
    let mut fx = Fixture::new(1);
    fx.ctx.store.config.review.autopilot = Some(true);
    fx.ctx.store.save_config().unwrap();
    let id = fx.task();
    let gate = crate::gate::gate_start(&fx.ctx, &id).unwrap();
    fx.ctx
        .store
        .update(&id, |task| {
            let run = task.current_run_mut().unwrap();
            run.outcome = Some(corral_core::RunOutcome::Done);
            run.ended = Some(gate.at);
            Ok(())
        })
        .unwrap();
    crate::sync_at(&fx.ctx, gate.at).unwrap();
    let pane = gate.pane.unwrap();
    std::fs::remove_file(fx.tmp.path().join(pane)).unwrap();

    // When one sync misses the pane, then a second independent sync misses it.
    crate::sync_at(&fx.ctx, gate.at + jiff::SignedDuration::from_secs(1)).unwrap();
    let first = fx.ctx.store.get(&id).unwrap();
    assert_eq!(first.status, Status::Review);
    assert!(
        first
            .current_run()
            .unwrap()
            .gate
            .as_ref()
            .unwrap()
            .holds_slot()
    );
    crate::sync_at(&fx.ctx, gate.at + jiff::SignedDuration::from_secs(2)).unwrap();

    // Then capacity is released without inventing a failed gate receipt, and the owner
    // receives the gate-disappearance reason rather than a command-failure reason.
    let task = fx.ctx.store.get(&id).unwrap();
    assert_eq!(task.status, Status::NeedsMe);
    let run = task.current_run().unwrap();
    let retained = run.gate.as_ref().unwrap();
    assert!(retained.process_lost);
    assert!(!retained.holds_slot());
    assert_eq!(retained.result, GateResult::Running);
    assert_eq!(
        run.autopilot.as_ref().unwrap().reason.as_deref(),
        Some("the gate pane closed before the gate finished")
    );
    let released: Vec<_> = fx
        .ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|event| event.kind == "review_released")
        .collect();
    assert_eq!(released.len(), 1);
    assert_eq!(released[0].detail["kind"], "gate");
    assert_eq!(released[0].detail["reason"], "pane gone");
}

#[test]
fn unseen_gate_uses_configured_startup_deadline() {
    // Given a gate never listed, with a non-default five-second startup grace.
    let mut fx = Fixture::new(1);
    fx.ctx.store.config.sync.startup_grace_secs = 5;
    fx.ctx.store.save_config().unwrap();
    let id = fx.task();
    let gate = crate::gate::gate_start(&fx.ctx, &id).unwrap();
    std::fs::remove_file(fx.tmp.path().join(gate.pane.unwrap())).unwrap();
    let absent = fx.inventory();

    // When repeated inventories miss the launch before and at the grace deadline.
    for seconds in [1, 2, 4] {
        reconcile(
            &fx.ctx,
            &absent,
            gate.at + jiff::SignedDuration::from_secs(seconds),
        )
        .unwrap();
        assert!(
            fx.ctx
                .store
                .get(&id)
                .unwrap()
                .current_run()
                .unwrap()
                .gate
                .as_ref()
                .unwrap()
                .holds_slot()
        );
    }
    reconcile(
        &fx.ctx,
        &absent,
        gate.at + jiff::SignedDuration::from_secs(5),
    )
    .unwrap();

    // Then misses cannot shorten startup, but its deadline still releases capacity.
    assert!(
        fx.ctx
            .store
            .get(&id)
            .unwrap()
            .current_run()
            .unwrap()
            .gate
            .as_ref()
            .unwrap()
            .process_lost
    );
}

#[test]
fn reviewer_single_miss_survives_watchdog_and_budget_in_the_same_sync() {
    // Given a listed working reviewer, older than startup grace.
    let fx = Fixture::new(1);
    let id = fx.task();
    let record = crate::reviewer::review_run(&fx.ctx, &id, Some("rev")).unwrap();
    let now = record.at + jiff::SignedDuration::from_secs(60);
    crate::sync_at(&fx.ctx, now).unwrap();
    let pane = record.pane.unwrap();
    std::fs::remove_file(fx.tmp.path().join(&pane)).unwrap();

    // When one sync traverses both liveness consumers with the same missing sample.
    crate::sync_at(&fx.ctx, now + jiff::SignedDuration::from_secs(1)).unwrap();
    let task = fx.ctx.store.get(&id).unwrap();
    assert_eq!(task.status, Status::Review);
    assert!(
        task.current_run()
            .unwrap()
            .review
            .as_ref()
            .unwrap()
            .holds_slot()
    );
    fx.mark(&pane);
    crate::sync_at(&fx.ctx, now + jiff::SignedDuration::from_secs(2)).unwrap();

    // Then neither capacity release nor a false owner alert survives the recovered listing.
    assert_eq!(fx.ctx.store.get(&id).unwrap().status, Status::Review);
    assert!(
        !fx.ctx
            .store
            .events()
            .unwrap()
            .iter()
            .any(|event| matches!(event.kind.as_str(), "review_released" | "review_escalated"))
    );
}

#[test]
fn admission_probe_cannot_count_a_sync_miss_twice() {
    // Given a reviewer seen in a previous inventory.
    let fx = Fixture::new(1);
    let id = fx.task();
    let record = crate::reviewer::review_run(&fx.ctx, &id, Some("rev")).unwrap();
    crate::sync_at(&fx.ctx, Timestamp::now()).unwrap();
    std::fs::remove_file(fx.tmp.path().join(record.pane.unwrap())).unwrap();
    crate::sync_at(&fx.ctx, Timestamp::now()).unwrap();
    let next = fx.task();

    // When admission probes the same missing reviewer before another sync.
    let error = crate::gate::gate_start(&fx.ctx, &next).unwrap_err();

    // Then the single missing sync still owns capacity.
    assert!(error.to_string().contains("1/1"), "{error:#}");
    assert!(
        fx.ctx
            .store
            .get(&id)
            .unwrap()
            .current_run()
            .unwrap()
            .review
            .as_ref()
            .unwrap()
            .holds_slot()
    );
}
