use std::fs::{self, File};

use crate::{Outcome, PipelineMode, SessionStatus, Status, as_refusal};

use super::{Fixture, git};

#[test]
fn takeover_checks_live_placements_even_for_ended_workers() {
    let fixture = Fixture::new(&[]);
    let task = fixture.backlog("Race a worker");
    let run = fixture.dispatch(&task);
    let engine = &fixture.engine;
    assert!(engine.takeover_probe(&task).is_err());

    // Ending the run in the board does not stop a physical writer.
    let mut ended = run.clone();
    ended.ended = Some(ended.started);
    ended.outcome = Some(Outcome::Stopped);
    engine.store.update_run(&ended).unwrap();
    let live = engine.takeover_probe(&task).unwrap_err();
    assert!(as_refusal(&live).is_some());
    assert!(fixture.runner.stops().is_empty());
    let lock = engine.takeover_writer_lock(&task, &run.worktree).unwrap();
    assert!(lock.lock.is_some());

    fixture
        .runner
        .set_status(&run.session.unwrap(), SessionStatus::Gone);
    assert_eq!(engine.takeover_probe(&task).unwrap().run, run.id);
}

#[test]
fn exclusive_takeover_blocks_retry_until_descriptor_drop() {
    let mut fixture = Fixture::new(&[]);
    fixture.engine.pipeline = PipelineMode::Off;
    let task = fixture.backlog("Reserve checkout");
    let run = fixture.dispatch(&task);
    let engine = &fixture.engine;
    engine.stop(&task).unwrap();
    let preview = engine.takeover_probe(&task).unwrap();
    let writer = File::open(&preview.lock).unwrap();
    writer.try_lock_shared().unwrap();
    assert!(engine.takeover_probe(&task).is_err());
    assert!(engine.takeover_verify(&task).is_err());
    writer.unlock().unwrap();
    drop(writer);

    let host = File::open(&preview.lock).unwrap();
    host.lock().unwrap();
    assert_eq!(engine.takeover_verify(&task).unwrap(), preview);
    assert!(engine.takeover_probe(&task).is_err());
    assert!(as_refusal(&engine.retry(&task, None, true, None).unwrap_err()).is_some());
    assert!(as_refusal(&engine.done(&task, "unapproved submission").unwrap_err()).is_some());
    assert!(as_refusal(&engine.drop_task(&task).unwrap_err()).is_some());
    assert_eq!(fixture.runner.starts().len(), 1);
    assert_eq!(
        engine.store.require_task(&task).unwrap().status,
        Status::Blocked
    );
    drop(host);

    // An unrelated test's in-flight fork may briefly retain a copy before exec.
    // Synchronize on release of exclusivity without assuming close timing.
    let released = File::open(&preview.lock).unwrap();
    released.lock_shared().unwrap();
    assert!(engine.takeover_verify(&task).is_err());
    let replacement = engine.retry(&task, None, true, None).unwrap();
    assert_ne!(replacement.id, run.id);
    engine.stop(&task).unwrap();
    released.unlock().unwrap();
    let current = engine.takeover_probe(&task).unwrap();
    assert_eq!(current.lock_inode, preview.lock_inode);
    assert_ne!(current.run, preview.run);
}

#[test]
fn takeover_reserves_a_task_checkout_against_landing_from_another_board() {
    let target_board = Fixture::new(&[]);
    let target = target_board.backlog("Keep this checkout reserved");
    let target_run = target_board.dispatch(&target);
    target_board.engine.stop(&target).unwrap();

    let mut source_board = Fixture::new(&[]);
    source_board.engine.pipeline = PipelineMode::Off;
    source_board.project().path = target_board.repo.clone();
    source_board.engine.store.set_first_id(100).unwrap();
    let source = source_board.backlog("Land onto target");
    let source_run = source_board.dispatch(&source);
    fs::write(source_run.worktree.join("file.txt"), "source change\n").unwrap();
    source_board.done(&source);

    let reservation = target_board.engine.takeover_probe(&target).unwrap();
    let host = File::open(&reservation.lock).unwrap();
    host.lock().unwrap();
    let engine = &source_board.engine;
    assert!(engine.store.get_task(&target).unwrap().is_none());
    let error = engine
        .approve(&source, Some(&target_run.branch), true)
        .unwrap_err();
    assert!(crate::takeover::is_checkout_reserved(&error));
    assert_eq!(
        fs::read_to_string(target_run.worktree.join("file.txt")).unwrap(),
        "base\n"
    );
    host.unlock().unwrap();
    engine
        .approve(&source, Some(&target_run.branch), true)
        .unwrap();
    assert_eq!(
        fs::read_to_string(target_run.worktree.join("file.txt")).unwrap(),
        "source change\n"
    );
}

#[test]
fn gate_leases_the_selected_run_even_when_admission_pinned_the_prior_repo() {
    let mut fixture = Fixture::new(&[]);
    let task = fixture.backlog("Reproject a waiting gate");
    let prior = fixture.dispatch(&task);
    fixture.engine.stop(&task).unwrap();
    // Model a gate paused between its outer admission and selecting the run.
    let _prior_writer = fixture.engine.checkout_writer(&task).unwrap();
    let replacement = Fixture::new(&[]);
    fixture.project().path = replacement.repo.clone();
    let engine = &fixture.engine;
    engine.move_task(&task, Status::Backlog, true).unwrap();
    let selected = fixture.dispatch(&task);
    assert_ne!(prior.worktree, selected.worktree);
    engine.stop(&task).unwrap();

    let preview = engine.takeover_probe(&task).unwrap();
    let host = File::open(&preview.lock).unwrap();
    host.lock().unwrap();
    let mut effects = false;
    let error = engine
        .run_gate(&selected, 0, None, crate::Actor::Owner, &mut effects)
        .unwrap_err();
    assert!(crate::takeover::is_checkout_reserved(&error));
    assert!(!effects);
}

#[test]
fn takeover_rejects_alias_and_reports_recreated_checkout_identity() {
    let fixture = Fixture::new(&[]);
    let task = fixture.backlog("Identify checkout");
    let run = fixture.dispatch(&task);
    let engine = &fixture.engine;
    engine.stop(&task).unwrap();
    let preview = engine.takeover_probe(&task).unwrap();
    let prior = run.worktree.with_file_name("moved");
    fs::rename(&run.worktree, &prior).unwrap();
    std::os::unix::fs::symlink(&prior, &run.worktree).unwrap();
    assert!(engine.takeover_probe(&task).is_err());

    fs::remove_file(&run.worktree).unwrap();
    git(&fixture.repo, &["worktree", "prune", "--expire", "now"]);
    git(
        &fixture.repo,
        &[
            "worktree",
            "add",
            run.worktree.to_str().unwrap(),
            &run.branch,
        ],
    );
    let host = File::open(&preview.lock).unwrap();
    host.lock().unwrap();
    let recreated = engine.takeover_verify(&task).unwrap();
    assert_eq!(recreated.checkout, preview.checkout);
    assert_eq!(recreated.run, preview.run);
    assert_ne!(recreated.inode, preview.inode);
}
