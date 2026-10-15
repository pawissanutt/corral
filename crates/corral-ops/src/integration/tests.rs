use super::*;
use corral_core::{AutoStage, AutopilotState, Store};
use corral_herdr::Herdr;
use serde_json::json;
use std::path::Path;
use std::process::Stdio;

use crate::test_support as support;

fn git(repo: &Path, args: &[&str]) -> String {
    let out = support::command("git", repo)
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn fixture() -> (tempfile::TempDir, Ctx, Task, Run, RepairKey) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("app");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.name", "corral test"]);
    git(&repo, &["config", "user.email", "corral@test.invalid"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    std::fs::write(repo.join("file.rs"), "fn original() {}\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "initial"]);
    let sha = git(&repo, &["rev-parse", "HEAD"]);
    let ctx = Ctx {
        store: Store::init(tmp.path()).unwrap(),
        herdr: Herdr::with_bin("/nonexistent/herdr".into(), None),
    };
    let task = ctx
        .store
        .create("Repair guard", Some("app"), Status::Review, "")
        .unwrap();
    let run: Run = serde_json::from_value(json!({
        "id":"R1", "agent":"fake", "worktree":repo, "branch":"main",
        "base":"main", "base_sha":sha, "started":"2026-10-15T09:00:00Z",
        "outcome":"done"
    }))
    .unwrap();
    let task = ctx
        .store
        .update(&task.id, |task| {
            task.runs.push(run.clone());
            Ok(())
        })
        .unwrap();
    let key = RepairKey::new(&task, &run, "merge_conflict").unwrap();
    (tmp, ctx, task, run, key)
}

fn state() -> AutopilotState {
    AutopilotState {
        stage: AutoStage::Changes,
        reason: Some("fixture".into()),
        sha: None,
        at: "2026-10-15T09:00:00Z".parse().unwrap(),
    }
}

#[test]
fn stale_packet_cannot_deliver_retry_or_attach_to_a_newer_run() {
    // Given a computed packet whose original run was superseded before any side effect.
    let (_tmp, ctx, task, run, key) = fixture();
    let policy = ctx
        .store
        .config
        .review_policy(task.project.as_deref(), None);
    let packet =
        IntegrationPacket::build(&ctx, &task, &run, &policy, key.clone(), vec![], None).unwrap();
    let current = ctx
        .store
        .update(&task.id, |task| {
            task.current_run_mut().unwrap().id = "R2".into();
            task.current_run_mut().unwrap().summary = Some("newer summary".into());
            Ok(())
        })
        .unwrap();

    // When stale send-back, retry and artifact operations each reach their write boundary.
    let delivery =
        crate::dispatch::auto_request_changes(&ctx, &task.id, "repair", 1, state(), &key);
    let retry = crate::dispatch::auto_retry(&ctx, &task.id, 1, state(), &key);
    let attach = packet.attach(&ctx);

    // Then they all reject the obsolete run, and the newer task remains byte-for-byte equivalent.
    assert!(delivery.unwrap_err().is::<StalePacket>());
    assert!(retry.unwrap_err().is::<StalePacket>());
    assert!(attach.is_err());
    assert_eq!(ctx.store.get(&task.id).unwrap(), current);
}

#[test]
fn main_movement_invalidates_packet_before_delivery_without_consuming_a_round() {
    // Given a packet computed before a new main commit.
    let (_tmp, ctx, task, run, key) = fixture();
    std::fs::write(run.worktree.join("file.rs"), "fn landed() {}\n").unwrap();
    git(&run.worktree, &["add", "-A"]);
    git(&run.worktree, &["commit", "-q", "-m", "main moved"]);

    // When delivery checks the original head/base under the task lock.
    let result = crate::dispatch::auto_request_changes(&ctx, &task.id, "repair", 1, state(), &key);

    // Then no request or stale task mutation reaches the unavailable transport.
    assert!(result.unwrap_err().is::<StalePacket>());
    assert_eq!(ctx.store.get(&task.id).unwrap(), task);
}

#[test]
fn repair_preflight_inside_capacity_claim_refuses_stale_inputs_without_holding_a_slot() {
    use std::os::unix::fs::PermissionsExt;

    let (tmp, mut ctx, task, _run, key) = fixture();
    ctx.store.config.limits.running = 1;
    ctx.store.save_config().unwrap();
    let task = ctx
        .store
        .update(&task.id, |task| {
            task.current_run_mut().unwrap().herdr = Some(corral_core::HerdrRef {
                workspace_id: "w1".into(),
                pane_id: "w1:p1".into(),
            });
            Ok(())
        })
        .unwrap();
    let bin = tmp.path().join("herdr");
    std::fs::write(
        &bin,
        r#"#!/usr/bin/env bash
export CORRAL_ROOT="$(cd "$(dirname "$0")" && pwd)"
unset CORRAL_GATE_ATTEMPT CORRAL_CANDIDATE_HEAD CORRAL_INTEGRATION_BASE HERDR_PANE_ID HERDR_WORKSPACE_ID HERDR_TAB_ID
if [ "$1 $2" != "pane get" ]; then exit 99; fi
git -C "$(dirname "$0")/app" commit --allow-empty -qm "moved during pane lookup" || exit 1
printf '{"result":{"pane":{"pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1"}}}\n'
"#,
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    ctx.herdr = Herdr::with_bin(bin, None);

    // The initial check passes; pane lookup moves HEAD before the atomic claim check.
    let result = crate::dispatch::auto_request_changes(&ctx, &task.id, "repair", 1, state(), &key);

    assert!(result.unwrap_err().is::<StalePacket>());
    assert_eq!(ctx.store.get(&task.id).unwrap(), task);
    assert_eq!(crate::slot_usage(&ctx).unwrap().total(), 0);
    assert!(
        !ctx.store
            .events()
            .unwrap()
            .iter()
            .any(|e| e.kind == "changes_requested")
    );
}
