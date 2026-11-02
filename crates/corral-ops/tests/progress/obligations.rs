use corral_core::{
    DecisionRequest, ObligationSource, ProfileChoice, Status, WaitHandle, WaitRequest,
};
use corral_ops::{done, register_decision, register_wait, retry_run, save_checkpoint};
use serde_json::Value;

#[cfg(unix)]
use super::delivery::worker;
use super::fixture::Fixture;

fn decision(f: &Fixture) {
    register_decision(
        &f.ctx,
        &f.id,
        DecisionRequest {
            version: 1,
            id: "release".into(),
            run: "R1".into(),
            question: "Which release branch?".into(),
            requested_action: "Choose a branch".into(),
            owner: "owner".into(),
            source: ObligationSource::Worker,
            deadline: None,
        },
        "2026-10-15T12:00:00Z".parse().unwrap(),
    )
    .unwrap();
}

#[test]
#[cfg(unix)]
fn native_obligations_survive_checkpoint_and_fresh_or_stale_resume() {
    let f = worker();
    register_wait(
        &f.ctx,
        &f.id,
        WaitRequest {
            version: 1,
            id: "native-build".into(),
            run: "R1".into(),
            condition: "The external build completes".into(),
            owner: "fixture-a".into(),
            source: ObligationSource::Worker,
            deadline: None,
            handle: WaitHandle::Channel {
                channel_id: "build-42".into(),
                capability: None,
            },
        },
        "2026-10-15T12:00:00Z".parse().unwrap(),
    )
    .unwrap();
    decision(&f);
    f.seed_checkpoint();
    let mut task = f.task();
    let source = task.current_run().unwrap();
    let native = source.obligations.clone();
    assert_eq!(native.waits.len(), 1);
    assert!(native.pending_decision().is_some());
    let record = source.checkpoints.last().unwrap().clone();
    assert_eq!(record.checkpoint.run_obligations, native);
    let bytes = f.checkpoint_bytes(&record.artifact);
    let frozen: corral_core::Checkpoint = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(frozen.run_obligations, native);

    let mut target = source.clone();
    target.id = "R2".into();
    target.worktree = f.tmp.path().join("fresh-worker");
    target.obligations = Default::default();
    corral_git::worktree_add(&f.repo, &target.worktree, "fresh-worker", "main").unwrap();
    for stale in [false, true] {
        if stale {
            task.body.push_str("\nA revised contract.\n");
        }
        let prompt = corral_ops::task_prompt(&f.ctx, &task, &target).unwrap();
        let json = prompt
            .split_once("<corral-resume>\n")
            .unwrap()
            .1
            .split_once("\n</corral-resume>")
            .unwrap()
            .0;
        let packet: Value = serde_json::from_str(json).unwrap();
        assert_eq!(packet["status"], if stale { "stale" } else { "compatible" });
        assert_eq!(
            packet["prior_run_obligations"],
            serde_json::to_value(&native).unwrap()
        );
        assert!(target.obligations.is_empty());
        assert_eq!(f.checkpoint_bytes(&record.artifact), bytes);
    }
}

#[test]
#[cfg(unix)]
fn a_new_native_decision_requires_a_fresh_handoff_without_rewriting_history() {
    let f = worker();
    f.seed_checkpoint();
    decision(&f);
    let back = corral_ops::progress_readback(&f.ctx, &f.id).unwrap();
    assert_eq!(back.checkpoint_status, corral_core::ResumeStatus::Stale);
    assert!(
        back.checkpoint_reasons
            .iter()
            .any(|r| r == "obligations_changed")
    );
    let before = f.snapshot();
    let error = done(&f.ctx, &f.id, "Implemented").unwrap_err();
    assert!(error.to_string().contains("obligations_changed"));
    assert_eq!(f.snapshot(), before);
    let old = f.task().current_run().unwrap().checkpoints[0].clone();
    save_checkpoint(&f.ctx, &f.id, &f.checkpoint()).unwrap();
    let task = f.task();
    let run = task.current_run().unwrap();
    assert_eq!(run.checkpoints[0], old);
    assert!(
        run.checkpoints[1]
            .checkpoint
            .run_obligations
            .pending_decision()
            .is_some()
    );
}

#[test]
#[cfg(unix)]
fn done_with_large_history_and_evidence_scratch() {
    let f = worker();
    std::fs::write(f.repo.join(".gitignore"), ".evidence/\n").unwrap();
    super::fixture::git(&f.repo, &["add", ".gitignore"]);
    super::fixture::git(&f.repo, &["commit", "-qm", "ignore scratch"]);
    let scratch = f.repo.join(".evidence");
    std::fs::create_dir(&scratch).unwrap();
    for n in 0..1637 {
        use std::io::Write;
        let mut file = std::fs::File::create(scratch.join(format!("{n}.bin"))).unwrap();
        file.write_all(format!("{n:08}").as_bytes()).unwrap();
        file.set_len(76_000).unwrap();
    }
    f.seed_checkpoint();
    f.ctx
        .store
        .update(&f.id, |task| {
            let artifact = task.artifacts[0].clone();
            for n in 0..439 {
                let mut a = artifact.clone();
                a.path = format!("artifacts/synthetic-{n}");
                a.sha256 = format!("{n:064x}");
                task.artifacts.push(a);
            }
            let mut older = task.current_run().unwrap().clone();
            older.checkpoints.clear();
            older.progress = None;
            older.id = "older-1".into();
            task.runs.insert(0, older.clone());
            older.id = "older-2".into();
            task.runs.insert(0, older);
            let run = task.current_run_mut().unwrap();
            let latest = run.checkpoints[0].clone();
            for n in 0..50 {
                let mut c = latest.clone();
                c.artifact = format!("synthetic-{n}");
                c.checkpoint.input.next_action = "x".repeat(150_000);
                run.checkpoints.insert(0, c);
            }
            Ok(())
        })
        .unwrap();
    // A full task listing clones history even when decoding is cached. It must not
    // happen once per scratch artifact; unlike timing or cache reads, this count
    // does not depend on host load or the file-stamp settle window.
    use corral_core::work::{Work, count};
    let lists = count(Work::TaskList);
    let gets = count(Work::TaskGet);
    done(&f.ctx, &f.id, "Implemented").unwrap();
    let lists = count(Work::TaskList) - lists;
    let gets = count(Work::TaskGet) - gets;
    eprintln!("large-history done: {lists} task listings, {gets} task lookups");
    assert!(
        lists < 100,
        "task listings scaled with scratch artifacts: {lists}"
    );
    assert!(
        gets < 100,
        "task lookups scaled with scratch artifacts: {gets}"
    );
    assert_eq!(f.task().status, Status::Review);
    assert_eq!(
        f.task()
            .artifacts
            .iter()
            .filter(|a| a.title.ends_with(".bin"))
            .count(),
        1637
    );
}

#[test]
#[cfg(unix)]
fn resending_a_checkpoint_request_does_not_answer_an_owner_decision() {
    let f = worker();
    decision(&f);
    std::fs::write(f.tmp.path().join("reject"), "").unwrap();
    assert!(retry_run(&f.ctx, &f.id, "R1", &ProfileChoice::default()).is_err());
    assert!(corral_ops::resend_reply(&f.ctx, &f.id).is_err());
    assert!(
        f.task()
            .current_run()
            .unwrap()
            .obligations
            .pending_decision()
            .is_some()
    );
    std::fs::remove_file(f.tmp.path().join("reject")).unwrap();
    let task = corral_ops::resend_reply(&f.ctx, &f.id).unwrap();
    let run = task.current_run().unwrap();
    assert!(run.pending_reply.is_none());
    assert!(run.obligations.pending_decision().is_some());
    assert_eq!(task.status, Status::NeedsMe);
    assert_eq!(task.runs.len(), 1);
    assert!(!f.tmp.path().join("closed").exists());
}
