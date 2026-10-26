//! Public sync/handling paths use isolated stores and a fake engine, never the live server.
#![cfg(unix)]
use corral_core::Status;
use corral_ops::attention;
pub use corral_ops::{Ctx, resources};
use serde_json::json;
#[path = "attention_handling/action_identity.rs"]
mod action_identity;
#[path = "operating_launch/fixture.rs"]
mod operating_fixture;
#[path = "attention_handling/same_pass.rs"]
mod same_pass;

#[path = "attention_handling/fixture.rs"]
mod fixture;
use fixture::{Fx, at};

#[test]
fn sync_unavailable_branches_record_deadlines_without_inventing_pane_loss() {
    for branch in ["offline", "inventory-down"] {
        // Given an already known condition and an unavailable engine inventory.
        let fx = Fx::new();
        let task = fx.stalled(None);
        corral_ops::sync_at(&fx.ctx, at(0)).unwrap();
        std::fs::write(fx.tmp.path().join(branch), "").unwrap();
        std::fs::write(fx.tmp.path().join("notify-fail"), "").unwrap();
        // When the owner deadline crosses in each early-return sync branch.
        corral_ops::sync_at(&fx.reopen(), at(1800)).unwrap();
        corral_ops::sync_at(&fx.reopen(), at(1900)).unwrap();
        // Then its recorded levels have unknown PM liveness, and failure is never resent.
        let task = fx.ctx.store.get(&task.id).unwrap();
        assert!(
            task.attention
                .records
                .values()
                .all(|record| record.kind != "agent_lost")
        );
        assert_eq!(fx.calls(), 1);
        assert_eq!(fx.logical("owner_notify_failed").len(), 1);
        let events = fx.ctx.store.events().unwrap();
        let overdue: Vec<_> = events
            .iter()
            .filter(|event| event.kind == "attention_overdue")
            .collect();
        assert!(!overdue.is_empty());
        assert!(
            overdue
                .iter()
                .all(|event| event.detail["pm_live"].is_null())
        );
    }
}

#[test]
fn muted_push_and_disabled_desktop_do_not_hide_conditions_or_owner_deadlines() {
    // Given push and desktop notifications disabled in persisted configuration.
    let fx = Fx::new();
    let task = fx.stalled(None);
    assert!(!fx.ctx.store.config.pm.push_events);
    // When no PM consumes a wake and the owner deadline passes.
    corral_ops::sync_at(&fx.ctx, at(0)).unwrap();
    corral_ops::sync_at(&fx.ctx, at(1800)).unwrap();
    // Then canonical reads still show the condition and a bounded owner invocation.
    let board = attention::board(&fx.ctx, at(1800)).unwrap();
    let row = board.tasks.iter().find(|row| row.task == task.id).unwrap();
    assert_eq!(row.assessment.conditions[0].age_secs, Some(1800));
    assert_eq!(fx.calls(), 1);
    assert_eq!(fx.logical("attention_opened").len(), 1);
}

#[test]
fn handling_context_refusals_are_enforced_in_fresh_processes() {
    for (mode, expected) in [
        ("worker", "worker_context"),
        ("worktree", "run_worktree"),
        ("stale", "stale_item"),
    ] {
        // Given an isolated canonical key and caller context.
        let fx = Fx::new();
        let task = fx.stalled(None);
        corral_ops::sync_at(&fx.ctx, at(0)).unwrap();
        let mut key = fx
            .ctx
            .store
            .get(&task.id)
            .unwrap()
            .attention
            .records
            .keys()
            .next()
            .unwrap()
            .0
            .clone();
        if mode == "worktree" {
            fx.ctx
                .store
                .update(&task.id, |task| {
                    task.current_run_mut().unwrap().worktree = std::env::current_dir()?;
                    Ok(())
                })
                .unwrap();
        }
        if mode == "stale" {
            key.push_str("-obsolete");
        }
        let before = fx.ctx.store.get(&task.id).unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "handling_context_subprocess", "--nocapture"])
            .env("ATTENTION_TEST_ROOT", fx.ctx.store.root())
            .env("ATTENTION_TEST_KEY", &key)
            .env("ATTENTION_TEST_REASON", expected);
        for name in [
            "AI_AGENT",
            "PI_CODING_AGENT",
            "CORRAL_REVIEWER",
            "CORRAL_AGENT",
            "CORRAL_AGENT_TASK",
            "CORRAL_PM",
        ] {
            command.env_remove(name);
        }
        if mode == "worker" {
            command.env("AI_AGENT", "1");
        }
        // When the public operation runs in a fresh process, then refusal writes nothing.
        let output = command.stdin(std::process::Stdio::null()).output().unwrap();
        assert!(
            output.status.success(),
            "{mode}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(fx.ctx.store.get(&task.id).unwrap(), before);
    }
}

#[test]
fn handling_context_subprocess() {
    let Ok(root) = std::env::var("ATTENTION_TEST_ROOT") else {
        return;
    };
    let ctx = Ctx::from_store(corral_core::Store::open(std::path::Path::new(&root)).unwrap());
    let key = std::env::var("ATTENTION_TEST_KEY").unwrap();
    let expected = std::env::var("ATTENTION_TEST_REASON").unwrap();
    let before = ctx.store.list().unwrap();
    let error = attention::attend(&ctx, &key, "Looking", 1800, "refusal", at(1)).unwrap_err();
    assert!(error.to_string().contains(&expected), "{error:#}");
    assert_eq!(ctx.store.list().unwrap(), before);
}

#[test]
fn terminal_status_resolves_the_ledger_in_the_same_public_sync_pass() {
    // Given an open condition on a task that is finished before the next pass.
    let fx = Fx::new();
    let task = fx.questions(1);
    corral_ops::sync_at(&fx.ctx, at(0)).unwrap();
    fx.ctx
        .store
        .update(&task.id, |task| {
            task.status = Status::Done;
            Ok(())
        })
        .unwrap();
    // When the owning sync observes the canonical completion.
    corral_ops::sync_at(&fx.ctx, at(1)).unwrap();
    // Then a self-contained resolution is retained after the record is pruned.
    assert!(
        fx.ctx
            .store
            .get(&task.id)
            .unwrap()
            .attention
            .records
            .is_empty()
    );
    assert_eq!(fx.logical("attention_resolved").len(), 1);
    let resolved = fx
        .ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .find(|event| event.kind == "attention_resolved")
        .unwrap();
    assert_eq!(resolved.detail["responsible"], json!("pm"));
}
