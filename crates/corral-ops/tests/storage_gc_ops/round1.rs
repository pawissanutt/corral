//! Review round 1 regressions (T-334 F1, F2, F4, F5, F7).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use corral_core::backend::LifecycleOutcome;
use corral_core::{ArtifactKind, Status};
use corral_ops::storage_gc::{Disposition, GcReport, lifecycle_pass, storage_gc};
use jiff::SignedDuration;

use crate::fixture::{Fx, Resident, git};

const TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n";

fn entry<'a>(report: &'a GcReport, path: &Path) -> &'a corral_ops::storage_gc::GcEntry {
    let target = path.display().to_string();
    report
        .entries
        .iter()
        .find(|e| e.target == target)
        .unwrap_or_else(|| panic!("no entry for {target}: {:#?}", report.entries))
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for item in std::fs::read_dir(from).unwrap() {
        let item = item.unwrap();
        let dest = to.join(item.file_name());
        if item.file_type().unwrap().is_dir() {
            copy_dir(&item.path(), &dest);
        } else {
            std::fs::copy(item.path(), dest).unwrap();
        }
    }
}

#[test]
fn f1_build_output_behind_a_symlinked_state_area_is_never_deleted() {
    let fx = Fx::new();
    let state = fx.ctx.store.dir().join("state");
    let outside = fx.root.join("outside-state");
    copy_dir(&state, &outside);
    let build = outside.join("build");
    std::fs::create_dir_all(&build).unwrap();
    std::fs::write(build.join("CACHEDIR.TAG"), TAG).unwrap();
    std::fs::write(build.join("private.bin"), b"external bytes").unwrap();
    std::fs::remove_dir_all(&state).unwrap();
    std::os::unix::fs::symlink(&outside, &state).unwrap();

    let report = storage_gc(&fx.ctx, true).unwrap();
    assert_eq!(
        std::fs::read(build.join("private.bin")).unwrap(),
        b"external bytes"
    );
    assert!(
        report
            .entries
            .iter()
            .all(|e| e.class != "legacy-build-output"),
        "{:#?}",
        report.entries
    );
}

#[test]
fn f2_a_resident_build_only_worktree_is_deferred_until_idle() {
    let fx = Fx::new();
    let now = corral_core::clock::now();
    let id = fx.task(Status::Running);
    let dir = fx.app.join(".worktrees").join(&id);
    let target = dir.join("target");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("CACHEDIR.TAG"), TAG).unwrap();
    std::fs::write(target.join("app"), vec![1u8; 512]).unwrap();
    fx.start(&id, "R1", &dir, Status::Done);
    let resident = Resident::start(&target, None);
    lifecycle_pass(&fx.ctx, now).unwrap();
    let row = fx
        .ctx
        .store
        .lifecycle_row(&id, "R1", "worktree")
        .unwrap()
        .unwrap();
    assert_eq!(row.outcome, LifecycleOutcome::Deferred, "{row:?}");
    assert!(
        row.reason.as_deref().unwrap().contains("process"),
        "{row:?}"
    );
    assert!(dir.exists());
    drop(resident);
    lifecycle_pass(
        &fx.ctx,
        now.checked_add(SignedDuration::from_secs(60)).unwrap(),
    )
    .unwrap();
    let row = fx
        .ctx
        .store
        .lifecycle_row(&id, "R1", "worktree")
        .unwrap()
        .unwrap();
    assert_eq!(row.outcome, LifecycleOutcome::Reclaimed);
    assert!(!dir.exists());
}

#[test]
fn f2_sessions_of_a_task_with_an_unconfirmed_pane_are_kept() {
    let fx = Fx::new();
    let id = fx.task(Status::Running);
    let run: corral_core::Run = serde_json::from_value(serde_json::json!({
        "id": "R1", "agent": "fake", "worktree": fx.root.join("gone"), "branch": "corral/x",
        "base": "main", "base_sha": "0".repeat(40), "started": corral_core::clock::now(),
        "herdr": {"workspace_id": "w1", "pane_id": "p-still-open"},
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
    let sessions = fx.ctx.store.dir().join("agent/sessions").join(&id);
    std::fs::create_dir_all(sessions.join("R1")).unwrap();
    std::fs::write(sessions.join("R1/session.jsonl"), "{}\n").unwrap();

    let report = storage_gc(&fx.ctx, true).unwrap();
    let line = entry(&report, &sessions);
    assert_eq!(line.disposition, Disposition::Deferred, "{line:?}");
    assert!(
        line.reason
            .as_deref()
            .unwrap()
            .contains("pane p-still-open"),
        "{line:?}"
    );
    assert!(sessions.join("R1/session.jsonl").exists());
}

#[test]
fn f4_a_dry_run_does_not_refresh_a_worktree_index() {
    let fx = Fx::new();
    let (_, path) = fx.finished(Status::Done);
    let readme = std::fs::File::options()
        .write(true)
        .open(path.join("README.md"))
        .unwrap();
    readme
        .set_modified(SystemTime::now() - Duration::from_secs(3600))
        .unwrap();
    let index = PathBuf::from(
        git(
            &path,
            &["rev-parse", "--path-format=absolute", "--git-path", "index"],
        )
        .trim(),
    );
    let before = std::fs::read(&index).unwrap();
    let report = storage_gc(&fx.reopen(), false).unwrap();
    let line = entry(&report, &path);
    assert_eq!(line.disposition, Disposition::Reclaimable, "{line:?}");
    assert_eq!(
        std::fs::read(&index).unwrap(),
        before,
        "the index was not rewritten"
    );
}

#[test]
fn f5_work_that_predates_the_first_pass_is_discovered() {
    let fx = Fx::new();
    let (id, path) = fx.finished(Status::Done);
    lifecycle_pass(&fx.ctx, corral_core::clock::now()).unwrap();
    let row = fx.ctx.store.lifecycle_row(&id, "R1", "worktree").unwrap();
    assert_eq!(row.map(|r| r.outcome), Some(LifecycleOutcome::Reclaimed));
    assert!(!path.exists());
}

#[test]
fn f5_a_headless_sync_runs_the_lifecycle_pass() {
    let fx = Fx::new();
    let (id, path) = fx.finished(Status::Dropped);
    corral_ops::sync_at(&fx.ctx, corral_core::clock::now()).unwrap();
    let row = fx.ctx.store.lifecycle_row(&id, "R1", "worktree").unwrap();
    assert_eq!(row.map(|r| r.outcome), Some(LifecycleOutcome::Reclaimed));
    assert!(!path.exists());
}

/// A Done task with a main and a nested review transcript in its sessions directory.
fn terminal_sessions(fx: &Fx) -> (String, PathBuf, PathBuf) {
    let (id, _) = fx.finished(Status::Running);
    let sessions = fx.ctx.store.dir().join("agent/sessions").join(&id);
    std::fs::create_dir_all(sessions.join("R1/R1-review1")).unwrap();
    let main = sessions.join("R1/session.jsonl");
    std::fs::write(&main, "old\n").unwrap();
    std::fs::write(sessions.join("R1/R1-review1/review.jsonl"), "review\n").unwrap();
    (id, sessions, main)
}

fn live_shas(fx: &Fx, id: &str) -> Vec<String> {
    fx.ctx
        .store
        .artifact_rows(id, None)
        .unwrap()
        .into_iter()
        .filter(|a| a.purged_at.is_none())
        .map(|a| a.sha256)
        .collect()
}

#[test]
fn f7_a_transcript_changed_after_an_earlier_promotion_is_promoted_again() {
    let fx = Fx::new();
    let (id, sessions, main) = terminal_sessions(&fx);
    corral_ops::add_artifact(&fx.ctx, &id, &main, Some(ArtifactKind::Log), None).unwrap();
    std::fs::write(&main, "old\nnew final turn\n").unwrap();
    fx.set_status(&id, Status::Done);

    let report = storage_gc(&fx.ctx, true).unwrap();
    assert_eq!(
        entry(&report, &sessions).disposition,
        Disposition::Reclaimed
    );
    assert!(!sessions.exists());
    let shas = live_shas(&fx, &id);
    for bytes in [&b"old\nnew final turn\n"[..], b"review\n"] {
        let sha = corral_core::agent::paths::sha256_hex(bytes);
        assert!(shas.contains(&sha), "{sha} live in {shas:?}");
    }
}

#[test]
fn f7_a_purged_snapshot_of_the_same_bytes_is_revived_before_removal() {
    let fx = Fx::new();
    let (id, sessions, main) = terminal_sessions(&fx);
    corral_ops::add_artifact(&fx.ctx, &id, &main, Some(ArtifactKind::Log), None).unwrap();
    fx.ctx
        .store
        .update(&id, |t| {
            for artifact in &mut t.artifacts {
                artifact.purged_at = Some(corral_core::clock::now());
            }
            t.status = Status::Done;
            Ok(())
        })
        .unwrap();
    assert!(live_shas(&fx, &id).is_empty());

    storage_gc(&fx.ctx, true).unwrap();
    assert!(!sessions.exists());
    let sha = corral_core::agent::paths::sha256_hex(b"old\n");
    assert!(live_shas(&fx, &id).contains(&sha));
}
