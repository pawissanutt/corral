#[path = "progress/delivery.rs"]
mod delivery;
#[path = "progress/fixture.rs"]
mod fixture;
#[path = "progress/obligations.rs"]
mod obligations;

use corral_core::{Checkpoint, ProfileChoice, RunOutcome, Status, StepState, Store};
use corral_ops::{Ctx, done, progress_readback, retry_run, save_checkpoint, update_progress};
use fixture::{Fixture, git};
use serde_json::json;

#[test]
fn two_phase_progress_and_checkpoint_survive_restart() {
    // Given stable phase/step identities with completed, current and pending work.
    let f = Fixture::new();
    let input = f.seed_checkpoint();
    let before = progress_readback(&f.ctx, &f.id).unwrap();

    // When the process reconstructs its context from the persisted store.
    let restarted = Ctx {
        store: Store::open(f.tmp.path()).unwrap(),
        herdr: corral_herdr::Herdr::new(Some("corral-test".into())),
    };
    let read = progress_readback(&restarted, &f.id).unwrap();

    // Then the complete guarded identity and portable snapshot round-trip.
    assert_eq!(read, before);
    let progress = read.progress.unwrap();
    assert_eq!(progress.revision, 1);
    assert_eq!(progress.phases.len(), 2);
    assert_eq!(progress.phases[0].id, "design");
    assert_eq!(progress.phases[1].id, "implementation");
    assert_eq!(progress.current_step().unwrap().id, "code");
    assert_eq!(progress.steps().next().unwrap().status, StepState::Done);
    let checkpoint = &read.checkpoints[0].checkpoint;
    assert_eq!(checkpoint.input, input);
    assert_eq!(checkpoint.completed_steps, ["contract"]);
    assert_eq!(checkpoint.current_step.as_deref(), Some("code"));
    let packet = f.packet();
    assert_eq!(packet["status"], "compatible");
    assert_eq!(packet["progress"], serde_json::to_value(progress).unwrap());
    assert_eq!(packet["reusable_evidence"], json!(input.evidence_refs));
}

#[test]
fn renaming_labels_preserves_phase_and_step_identities() {
    // Given previously persisted progress.
    let f = Fixture::new();
    f.seed_progress();
    let original = f.task().current_run().unwrap().progress.clone().unwrap();
    let mut input = f.progress();
    for phase in &mut input.phases {
        phase.label.push_str(" renamed");
        for step in &mut phase.steps {
            step.label.push_str(" renamed");
        }
    }

    // When only display labels change.
    let task = update_progress(&f.ctx, &f.id, &input).unwrap();

    // Then IDs, statuses and criterion references remain stable.
    let progress = task.current_run().unwrap().progress.as_ref().unwrap();
    assert_eq!(progress.revision, 2);
    assert_eq!(progress.phases, input.phases);
    for (old, new) in original.phases.iter().zip(&progress.phases) {
        assert_eq!(old.id, new.id);
        assert_ne!(old.label, new.label);
    }
    for (old, new) in original.steps().zip(progress.steps()) {
        assert_eq!(old.id, new.id);
        assert_eq!(old.status, new.status);
        assert_eq!(old.criteria, new.criteria);
        assert_ne!(old.label, new.label);
    }
}

#[test]
fn invalid_progress_updates_leave_task_events_and_artifacts_unchanged() {
    for case in [
        "duplicate_phase",
        "duplicate_step",
        "stale_revision",
        "stale_spec",
        "stale_run",
        "stale_task",
        "omitted_phase",
        "omitted_step",
        "terminal_regression",
        "empty_drop_reason",
    ] {
        // Given a checkpointed progress record.
        let f = Fixture::new();
        f.seed_checkpoint();
        let mut input = f.progress();
        let code = match case {
            "duplicate_phase" => {
                input.phases[1].id = input.phases[0].id.clone();
                "duplicate_or_empty_id"
            }
            "duplicate_step" => {
                input.phases[1].steps[0].id = input.phases[0].steps[0].id.clone();
                "duplicate_or_empty_id"
            }
            "stale_revision" => {
                input.expected_revision = 0;
                "stale_progress"
            }
            "stale_spec" => {
                input.spec_revision = "obsolete".into();
                "stale_spec"
            }
            "stale_run" => {
                input.run = "R0".into();
                "stale_run"
            }
            "stale_task" => {
                input.task = "T-999".into();
                "task_identity_mismatch"
            }
            "omitted_phase" => {
                input.phases.remove(0);
                "phase_removed"
            }
            "omitted_step" => {
                input.phases[1].steps.pop();
                "step_removed"
            }
            "terminal_regression" => {
                input.phases[0].steps[0].status = StepState::Pending;
                "terminal_step_changed"
            }
            "empty_drop_reason" => {
                input.phases[1].steps[1].status = StepState::Dropped {
                    reason: " \n".into(),
                };
                "drop_reason_required"
            }
            _ => unreachable!(),
        };
        let before = f.snapshot();

        // When a malformed or obsolete write is attempted.
        let error = update_progress(&f.ctx, &f.id, &input).unwrap_err();

        // Then it fails by machine error category with no persisted mutation.
        assert_eq!(error.to_string().split(':').next().unwrap(), code, "{case}");
        assert_eq!(f.snapshot(), before, "{case}");
    }
}

#[test]
fn dropped_step_keeps_reason_and_criteria_without_becoming_completed() {
    // Given a pending step associated with a criterion, not acceptance authority.
    let f = Fixture::new();
    f.seed_progress();
    let mut input = f.progress();
    input.phases[1].steps[1].status = StepState::Dropped {
        reason: "Covered by a separate task".into(),
    };
    update_progress(&f.ctx, &f.id, &input).unwrap();

    // When a checkpoint captures the dropped step.
    let task = save_checkpoint(&f.ctx, &f.id, &f.checkpoint()).unwrap();

    // Then dropping is not completion or an acceptance receipt.
    let run = task.current_run().unwrap();
    let checkpoint = &run.checkpoints[0].checkpoint;
    assert_eq!(checkpoint.completed_steps, ["contract"]);
    let dropped = checkpoint
        .progress
        .as_ref()
        .unwrap()
        .steps()
        .find(|s| s.id == "verify")
        .unwrap();
    assert_eq!(dropped.status, input.phases[1].steps[1].status);
    assert_eq!(dropped.criteria, ["C-1"]);
    assert!(checkpoint.input.evidence_refs.is_empty());
    assert!(run.outcome.is_none());
    assert_eq!(task.status, Status::Running);
    assert_eq!(task.artifacts.len(), 1);
    assert_eq!(task.artifacts[0].path, run.checkpoints[0].artifact);
}

#[test]
fn dropped_terminal_step_cannot_be_reopened_or_rewritten() {
    for replacement in [
        StepState::Pending,
        StepState::Done,
        StepState::Dropped {
            reason: "Different reason".into(),
        },
    ] {
        // Given a terminal dropped step with a recorded reason.
        let f = Fixture::new();
        let mut initial = f.progress();
        initial.phases[1].steps[1].status = StepState::Dropped {
            reason: "Out of scope".into(),
        };
        update_progress(&f.ctx, &f.id, &initial).unwrap();
        let mut input = f.progress();
        input.phases = initial.phases;
        input.phases[1].steps[1].status = replacement;
        let before = f.snapshot();

        // When an update changes its terminal state or reason.
        let error = update_progress(&f.ctx, &f.id, &input).unwrap_err();

        // Then the original record stays untouched.
        assert_eq!(
            error.to_string().split(':').next().unwrap(),
            "terminal_step_changed"
        );
        assert_eq!(f.snapshot(), before);
    }
}

#[test]
fn later_checkpoint_cannot_mutate_earlier_snapshot_or_pending_obligations() {
    // Given an immutable checkpoint artifact containing outstanding obligations.
    let f = Fixture::new();
    let first_input = f.seed_checkpoint();
    let first_task = f.task();
    let first_record = first_task.current_run().unwrap().checkpoints[0].clone();
    let first_path = f.ctx.store.dir().join(&first_record.artifact);
    let first_bytes = std::fs::read(&first_path).unwrap();
    let mut progress = f.progress();
    progress.phases[1].steps[0].status = StepState::Done;
    progress.phases[1].steps[1].status = StepState::InProgress;
    update_progress(&f.ctx, &f.id, &progress).unwrap();
    let mut next = f.checkpoint();
    next.evidence_refs = first_input.evidence_refs.clone();
    next.next_action = "Collect final result".into();

    // When a later checkpoint records the new execution position.
    let task = save_checkpoint(&f.ctx, &f.id, &next).unwrap();

    // Then history remains byte-stable and pending obligations are portable.
    let records = &task.current_run().unwrap().checkpoints;
    assert_eq!(records.len(), 2);
    assert_eq!(records[0], first_record);
    assert_eq!(std::fs::read(&first_path).unwrap(), first_bytes);
    let on_disk: Checkpoint = serde_json::from_slice(&first_bytes).unwrap();
    assert_eq!(on_disk, first_record.checkpoint);
    assert_ne!(records[0].artifact, records[1].artifact);
    assert_eq!(records[1].checkpoint.revision, 2);
    assert_eq!(records[1].checkpoint.completed_steps, ["contract", "code"]);
    assert_eq!(
        records[1].checkpoint.current_step.as_deref(),
        Some("verify")
    );
    assert_eq!(records[0].checkpoint.completed_steps, ["contract"]);
    assert_eq!(records[0].checkpoint.input.waits, first_input.waits);
    assert_eq!(records[0].checkpoint.input.resources, first_input.resources);
    let latest_bytes = std::fs::read(f.ctx.store.dir().join(&records[1].artifact)).unwrap();
    let latest: Checkpoint = serde_json::from_slice(&latest_bytes).unwrap();
    assert_eq!(latest, records[1].checkpoint);
    let packet = f.packet();
    assert_eq!(packet["status"], "compatible");
    assert_eq!(
        packet["checkpoint"]["checkpoint"]["input"]["waits"],
        json!(first_input.waits)
    );
    assert_eq!(
        packet["checkpoint"]["checkpoint"]["input"]["resources"],
        json!(first_input.resources)
    );
    assert_eq!(
        packet["checkpoint"]["checkpoint"]["input"]["decisions"],
        json!(next.decisions)
    );
}

#[test]
fn pending_obligations_cannot_vanish_without_mutation() {
    for omit_wait in [true, false] {
        // Given a checkpoint with both a pending wait and an owned resource.
        let f = Fixture::new();
        f.seed_checkpoint();
        let mut input = f.checkpoint();
        if omit_wait {
            input.waits.clear();
        } else {
            input.resources.clear();
        }
        let before = f.snapshot();

        // When a subsequent checkpoint silently omits either obligation.
        let error = save_checkpoint(&f.ctx, &f.id, &input).unwrap_err();

        // Then rejection leaves both metadata and immutable artifacts unchanged.
        assert_eq!(
            error.to_string().split(':').next().unwrap(),
            "pending_obligation_removed"
        );
        assert_eq!(f.snapshot(), before);
    }
}

#[test]
fn blank_obligation_resolution_is_rejected_without_mutation() {
    for resolve_wait in [true, false] {
        // Given pending obligations in the previous checkpoint.
        let f = Fixture::new();
        f.seed_checkpoint();
        let mut input = f.checkpoint();
        if resolve_wait {
            input.waits[0].resolution = Some(" \n".into());
        } else {
            input.resources[0].resolution = Some(" \n".into());
        }
        let before = f.snapshot();

        // When an update claims resolution without a concrete reason or receipt.
        let error = save_checkpoint(&f.ctx, &f.id, &input).unwrap_err();

        // Then no obligation or checkpoint changes.
        assert_eq!(
            error.to_string().split(':').next().unwrap(),
            "empty_obligation_resolution"
        );
        assert_eq!(f.snapshot(), before);
    }
}

#[test]
fn explicit_obligation_resolution_persists_without_rewriting_pending_history() {
    // Given pending obligations and their immutable historical snapshot.
    let f = Fixture::new();
    f.seed_checkpoint();
    let pending = f.task().current_run().unwrap().checkpoints[0].clone();
    let pending_path = f.ctx.store.dir().join(&pending.artifact);
    let pending_bytes = std::fs::read(&pending_path).unwrap();
    let mut input = f.checkpoint();
    input.waits[0].resolution = Some("build-42 exited with code 0".into());
    input.resources[0].resolution = Some("server-42 stopped".into());

    // When resolution is explicitly captured in the next checkpoint.
    save_checkpoint(&f.ctx, &f.id, &input).unwrap();

    // Then readback after reopening preserves both pending and resolved history.
    let store = Store::open(f.tmp.path()).unwrap();
    let task = store.get(&f.id).unwrap();
    let records = &task.current_run().unwrap().checkpoints;
    assert_eq!(records.len(), 2);
    assert_eq!(records[0], pending);
    assert!(records[0].checkpoint.input.waits[0].resolution.is_none());
    assert!(
        records[0].checkpoint.input.resources[0]
            .resolution
            .is_none()
    );
    assert_eq!(records[1].checkpoint.input.waits, input.waits);
    assert_eq!(records[1].checkpoint.input.resources, input.resources);
    assert_eq!(std::fs::read(pending_path).unwrap(), pending_bytes);
    let latest_bytes = std::fs::read(store.dir().join(&records[1].artifact)).unwrap();
    let latest: Checkpoint = serde_json::from_slice(&latest_bytes).unwrap();
    assert_eq!(latest.input, input);
    let packet = f.packet();
    assert_eq!(packet["status"], "compatible");
    assert_eq!(
        packet["checkpoint"]["checkpoint"]["input"]["waits"],
        json!(input.waits)
    );
    assert_eq!(
        packet["checkpoint"]["checkpoint"]["input"]["resources"],
        json!(input.resources)
    );
}

#[test]
fn invalid_checkpoint_writes_leave_history_and_artifacts_unchanged() {
    for case in [
        "revision",
        "progress",
        "task",
        "run",
        "spec",
        "head",
        "base",
        "tree",
        "unknown_evidence",
    ] {
        // Given a valid prior checkpoint.
        let f = Fixture::new();
        f.seed_checkpoint();
        let mut input = f.checkpoint();
        let code = match case {
            "revision" => {
                input.expected_revision = 0;
                "stale_checkpoint"
            }
            "progress" => {
                input.progress_revision = 0;
                "stale_progress"
            }
            "task" => {
                input.task = "T-999".into();
                "task_identity_mismatch"
            }
            "run" => {
                input.run = "R0".into();
                "stale_run"
            }
            "spec" => {
                input.spec_revision = "obsolete".into();
                "stale_spec"
            }
            "head" => {
                input.head = "0".repeat(40);
                "stale_head"
            }
            "base" => {
                input.base = "0".repeat(40);
                "stale_base"
            }
            "tree" => {
                input.tree = "obsolete".into();
                "stale_tree"
            }
            "unknown_evidence" => {
                input.evidence_refs = vec!["artifacts/missing.json".into()];
                "unknown_evidence_reference"
            }
            _ => unreachable!(),
        };
        let before = f.snapshot();

        // When the checkpoint identity or artifact reference is invalid.
        let error = save_checkpoint(&f.ctx, &f.id, &input).unwrap_err();

        // Then no new checkpoint artifact, task revision or event is written.
        assert_eq!(error.to_string().split(':').next().unwrap(), code, "{case}");
        assert_eq!(f.snapshot(), before, "{case}");
    }
}

#[test]
fn missing_checkpoint_has_no_authoritative_resume_progress_or_evidence() {
    // Given progress that was never captured in a checkpoint.
    let f = Fixture::new();
    f.seed_progress();
    let before = f.snapshot();

    // When the worker receives its structured resume packet.
    let packet = f.packet();

    // Then uncaptured progress is not promoted.
    assert_eq!(packet["version"], 1);
    assert_eq!(packet["task"], f.id);
    assert_eq!(packet["run"], "R1");
    assert_eq!(packet["status"], "missing");
    assert_eq!(packet["reasons"], json!(["checkpoint_missing"]));
    assert!(packet["checkpoint"].is_null());
    assert!(packet["progress"].is_null());
    assert_eq!(packet["reusable_evidence"], json!([]));
    assert_eq!(f.snapshot(), before);
}

#[test]
fn stale_resume_retains_history_but_never_promotes_progress_or_evidence() {
    for case in [
        "head",
        "spec",
        "tracked_tree",
        "untracked_binary",
        "progress",
    ] {
        // Given checkpointed progress, evidence and cleanup obligations.
        let f = Fixture::new();
        if case == "untracked_binary" {
            std::fs::write(f.repo.join("payload.bin"), [0, 255, 1, 0]).unwrap();
        }
        let input = f.seed_checkpoint();
        let original = f.task().current_run().unwrap().checkpoints[0].clone();
        let reason = match case {
            "head" => {
                git(&f.repo, &["commit", "--allow-empty", "-q", "-m", "advance"]);
                "head_changed"
            }
            "spec" => {
                f.ctx
                    .store
                    .update(&f.id, |task| {
                        task.body.push_str("Changed specification.\n");
                        Ok(())
                    })
                    .unwrap();
                "spec_changed"
            }
            "tracked_tree" => {
                std::fs::write(f.repo.join("README.md"), "modified\n").unwrap();
                "tree_changed"
            }
            "untracked_binary" => {
                // Same path and byte count: the digest must include binary content.
                std::fs::write(f.repo.join("payload.bin"), [0, 255, 2, 0]).unwrap();
                "tree_changed"
            }
            "progress" => {
                let mut progress = f.progress();
                progress.phases[1].steps[0].label.push_str(" revised");
                update_progress(&f.ctx, &f.id, &progress).unwrap();
                "progress_changed"
            }
            _ => unreachable!(),
        };
        let before = f.snapshot();

        // When a changed environment receives its resume packet.
        let packet = f.packet();

        // Then historical obligations survive, but authority does not.
        assert_eq!(packet["status"], "stale", "{case}");
        assert!(
            packet["reasons"]
                .as_array()
                .unwrap()
                .contains(&json!(reason)),
            "{case}: {packet}"
        );
        assert!(packet["progress"].is_null(), "{case}");
        assert_eq!(packet["reusable_evidence"], json!([]), "{case}");
        assert_eq!(
            packet["checkpoint"],
            serde_json::to_value(&original).unwrap(),
            "{case}"
        );
        assert_eq!(
            packet["checkpoint"]["checkpoint"]["input"]["waits"],
            json!(input.waits)
        );
        assert_eq!(
            packet["checkpoint"]["checkpoint"]["input"]["resources"],
            json!(input.resources)
        );
        assert_eq!(f.snapshot(), before, "{case}");
    }
}

#[test]
fn compatible_checkpoint_on_new_run_does_not_promote_old_run_evidence() {
    // Given a compatible source checkpoint and a new run on the same tree.
    let f = Fixture::new();
    f.seed_checkpoint();
    let task = f.task();
    let mut target = task.current_run().unwrap().clone();
    target.id = "R2".into();
    target.checkpoints.clear();
    target.progress = None;
    let before = f.snapshot();

    // When dispatch asks for a resume packet for that target.
    let packet = corral_ops::resume_packet(&f.ctx, &task, &target).unwrap();
    let packet = serde_json::to_value(packet).unwrap();

    // Then compatible execution progress is reusable, not old evidence receipts.
    assert_eq!(packet["status"], "compatible");
    assert_eq!(packet["run"], "R2");
    assert_eq!(
        packet["progress"],
        serde_json::to_value(task.current_run().unwrap().progress.as_ref()).unwrap()
    );
    assert_eq!(packet["reusable_evidence"], json!([]));
    assert_eq!(packet["checkpoint"]["checkpoint"]["input"]["run"], "R1");
    assert_eq!(f.snapshot(), before);
}

#[test]
fn obsolete_retry_run_rejects_without_superseding_or_dispatching() {
    // Given a current run and a retry request from an obsolete worker.
    let f = Fixture::new();
    f.seed_checkpoint();
    let before = f.snapshot();

    // When the stale worker requests a new run.
    let error = retry_run(&f.ctx, &f.id, "R0", &ProfileChoice::default()).unwrap_err();

    // Then rejection precedes routing, herdr calls and superseding.
    assert_eq!(error.to_string().split(':').next().unwrap(), "stale_run");
    assert_eq!(f.snapshot(), before);
    let task = f.task();
    assert_eq!(task.status, Status::Running);
    assert_eq!(task.runs.len(), 1);
    assert!(task.current_run().unwrap().outcome.is_none());
    assert!(task.current_run().unwrap().retry.is_none());
    assert!(!f.repo.join(".worktrees").exists());
}

#[test]
fn opted_in_done_requires_a_present_current_checkpoint_without_mutation() {
    for case in ["missing", "head", "spec", "tree", "progress"] {
        // Given an opted-in run whose checkpoint is missing or no longer valid.
        let f = Fixture::new();
        if case != "missing" {
            f.seed_checkpoint();
        }
        match case {
            "missing" => {}
            "head" => git(&f.repo, &["commit", "--allow-empty", "-q", "-m", "advance"]),
            "spec" => {
                f.ctx
                    .store
                    .update(&f.id, |task| {
                        task.body.push_str("Changed specification.\n");
                        Ok(())
                    })
                    .unwrap();
            }
            "tree" => std::fs::write(f.repo.join("README.md"), "dirty\n").unwrap(),
            "progress" => {
                update_progress(&f.ctx, &f.id, &f.progress()).unwrap();
            }
            _ => unreachable!(),
        }
        let before = f.snapshot();
        let head = corral_git::rev_sha(&f.repo, "HEAD").unwrap();

        // When the worker tries to finish.
        let error = done(&f.ctx, &f.id, "Finished").unwrap_err();

        // Then neither auto-commit nor a completion receipt occurs.
        let code = if case == "missing" {
            "checkpoint_required"
        } else {
            "checkpoint_stale"
        };
        assert_eq!(error.to_string().split(':').next().unwrap(), code, "{case}");
        assert_eq!(f.snapshot(), before, "{case}");
        assert_eq!(corral_git::rev_sha(&f.repo, "HEAD").unwrap(), head);
        assert_eq!(f.task().status, Status::Running);
        assert!(f.task().current_run().unwrap().outcome.is_none());
    }
}

#[test]
fn checkpointed_dirty_tree_must_be_committed_before_done() {
    // Given a current checkpoint that includes uncommitted work.
    let f = Fixture::new();
    std::fs::write(f.repo.join("README.md"), "dirty\n").unwrap();
    f.seed_checkpoint();
    let before = f.snapshot();
    let head = corral_git::rev_sha(&f.repo, "HEAD").unwrap();

    // When the worker finishes without first committing and checkpointing.
    let error = done(&f.ctx, &f.id, "Finished").unwrap_err();

    // Then done cannot silently invalidate its own checkpoint by committing.
    assert_eq!(
        error.to_string().split(':').next().unwrap(),
        "commit_before_checkpoint"
    );
    assert_eq!(f.snapshot(), before);
    assert_eq!(corral_git::rev_sha(&f.repo, "HEAD").unwrap(), head);
}

#[test]
fn valid_checkpoint_allows_exactly_one_done_transition() {
    // Given committed work and a checkpoint for its current HEAD.
    let f = Fixture::new();
    std::fs::write(f.repo.join("README.md"), "implemented\n").unwrap();
    git(&f.repo, &["add", "-A"]);
    git(&f.repo, &["commit", "-q", "-m", "implementation"]);
    f.seed_checkpoint();

    // When done is submitted, then repeated.
    let task = done(&f.ctx, &f.id, "Finished").unwrap();
    let after = f.snapshot();
    assert!(done(&f.ctx, &f.id, "Duplicate").is_err());

    // Then one completion reaches Review with one event and no duplicate artifacts.
    assert_eq!(task.status, Status::Review);
    let run = task.current_run().unwrap();
    assert_eq!(run.outcome, Some(RunOutcome::Done));
    assert!(run.ended.is_some());
    assert_eq!(run.summary.as_deref(), Some("Finished"));
    assert_eq!(f.snapshot(), after);
    let events = f.ctx.store.events().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.task.as_deref() == Some(f.id.as_str()) && event.kind == "done")
            .count(),
        1
    );
    assert_eq!(
        task.artifacts
            .iter()
            .filter(|a| a.kind == corral_core::ArtifactKind::Summary)
            .count(),
        1
    );
    assert_eq!(
        task.artifacts
            .iter()
            .filter(|a| a.kind == corral_core::ArtifactKind::Diff)
            .count(),
        1
    );
}

#[test]
fn finished_worker_can_refresh_checkpoint_without_repeating_completion() {
    // Given a completed handoff whose base has moved before a reachable retry.
    let f = Fixture::new();
    f.seed_checkpoint();
    done(&f.ctx, &f.id, "Finished").unwrap();
    git(&f.repo, &["commit", "--allow-empty", "-q", "-m", "advance"]);
    let before = f.task();
    let next = f.checkpoint();

    // When the same worker refreshes only its portable checkpoint.
    let after = save_checkpoint(&f.ctx, &f.id, &next).unwrap();

    // Then completion authority is unchanged and progress cannot reopen.
    assert_eq!(after.status, before.status);
    assert_eq!(after.current_run().unwrap().outcome, Some(RunOutcome::Done));
    assert_eq!(
        after.current_run().unwrap().ended,
        before.current_run().unwrap().ended
    );
    assert_eq!(after.current_run().unwrap().checkpoints.len(), 2);
    assert!(update_progress(&f.ctx, &f.id, &f.progress()).is_err());
    assert_eq!(
        f.ctx
            .store
            .events()
            .unwrap()
            .iter()
            .filter(|e| e.kind == "done")
            .count(),
        1
    );
}

#[test]
fn custom_prompt_cannot_remove_canonical_task_contract() {
    // Given a harness template that omits the task body entirely.
    let f = Fixture::new();
    std::fs::write(f.ctx.store.prompts_dir().join("run.md"), "custom").unwrap();
    let task = f.task();
    // When a fresh worker receives its missing-checkpoint packet.
    let packet = f.packet();
    // Then the current task contract still arrives as structured canonical data.
    assert_eq!(packet["canonical_title"], task.title);
    assert_eq!(packet["canonical_spec"], task.body);
    assert_eq!(packet["status"], "missing");
}

#[test]
fn newest_compatible_snapshot_reuses_progress_but_keeps_latest_obligations() {
    // Given a worker branch with two snapshots and an unchanged progress revision.
    let f = Fixture::new();
    git(&f.repo, &["checkout", "-qb", "worker"]);
    f.seed_checkpoint();
    git(
        &f.repo,
        &["commit", "--allow-empty", "-qm", "worker advance"],
    );
    let mut latest = f.checkpoint();
    latest.waits[0].handle = "build-43".into();
    save_checkpoint(&f.ctx, &f.id, &latest).unwrap();
    let task = f.task();
    let mut target = task.current_run().unwrap().clone();
    target.id = "R2".into();
    target.worktree = f.repo.join(".worktrees/replacement");
    corral_git::worktree_add(&f.repo, &target.worktree, "replacement", "main").unwrap();

    // When a replacement starts on the older, exactly compatible base.
    let packet =
        serde_json::to_value(corral_ops::resume_packet(&f.ctx, &task, &target).unwrap()).unwrap();

    // Then old valid progress is reusable without losing the latest wait/cleanup context.
    assert_eq!(packet["status"], "compatible");
    assert_eq!(packet["compatible_checkpoint"]["checkpoint"]["revision"], 1);
    assert_eq!(packet["progress"]["revision"], 1);
    assert_eq!(packet["checkpoint"]["checkpoint"]["revision"], 2);
    assert_eq!(
        packet["checkpoint"]["checkpoint"]["input"]["waits"][0]["handle"],
        "build-43"
    );
    assert_eq!(
        packet["checkpoint"]["checkpoint"]["input"]["resources"][0]["id"],
        "server"
    );
    assert!(
        packet["reasons"]
            .as_array()
            .unwrap()
            .contains(&json!("head_changed"))
    );
    assert_eq!(packet["reusable_evidence"], json!([]));
}
