//! Review attempt scratch (T-253 review-scratch): the prompt, packet and notes of a review live
//! in a registered directory under the scratch root, `--evidence` files are promoted with the
//! verdict or refused with no verdict, and the sync-time pass reclaims closed directories only
//! while nothing uses them.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use corral_core::backend::{ScratchKind, ScratchRow, ScratchState};
use corral_core::scratch::AttemptScratch;
use corral_core::{Status, Store, Verdict};
use corral_herdr::Herdr;
use corral_ops::Ctx;
use jiff::{SignedDuration, Timestamp};

use super::task_in_review;

/// Every path under `.corral` except the database, leases and blob store, which a launch
/// legitimately writes.
fn corral_tree(dir: &Path) -> BTreeSet<PathBuf> {
    let mut out = BTreeSet::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            let rel = path.strip_prefix(dir).unwrap().to_path_buf();
            if ["db", "lock", "blobs"]
                .iter()
                .any(|skip| rel.starts_with(skip))
            {
                continue;
            }
            if path.is_dir() {
                pending.push(path.clone());
            }
            out.insert(rel);
        }
    }
    out
}

fn events(ctx: &Ctx, kind: &str) -> Vec<serde_json::Value> {
    ctx.store
        .events()
        .unwrap()
        .into_iter()
        .filter(|event| event.kind == kind)
        .map(|event| event.detail)
        .collect()
}

fn row(ctx: &Ctx, id: &str) -> ScratchRow {
    ctx.store
        .scratch_rows_in(&[
            ScratchState::Open,
            ScratchState::Closed,
            ScratchState::Deferred,
            ScratchState::Reclaiming,
            ScratchState::Reclaimed,
            ScratchState::Failed,
        ])
        .unwrap()
        .into_iter()
        .find(|row| row.id == id)
        .unwrap_or_else(|| panic!("scratch {id} is not registered"))
}

#[test]
fn a_review_launch_keeps_its_prompt_and_notes_in_a_registered_scratch_dir() {
    let (_tmp, ctx, id, _worktree) = task_in_review();
    let before = corral_tree(ctx.store.dir());

    corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();

    let notes = corral_ops::review_notes_path(&ctx, &id, "R1").unwrap();
    let dir = notes.parent().unwrap().to_path_buf();
    assert!(
        dir.starts_with(ctx.store.scratch_path("reviews").unwrap()),
        "{}",
        dir.display()
    );
    let registered = ctx
        .store
        .scratch_rows_in(&[ScratchState::Open])
        .unwrap()
        .into_iter()
        .find(|row| row.kind == ScratchKind::ImplementationReview)
        .expect("the review attempt is registered");
    assert_eq!(ctx.store.scratch_path(&registered.path).unwrap(), dir);
    let prompt = std::fs::read_to_string(dir.join("review.md")).unwrap();
    assert!(prompt.contains(&dir.display().to_string()), "{prompt}");
    assert!(prompt.contains("at most 64MiB in total"), "{prompt}");
    let created: Vec<_> = corral_tree(ctx.store.dir())
        .difference(&before)
        .cloned()
        .collect();
    assert!(created.is_empty(), "created under .corral: {created:?}");
}

#[test]
fn verdict_evidence_from_the_attempt_dir_and_worktree_is_promoted_with_the_verdict() {
    let (_tmp, ctx, id, worktree) = task_in_review();
    corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();
    let notes = corral_ops::review_notes_path(&ctx, &id, "R1").unwrap();
    let dir = notes.parent().unwrap().to_path_buf();
    let shot = dir.join("a.png");
    std::fs::write(&shot, b"\x89PNG fixture").unwrap();
    let committed = worktree.join("spin.txt");

    let record = corral_ops::verdict_with_evidence(
        &ctx,
        &id,
        Verdict::Approve,
        "spins clockwise",
        None,
        &[shot, committed],
    )
    .unwrap();

    assert_eq!(record.verdict, Some(Verdict::Approve));
    let task = ctx.store.get(&id).unwrap();
    let titles: BTreeSet<_> = task
        .artifacts
        .iter()
        .filter(|a| a.run == "R1" && a.title.starts_with("Evidence: "))
        .map(|a| a.title.clone())
        .collect();
    assert_eq!(
        titles,
        BTreeSet::from([
            "Evidence: a.png".to_string(),
            "Evidence: spin.txt".to_string()
        ])
    );
    let review = task.current_run().unwrap().review_runs.last().unwrap();
    assert_eq!(
        row(&ctx, &format!("review-{id}-{}", review.id)).state,
        ScratchState::Closed,
        "the verdict closes the attempt's directory"
    );
}

#[test]
fn refused_evidence_records_no_verdict() {
    let (tmp, ctx, id, worktree) = task_in_review();
    corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();
    let dir = corral_ops::review_notes_path(&ctx, &id, "R1")
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let outside = tmp.path().join("outside.txt");
    std::fs::write(&outside, "not reviewed").unwrap();
    let link = dir.join("link.txt");
    std::os::unix::fs::symlink(worktree.join("spin.txt"), &link).unwrap();
    std::fs::write(dir.join("a.txt"), "abc").unwrap();
    std::fs::write(dir.join("b.txt"), "def").unwrap();
    let mut small = ctx.clone();
    small.store.config.storage.review_evidence_max = "5B".into();
    small.store.save_config().unwrap();

    for (case, ctx, files) in [
        ("outside", &ctx, vec![outside]),
        ("symlink", &ctx, vec![link]),
        (
            "over budget",
            &small,
            vec![dir.join("a.txt"), dir.join("b.txt")],
        ),
    ] {
        let error =
            corral_ops::verdict_with_evidence(ctx, &id, Verdict::Approve, "notes", None, &files)
                .unwrap_err();
        assert!(!format!("{error:#}").is_empty(), "{case}");
        let task = ctx.store.get(&id).unwrap();
        assert_eq!(task.status, Status::Review, "{case}");
        assert!(
            task.current_run()
                .unwrap()
                .review
                .as_ref()
                .is_some_and(|review| review.verdict.is_none()),
            "{case}: no verdict is recorded"
        );
        assert!(events(ctx, "verdict").is_empty(), "{case}");
        assert!(
            !task
                .artifacts
                .iter()
                .any(|a| a.title.starts_with("Evidence: ")),
            "{case}"
        );
    }
}

/// A store whose herdr is never called: the attempts below have no recorded pane.
fn bare() -> (tempfile::TempDir, Ctx, String) {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store
        .create("Scratch fixture", None, Status::Review, "")
        .unwrap();
    let ctx = Ctx {
        resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
        store,
        herdr: Herdr::with_bin(tmp.path().join("no-herdr"), None),
    };
    (tmp, ctx, task.id)
}

fn closed_attempt(ctx: &Ctx, task: &str, name: &str) -> (String, PathBuf) {
    let id = format!("review-{task}-{name}");
    let rel = format!("reviews/{task}-{name}");
    let dir = ctx
        .store
        .attempt_scratch(&AttemptScratch {
            kind: ScratchKind::ImplementationReview,
            id: &id,
            task,
            subject: Some("R1"),
            attempt: name,
            rel: &rel,
        })
        .unwrap();
    std::fs::write(dir.join("notes.md"), "notes").unwrap();
    assert!(ctx.store.close_open_scratch(&id).unwrap());
    (id, dir)
}

fn reclaimed_events(ctx: &Ctx, id: &str) -> usize {
    events(ctx, "storage_reclaimed")
        .iter()
        .filter(|detail| detail["scratch"] == id)
        .count()
}

/// Defers `id` (the pass runs two hours early so its recheck is already due afterwards),
/// checks the reason and that nothing was deleted.
fn assert_deferred(ctx: &Ctx, id: &str, dir: &Path, reason: &str) {
    let early = Timestamp::now() - SignedDuration::from_hours(2);
    corral_ops::reclaim_scratch_pass(ctx, early).unwrap();
    let deferred = row(ctx, id);
    assert_eq!(deferred.state, ScratchState::Deferred);
    assert!(
        deferred
            .reason
            .as_deref()
            .is_some_and(|r| r.contains(reason)),
        "{:?}",
        deferred.reason
    );
    assert!(deferred.next_check_at.is_some_and(|at| at > early));
    assert!(dir.join("notes.md").is_file(), "never deleted while in use");
    assert_eq!(reclaimed_events(ctx, id), 0);
}

fn assert_reclaimed_once(ctx: &Ctx, id: &str, dir: &Path) {
    corral_ops::reclaim_scratch_pass(ctx, Timestamp::now()).unwrap();
    corral_ops::reclaim_scratch_pass(ctx, Timestamp::now()).unwrap();
    assert_eq!(row(ctx, id).state, ScratchState::Reclaimed);
    assert!(!dir.exists());
    assert_eq!(reclaimed_events(ctx, id), 1);
}

fn stop(mut child: Child) {
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn an_unused_closed_attempt_is_reclaimed_once() {
    let (_tmp, ctx, task) = bare();
    let (id, dir) = closed_attempt(&ctx, &task, "R1-review1");
    assert_reclaimed_once(&ctx, &id, &dir);
}

#[test]
fn a_process_whose_cwd_is_under_the_attempt_defers_until_it_exits() {
    // The attempt has no live pane: a surviving process alone keeps the directory.
    let (_tmp, ctx, task) = bare();
    let (id, dir) = closed_attempt(&ctx, &task, "R1-review1");
    let child = Command::new("sleep")
        .arg("600")
        .current_dir(&dir)
        .spawn()
        .unwrap();
    assert_deferred(&ctx, &id, &dir, &format!("process {}", child.id()));
    stop(child);
    assert_reclaimed_once(&ctx, &id, &dir);
}

#[test]
fn a_process_with_an_open_file_under_the_attempt_defers_until_it_exits() {
    let (_tmp, ctx, task) = bare();
    let (id, dir) = closed_attempt(&ctx, &task, "R1-review1");
    let file = dir.join("notes.md");
    let child = Command::new("sh")
        .arg("-c")
        .arg("exec 3<\"$1\"; exec sleep 600")
        .arg("sh")
        .arg(&file)
        .current_dir("/")
        .spawn()
        .unwrap();
    // The descriptor is open once the shell has exec'd into sleep holding it.
    let fd = PathBuf::from(format!("/proc/{}/fd/3", child.id()));
    let target = file.canonicalize().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::fs::read_link(&fd).ok() != Some(target.clone()) {
        assert!(
            std::time::Instant::now() < deadline,
            "the child never opened {}",
            file.display()
        );
        std::thread::yield_now();
    }
    assert_deferred(&ctx, &id, &dir, &format!("process {}", child.id()));
    stop(child);
    assert_reclaimed_once(&ctx, &id, &dir);
}

#[test]
fn a_held_cargo_lock_under_the_attempt_defers_until_released() {
    let (_tmp, ctx, task) = bare();
    let (id, dir) = closed_attempt(&ctx, &task, "R1-review1");
    std::fs::create_dir_all(dir.join("target/debug")).unwrap();
    let lock = std::fs::File::create(dir.join("target/debug/.cargo-lock")).unwrap();
    lock.lock().unwrap();
    assert_deferred(&ctx, &id, &dir, ".cargo-lock is held");
    drop(lock);
    assert_reclaimed_once(&ctx, &id, &dir);
}

#[test]
fn an_open_attempt_over_the_warning_size_warns_once_across_restarts() {
    let (tmp, mut ctx, id, _worktree) = task_in_review();
    ctx.store.config.storage.review_scratch_warn = "1KiB".into();
    ctx.store.save_config().unwrap();
    let log = tmp.path().join("prompts.log");
    let wrapper = tmp.path().join("herdr-logged");
    std::fs::write(
        &wrapper,
        format!(
            "#!/usr/bin/env bash\nif [ \"$1\" = --session ]; then a=(\"$3\" \"$4\" \"$5\"); else a=(\"$1\" \"$2\" \"$3\"); fi\n\
             if [ \"${{a[0]}} ${{a[1]}}\" = 'agent prompt' ]; then printf '%s\\n' \"$*\" >> '{}'; fi\n\
             exec '{}' \"$@\"\n",
            log.display(),
            tmp.path().join("herdr").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(
        &wrapper,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    ctx.herdr = Herdr::with_bin(wrapper, None);
    corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();
    let dir = corral_ops::review_notes_path(&ctx, &id, "R1")
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    std::fs::write(dir.join("big.log"), vec![b'x'; 4096]).unwrap();

    corral_ops::reclaim_scratch_pass(&ctx, Timestamp::now()).unwrap();
    let mut restarted = ctx.clone();
    restarted.store = Store::open(tmp.path()).unwrap();
    corral_ops::reclaim_scratch_pass(&restarted, Timestamp::now()).unwrap();

    assert_eq!(events(&restarted, "storage_scratch_warning").len(), 1);
    let steering = std::fs::read_to_string(&log).unwrap_or_default();
    assert_eq!(
        steering
            .lines()
            .filter(|line| line.contains("review_scratch_warn"))
            .count(),
        1,
        "{steering}"
    );
    assert!(dir.join("big.log").is_file(), "a warning deletes nothing");
}
