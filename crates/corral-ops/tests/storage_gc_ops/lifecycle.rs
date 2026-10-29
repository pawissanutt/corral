use corral_core::Status;
use corral_core::backend::{LifecycleOutcome, LifecycleRow};
use corral_ops::storage_gc::lifecycle_pass;
use jiff::{SignedDuration, Timestamp};

use crate::fixture::{Fx, Resident};

fn pass(fx: &Fx, now: Timestamp) {
    lifecycle_pass(&fx.ctx, now).unwrap();
}

fn rows(fx: &Fx, task: &str) -> Vec<LifecycleRow> {
    fx.ctx.store.lifecycle_rows(task).unwrap()
}

fn only(fx: &Fx, task: &str) -> LifecycleRow {
    let rows = rows(fx, task);
    assert_eq!(rows.len(), 1, "{rows:?}");
    rows.into_iter().next().unwrap()
}

fn later(now: Timestamp, secs: i64) -> Timestamp {
    now.checked_add(SignedDuration::from_secs(secs)).unwrap()
}

fn seq(fx: &Fx) -> u64 {
    fx.ctx.store.view().unwrap().seq().0
}

fn reclaimed_events(fx: &Fx, task: &str) -> usize {
    fx.events("storage_reclaimed")
        .into_iter()
        .filter(|event| event.task.as_deref() == Some(task))
        .count()
}

#[test]
fn done_twice_and_a_policy_change_keep_one_row_per_generation() {
    let mut fx = Fx::new();
    let now = corral_core::clock::now();
    pass(&fx, now);
    let (id, path) = fx.finished(Status::Done);
    pass(&fx, now);
    let first = only(&fx, &id);
    assert_eq!(
        (first.generation.as_str(), first.target.as_str()),
        ("R1", "worktree")
    );
    assert_eq!(first.outcome, LifecycleOutcome::Reclaimed);
    assert!(first.bytes.unwrap() >= 4096);
    assert!(!path.exists());
    assert!(fx.branch_exists(&format!("corral/{id}-R1")), "branch kept");

    // The second sync does no filesystem work and writes nothing.
    let (git, before) = (corral_git::work::invocations(), seq(&fx));
    pass(&fx, now);
    assert_eq!(corral_git::work::invocations(), git);
    assert_eq!(seq(&fx), before);
    assert_eq!(only(&fx, &id), first);

    // Reopened with a new run: nothing while open; the new generation once Done again.
    let again = fx.worktree(&id, "R2");
    fx.start(&id, "R2", &again, Status::Running);
    pass(&fx, now);
    assert_eq!(rows(&fx, &id).len(), 1);
    fx.set_status(&id, Status::Done);
    pass(&fx, now);
    let rows_now = rows(&fx, &id);
    assert_eq!(rows_now.len(), 2, "{rows_now:?}");
    let second = rows_now.iter().find(|r| r.generation == "R2").unwrap();
    assert_eq!(second.outcome, LifecycleOutcome::Reclaimed);
    assert!(!again.exists());
    assert_eq!(reclaimed_events(&fx, &id), 2);

    // A policy change re-evaluates exactly the rows recorded under the old digest.
    let old = second.policy_digest.clone();
    let recorded: Vec<(String, Option<u64>)> = rows_now
        .iter()
        .map(|r| (r.generation.clone(), r.bytes))
        .collect();
    fx.ctx.store.config.review.remove_worktree = Some(false);
    let (kept, kept_path) = fx.finished(Status::Done);
    let stale_before = fx.ctx.store.lifecycle_policy_other_than("", 100).unwrap();
    assert_eq!(stale_before.len(), 2);
    pass(&fx, later(now, 5));
    let kept_row = only(&fx, &kept);
    assert_eq!(kept_row.outcome, LifecycleOutcome::Retained);
    assert!(kept_row.reason.as_deref().unwrap().starts_with("kept"));
    assert!(kept_path.exists());
    let new = kept_row.policy_digest.clone();
    assert_ne!(new, old);
    assert!(
        fx.ctx
            .store
            .lifecycle_policy_other_than(&new, 100)
            .unwrap()
            .is_empty(),
        "every old-digest row was re-evaluated"
    );
    let after = rows(&fx, &id);
    for row in &after {
        assert_eq!(row.policy_digest, new);
        assert_eq!(row.outcome, LifecycleOutcome::Reclaimed);
    }
    let kept_bytes: Vec<(String, Option<u64>)> = after
        .iter()
        .map(|r| (r.generation.clone(), r.bytes))
        .collect();
    assert_eq!(
        kept_bytes, recorded,
        "re-evaluation keeps the recorded bytes"
    );
    assert_eq!(reclaimed_events(&fx, &id), 2, "nothing removed twice");
    let git = corral_git::work::invocations();
    pass(&fx, later(now, 10));
    assert_eq!(
        only(&fx, &kept),
        kept_row,
        "rows under the current digest are not revisited"
    );
    assert_eq!(corral_git::work::invocations(), git);
}

#[test]
fn dirty_then_clean_worktree_is_rechecked_when_due_and_reclaimed_once() {
    let fx = Fx::new();
    let now = corral_core::clock::now();
    pass(&fx, now);
    let (id, path) = fx.finished(Status::Done);
    std::fs::write(path.join("notes.txt"), "unsaved\n").unwrap();
    pass(&fx, now);
    let deferred = only(&fx, &id);
    assert_eq!(deferred.outcome, LifecycleOutcome::Deferred);
    assert!(deferred.reason.as_deref().unwrap().starts_with("dirty"));
    assert_eq!(deferred.next_check_at, Some(later(now, 60)));

    std::fs::remove_file(path.join("notes.txt")).unwrap();
    pass(&fx, later(now, 30));
    assert_eq!(only(&fx, &id), deferred, "not due yet");
    pass(&fx, later(now, 60));
    assert_eq!(only(&fx, &id).outcome, LifecycleOutcome::Reclaimed);
    assert!(!path.exists());
    pass(&fx, later(now, 7200));
    assert_eq!(reclaimed_events(&fx, &id), 1);
}

#[test]
fn process_resident_worktree_is_deferred_with_backoff_then_reclaimed() {
    let fx = Fx::new();
    let now = corral_core::clock::now();
    pass(&fx, now);
    let (id, path) = fx.finished(Status::Done);
    let resident = Resident::start(&path, None);
    pass(&fx, now);
    let first = only(&fx, &id);
    assert_eq!(first.outcome, LifecycleOutcome::Deferred);
    assert!(
        first.reason.as_deref().unwrap().contains("process"),
        "{first:?}"
    );
    pass(&fx, later(now, 60));
    let second = only(&fx, &id);
    assert_eq!(second.outcome, LifecycleOutcome::Deferred);
    assert_eq!(
        second.next_check_at,
        Some(later(now, 60 + 120)),
        "the delay doubles"
    );
    assert!(path.exists());
    drop(resident);
    pass(&fx, later(now, 180));
    assert_eq!(only(&fx, &id).outcome, LifecycleOutcome::Reclaimed);
    assert!(!path.exists());
    assert_eq!(reclaimed_events(&fx, &id), 1);
}

#[test]
fn a_worktree_without_git_identity_goes_only_when_it_holds_build_output() {
    let fx = Fx::new();
    let now = corral_core::clock::now();
    pass(&fx, now);
    let untrusted = |extra: Option<&str>| {
        let id = fx.task(Status::Running);
        let dir = fx.app.join(".worktrees").join(&id);
        let target = dir.join("target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(
            target.join("CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55\n",
        )
        .unwrap();
        std::fs::write(target.join("app"), vec![1u8; 1024]).unwrap();
        if let Some(name) = extra {
            std::fs::write(dir.join(name), "fn main() {}\n").unwrap();
        }
        fx.start(&id, "R1", &dir, Status::Done);
        (id, dir)
    };
    let (build_id, build_dir) = untrusted(None);
    let (source_id, source_dir) = untrusted(Some("main.rs"));
    pass(&fx, now);
    assert_eq!(only(&fx, &build_id).outcome, LifecycleOutcome::Reclaimed);
    assert!(!build_dir.exists());
    let kept = only(&fx, &source_id);
    assert_eq!(kept.outcome, LifecycleOutcome::Retained);
    assert!(kept.reason.as_deref().unwrap().starts_with("unknown"));
    assert!(source_dir.join("main.rs").exists());
}
