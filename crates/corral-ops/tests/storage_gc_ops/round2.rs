//! Review round 2 regressions (T-334 F1, F2, F6 continued).

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use corral_core::Status;
use corral_core::backend::LifecycleOutcome;
use corral_core::work::{Counter, HotScope, snapshot};
use corral_ops::storage_gc::{Disposition, lifecycle_pass, storage_gc};

use crate::fixture::Fx;

#[test]
fn f1_a_trusted_worktree_behind_a_symlinked_parent_survives() {
    let fx = Fx::new();
    let (id, path) = fx.finished(Status::Done);
    let real = fx.app.join("external-worktrees");
    std::fs::rename(fx.app.join(".worktrees"), &real).unwrap();
    std::os::unix::fs::symlink(&real, fx.app.join(".worktrees")).unwrap();
    let report = storage_gc(&fx.ctx, true).unwrap();
    let line = report
        .entries
        .iter()
        .find(|e| e.target == path.display().to_string())
        .unwrap();
    assert_eq!(line.disposition, Disposition::Failed, "{line:?}");
    assert_eq!(
        std::fs::read(real.join(&id).join("README.md")).unwrap(),
        b"hello\n"
    );
}

/// A fake herdr whose `pane get` reports one pane in `status`.
fn fake_herdr(fx: &mut Fx, status: &str) {
    let bin = fx.root.join(format!("herdr-{status}"));
    std::fs::write(
        &bin,
        format!("#!/bin/sh\necho '{{\"result\":{{\"pane\":{{\"pane_id\":\"p1\",\"agent_status\":\"{status}\"}}}}}}'\n"),
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    fx.ctx.herdr = corral_herdr::Herdr::with_bin(bin, None);
}

/// A Done task whose run recorded pane `p1` on `socket`, with one transcript file.
fn session_with_pane(fx: &Fx, socket: &str) -> (String, std::path::PathBuf) {
    let id = fx.task(Status::Running);
    let run: corral_core::Run = serde_json::from_value(serde_json::json!({
        "id": "R1", "agent": "fake", "worktree": fx.root.join("gone"), "branch": "corral/x",
        "base": "main", "base_sha": "0".repeat(40), "started": corral_core::clock::now(),
        "herdr": {"workspace_id": "w1", "pane_id": "p1", "socket": socket},
    }))
    .unwrap();
    fx.ctx
        .store
        .update(&id, |t| {
            t.runs.push(run);
            t.status = Status::Done;
            Ok(())
        })
        .unwrap();
    let file = fx
        .ctx
        .store
        .dir()
        .join("agent/sessions")
        .join(&id)
        .join("R1/session.jsonl");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "final turn\n").unwrap();
    (id, file)
}

fn session_disposition(fx: &Fx, file: &Path) -> Disposition {
    let report = storage_gc(&fx.ctx, true).unwrap();
    let dir = file
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .display()
        .to_string();
    report
        .entries
        .iter()
        .find(|e| e.target == dir)
        .unwrap()
        .disposition
}

#[test]
fn f2_sessions_stay_until_their_pane_is_idle_on_the_recorded_server() {
    for (status, kept) in [
        ("working", true),
        ("blocked", true),
        ("unknown", true),
        ("idle", false),
    ] {
        let mut fx = Fx::new();
        fake_herdr(&mut fx, status);
        let socket = fx
            .ctx
            .herdr
            .socket_path()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let (_, file) = session_with_pane(&fx, &socket);
        let disposition = session_disposition(&fx, &file);
        if kept {
            assert_eq!(disposition, Disposition::Deferred, "{status}");
            assert_eq!(std::fs::read(&file).unwrap(), b"final turn\n", "{status}");
        } else {
            assert_eq!(disposition, Disposition::Reclaimed, "{status}");
        }
    }
}

#[test]
fn f2_an_absence_reported_by_another_server_keeps_the_sessions() {
    let mut fx = Fx::new();
    fake_herdr(&mut fx, "idle");
    let (_, file) = session_with_pane(&fx, "/tmp/recorded-owner.sock");
    assert_eq!(session_disposition(&fx, &file), Disposition::Deferred);
    assert_eq!(std::fs::read(&file).unwrap(), b"final turn\n");
}

/// The counted work of a cold first pass over `history` finished tasks whose worktrees are gone.
fn cold_recovery(history: usize) -> std::collections::BTreeMap<Counter, u64> {
    corral_core::clock::pin_thread(Some("2026-10-29T00:00:00Z".parse().unwrap()));
    let fx = Fx::new();
    for _ in 0..history {
        let (id, path) = fx.finished(Status::Done);
        corral_git_remove(&fx, &id, &path);
    }
    let before = snapshot();
    {
        let _hot = HotScope::enter();
        lifecycle_pass(&fx.ctx, corral_core::clock::now()).unwrap();
    }
    let after = snapshot();
    Counter::ALL
        .iter()
        .map(|&c| (c, after.total(c) - before.total(c)))
        .collect()
}

fn corral_git_remove(fx: &Fx, _id: &str, path: &Path) {
    crate::fixture::git(
        &fx.app,
        &["worktree", "remove", "--force", &path.to_string_lossy()],
    );
}

#[test]
fn f6_cold_recovery_work_does_not_follow_finished_history() {
    let small = cold_recovery(1);
    let large = cold_recovery(12);
    assert_eq!(small, large);
    assert_eq!(
        small[&Counter::TaskHeadDecodes],
        0,
        "no finished task is read: {small:?}"
    );
}

#[test]
fn f6_cold_recovery_still_finds_an_existing_finished_worktree() {
    let fx = Fx::new();
    let (id, path) = fx.finished(Status::Done);
    lifecycle_pass(&fx.ctx, corral_core::clock::now()).unwrap();
    let row = fx.ctx.store.lifecycle_row(&id, "R1", "worktree").unwrap();
    assert_eq!(row.map(|r| r.outcome), Some(LifecycleOutcome::Reclaimed));
    assert!(!path.exists());
}
