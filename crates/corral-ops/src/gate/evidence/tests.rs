//! Gate transitions commit atomically: an interrupted collection commits nothing, and the next
//! start collects the attempt whole. The fake herdr records pane commands without running
//! them; each test runs the gate script itself, as the pane would.
use std::os::unix::fs::PermissionsExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};

use corral_core::backend::ScratchState;
use corral_core::{GateResult, Run, Status, Store};
use corral_herdr::Herdr;
use serde_json::json;

use super::FAILPOINT;
use crate::Ctx;
use crate::test_support as support;

const HERDR: &str = r#"#!/usr/bin/env bash
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
      panes="$panes,{\"pane_id\":\"$pane\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t1\"}"
    done
    ok "{\"panes\":[$panes]}" ;;
  "pane get")
    if [ -f "$3" ]; then
      ok "{\"pane\":{\"pane_id\":\"$3\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t1\"}}"
    else ok '{}'; fi
    ;;
  "pane close") rm -f "$3"; ok '{}' ;;
  *) ok '{}' ;;
esac
"#;

struct Fx {
    _tmp: tempfile::TempDir,
    ctx: Ctx,
    app: PathBuf,
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

impl Fx {
    fn new() -> Self {
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
        std::fs::write(&bin, HERDR).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut store = Store::init(tmp.path()).unwrap();
        store.config.limits.count_reviews = false;
        store.config.limits.count_gates = false;
        store.config.dispatch.paused = true;
        store.config.review.gate = Some(vec!["echo collected".into()]);
        store.config.projects.insert(
            "app".into(),
            serde_json::from_value(json!({"path": "app"})).unwrap(),
        );
        store.save_config().unwrap();
        let ctx = Ctx {
            resource_probe: crate::resources::FixedResourceProbe::arc(),
            store,
            herdr: Herdr::with_bin(bin, None),
        };
        Self {
            _tmp: tmp,
            ctx,
            app,
        }
    }

    fn task(&self) -> String {
        let task = self
            .ctx
            .store
            .create("Gate", Some("app"), Status::Review, "")
            .unwrap();
        let run: Run = serde_json::from_value(json!({
            "id": "R1", "agent": "rev", "worktree": self.app,
            "branch": "main", "base": "main",
            "base_sha": git(&self.app, &["rev-parse", "HEAD"]),
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

    /// Runs the current attempt's script to completion, as its pane would; it exits with the
    /// failing command's status, which the result file records.
    fn run_gate(&self, id: &str) {
        let script = crate::gate::gate_paths(&self.ctx, id, "R1").script;
        support::command("bash", &self.app)
            .arg(&script)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(
            crate::gate::gate_paths(&self.ctx, id, "R1")
                .result
                .is_file()
        );
    }

    fn gate_events(&self, id: &str) -> Vec<serde_json::Value> {
        self.ctx
            .store
            .events()
            .unwrap()
            .into_iter()
            .filter(|e| e.task.as_deref() == Some(id) && e.kind == "gate")
            .map(|e| e.detail)
            .collect()
    }

    fn scratch(&self, id: &str, attempt: &str) -> ScratchState {
        self.ctx
            .store
            .scratch_row(&format!("gate-{id}-R1-{attempt}"))
            .unwrap()
            .unwrap()
            .state
    }
}

/// Runs `op` with a hook that panics when `point` is reached; true when it was interrupted.
fn interrupt_at<T>(point: &'static str, op: impl FnOnce() -> T) -> bool {
    FAILPOINT.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |reached: &str| {
            if reached == point {
                panic!("interrupted at {reached}");
            }
        }));
    });
    let interrupted = catch_unwind(AssertUnwindSafe(op)).is_err();
    FAILPOINT.with(|hook| *hook.borrow_mut() = None);
    interrupted
}

#[test]
fn infrastructure_recovery_keeps_round_and_restarts_review() {
    let mut fx = Fx::new();
    fx.ctx.store.config.agents.insert(
        "rev".into(),
        corral_core::AgentCmd {
            command: "true".into(),
        },
    );
    fx.ctx.store.save_config().unwrap();
    let id = fx.task();
    crate::gate::gate_start(&fx.ctx, &id).unwrap();
    fx.run_gate(&id);
    crate::gate::gate_collect(&fx.ctx, &id).unwrap();
    crate::reviewer::review_run(&fx.ctx, &id, Some("rev")).unwrap();
    fx.ctx
        .store
        .update(&id, |task| {
            task.status = Status::NeedsMe;
            task.rounds = 2;
            let run = task.current_run_mut().unwrap();
            run.outcome = Some(corral_core::RunOutcome::Done);
            run.autopilot = Some(corral_core::AutopilotState {
                stage: corral_core::AutoStage::Escalated,
                reason: Some(
                    "integration preflight error: Git operation ongoing: index.lock".into(),
                ),
                sha: None,
                at: corral_core::clock::now(),
            });
            Ok(())
        })
        .unwrap();

    fx.ctx
        .store
        .update(&id, |task| {
            task.current_run_mut()
                .unwrap()
                .autopilot
                .as_mut()
                .unwrap()
                .reason = Some("integration preflight error: merge conflict".into());
            Ok(())
        })
        .unwrap();
    assert!(crate::autopilot::recover_infrastructure(&fx.ctx, &id).is_err());
    fx.ctx
        .store
        .update(&id, |task| {
            task.current_run_mut()
                .unwrap()
                .gate
                .as_mut()
                .unwrap()
                .result = GateResult::Fail;
            task.status = Status::Review;
            task.current_run_mut()
                .unwrap()
                .review
                .as_mut()
                .unwrap()
                .process_lost = true;
            task.infra_wait = Some(corral_core::InfraWait {
                version: 1,
                class: corral_core::InfraWaitClass::Contention,
                marker: "index.lock".into(),
                path: fx.app.join(".git/index.lock"),
                first_seen: corral_core::clock::now(),
                first_mtime: None,
                held_at: None,
            });
            Ok(())
        })
        .unwrap();
    assert!(crate::autopilot::recover_infrastructure(&fx.ctx, &id).is_err());
    fx.ctx
        .store
        .update(&id, |task| {
            task.current_run_mut()
                .unwrap()
                .gate
                .as_mut()
                .unwrap()
                .result = GateResult::Pass;
            Ok(())
        })
        .unwrap();
    crate::autopilot::recover_infrastructure(&fx.ctx, &id).unwrap();
    let task = fx.ctx.store.get(&id).unwrap();
    assert_eq!(task.status, Status::Review);
    assert!(task.infra_wait.is_some());
    assert!(task.current_run().unwrap().review.is_none());
    assert_eq!(task.rounds, 2);
    fx.ctx
        .store
        .update(&id, |task| {
            task.status = Status::NeedsMe;
            task.infra_wait = None;
            task.current_run_mut()
                .unwrap()
                .autopilot
                .as_mut()
                .unwrap()
                .stage = corral_core::AutoStage::Escalated;
            task.current_run_mut()
                .unwrap()
                .autopilot
                .as_mut()
                .unwrap()
                .reason =
                Some("integration preflight error: Git operation ongoing: index.lock".into());
            Ok(())
        })
        .unwrap();
    crate::autopilot::recover_infrastructure(&fx.ctx, &id).unwrap();
    let task = fx.ctx.store.get(&id).unwrap();
    assert_eq!(task.status, Status::Review);
    assert_eq!(task.rounds, 2);
    assert_eq!(
        task.current_run()
            .unwrap()
            .autopilot
            .as_ref()
            .unwrap()
            .stage,
        corral_core::AutoStage::Reviewing
    );
    crate::reviewer::review_run(&fx.ctx, &id, Some("rev")).unwrap();
    let task = fx.ctx.store.get(&id).unwrap();
    assert_eq!(task.rounds, 2);
    assert!(
        task.current_run()
            .unwrap()
            .review
            .as_ref()
            .is_some_and(|review| review.verdict.is_none())
    );
    crate::reviewer::verdict(&fx.ctx, &id, corral_core::Verdict::Approve, "Reviewed").unwrap();
    let task = fx.ctx.store.get(&id).unwrap();
    assert_eq!(task.rounds, 2);
    assert_eq!(
        task.current_run().unwrap().review.as_ref().unwrap().verdict,
        Some(corral_core::Verdict::Approve)
    );
}

#[test]
fn interrupted_collection_commits_nothing_and_the_next_start_collects_it_whole() {
    // Given a started gate whose runner finished.
    let fx = Fx::new();
    let id = fx.task();
    let started = crate::gate::gate_start(&fx.ctx, &id).unwrap();
    let attempt = started.identity.unwrap().attempt;
    fx.run_gate(&id);

    // When its collection is interrupted at the commit.
    assert!(interrupt_at("gate_collect_commit", || {
        crate::gate::gate_collect(&fx.ctx, &id)
    }));

    // Then neither the receipt, its event, the row nor the scratch closure was committed.
    let task = fx.ctx.store.get(&id).unwrap();
    let gate = task.current_run().unwrap().gate.clone().unwrap();
    assert_eq!(gate.result, GateResult::Running);
    assert!(fx.gate_events(&id).is_empty());
    let row = fx
        .ctx
        .store
        .gate_attempt(&id, "R1", &attempt)
        .unwrap()
        .unwrap();
    assert!(row.ended.is_none() && row.result.is_none() && row.log.is_none());
    assert_eq!(fx.scratch(&id, &attempt), ScratchState::Open);

    // When a new gate starts on the same candidate.
    crate::gate::gate_start(&fx.ctx, &id).unwrap();

    // Then the interrupted attempt was collected whole: one event, and a row whose outcome
    // and evidence match the receipt, before its closed directory was reclaimed.
    let events = fx.gate_events(&id);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["attempt"], json!(attempt));
    assert_eq!(events[0]["result"], "pass");
    let row = fx
        .ctx
        .store
        .gate_attempt(&id, "R1", &attempt)
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.result.as_deref(), row.failed_cmd.as_deref()),
        (Some("pass"), None)
    );
    let log = fx
        .ctx
        .store
        .blob_bytes(row.log.as_ref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(
        events[0]["log_sha256"],
        json!(crate::verification::digest(&log))
    );
    assert_eq!(fx.scratch(&id, &attempt), ScratchState::Reclaimed);
}

#[test]
fn every_collected_receipt_matches_its_row() {
    // Given passing and failing gates collected normally.
    for (command, result, failed) in [
        ("echo fine", "pass", None),
        ("echo boom; exit 3", "fail", Some("echo boom; exit 3")),
    ] {
        let fx = Fx::new();
        let mut store = fx.ctx.store.reopen().unwrap();
        store.config.review.gate = Some(vec![command.into()]);
        store.save_config().unwrap();
        let id = fx.task();
        let started = crate::gate::gate_start(&fx.ctx, &id).unwrap();
        fx.run_gate(&id);
        let record = crate::gate::gate_collect(&fx.ctx, &id).unwrap().unwrap();

        // Then the receipt, its event and the closed row agree.
        assert_eq!(record.result.as_str(), result);
        assert_eq!(record.failed_cmd.as_deref(), failed);
        let attempt = started.identity.unwrap().attempt;
        let row = fx
            .ctx
            .store
            .gate_attempt(&id, "R1", &attempt)
            .unwrap()
            .unwrap();
        assert_eq!(
            (row.result.as_deref(), row.failed_cmd.as_deref()),
            (Some(result), failed)
        );
        assert_eq!(fx.scratch(&id, &attempt), ScratchState::Closed);
        assert_eq!(fx.gate_events(&id)[0]["result"], result);
    }
}

fn rows(fx: &Fx, id: &str) -> Vec<corral_core::backend::GateAttempt> {
    let mut page = corral_core::backend::PageReq::first(64);
    fx.ctx
        .store
        .task_rows(
            id,
            corral_core::backend::TaskPart::GateAttempt,
            None,
            None,
            &mut page,
        )
        .unwrap()
        .into_iter()
        .map(|doc| serde_json::from_value(doc).unwrap())
        .collect()
}

fn started_events(fx: &Fx, id: &str) -> Vec<serde_json::Value> {
    fx.ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.task.as_deref() == Some(id) && e.kind == "gate_started")
        .map(|e| e.detail)
        .collect()
}

#[test]
fn interrupted_start_commits_nothing_and_the_next_start_commits_marker_row_and_event_once() {
    // Given a run with no gate yet.
    let fx = Fx::new();
    let id = fx.task();

    // When its first gate start is interrupted at the commit.
    assert!(interrupt_at("gate_start_commit", || {
        crate::gate::gate_start(&fx.ctx, &id)
    }));

    // Then neither the marker, a row nor a start event was committed.
    let task = fx.ctx.store.get(&id).unwrap();
    assert!(task.current_run().unwrap().gate.is_none());
    assert!(rows(&fx, &id).is_empty());
    assert!(started_events(&fx, &id).is_empty());

    // When the gate starts again.
    let started = crate::gate::gate_start(&fx.ctx, &id).unwrap();

    // Then the marker, its one row and its one start event name the same attempt.
    let attempt = started.identity.unwrap().attempt;
    let task = fx.ctx.store.get(&id).unwrap();
    let marker = task.current_run().unwrap().gate.clone().unwrap();
    assert_eq!(marker.identity.unwrap().attempt, attempt);
    let rows = rows(&fx, &id);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].attempt, attempt);
    assert_eq!(rows[0].launched, Some(json!(started.pane)));
    let events = started_events(&fx, &id);
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["attempt"], json!(attempt));
}

#[test]
fn a_lost_attempt_ends_with_its_partial_log_when_a_new_start_replaces_it() {
    // Given a started gate whose runner wrote part of its log and died without a result.
    let fx = Fx::new();
    let id = fx.task();
    let lost = crate::gate::gate_start(&fx.ctx, &id).unwrap();
    let lost_attempt = lost.identity.clone().unwrap().attempt;
    let paths = crate::gate::gate_paths(&fx.ctx, &id, "R1");
    let partial = b"\n$ echo collected\ncollec".to_vec();
    std::fs::write(&paths.log, &partial).unwrap();
    assert!(!paths.result.exists());
    // Its pane is listed once, then missed by two successive inventories, which releases it.
    let inventory = || -> std::collections::HashMap<String, corral_herdr::PaneInfo> {
        fx.ctx
            .herdr
            .pane_list()
            .unwrap()
            .into_iter()
            .map(|pane| (pane.pane_id.clone(), pane))
            .collect()
    };
    let reconcile = |panes: &std::collections::HashMap<String, corral_herdr::PaneInfo>, at| {
        crate::review_budget::reconcile(
            &fx.ctx,
            crate::pane_liveness::Snapshot::injected(panes),
            at,
        )
        .unwrap();
    };
    reconcile(&inventory(), lost.at);
    std::fs::remove_file(fx._tmp.path().join(lost.pane.as_deref().unwrap())).unwrap();
    let absent = inventory();
    reconcile(&absent, lost.at + jiff::SignedDuration::from_secs(1));
    reconcile(&absent, lost.at + jiff::SignedDuration::from_secs(2));

    // When a new gate replaces it.
    let next = crate::gate::gate_start(&fx.ctx, &id).unwrap();

    // Then the lost attempt's row ended as lost with exactly the log it left, never a pass,
    // its directory was reclaimed, and the marker names the new attempt.
    let row = fx
        .ctx
        .store
        .gate_attempt(&id, "R1", &lost_attempt)
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.result.as_deref(), row.failed_cmd.as_deref()),
        (Some("fail"), Some(super::SUPERSEDED_LOST))
    );
    assert!(row.ended.is_some() && row.result_doc.is_none());
    let log = fx
        .ctx
        .store
        .blob_bytes(row.log.as_ref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(log, partial);
    assert_eq!(fx.scratch(&id, &lost_attempt), ScratchState::Reclaimed);
    assert!(fx.gate_events(&id).is_empty());
    let task = fx.ctx.store.get(&id).unwrap();
    let marker = task.current_run().unwrap().gate.clone().unwrap();
    assert_eq!(marker.identity, next.identity);
    assert_eq!(started_events(&fx, &id).len(), 2);
}
