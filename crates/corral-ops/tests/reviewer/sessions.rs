use super::*;

const HARNESS: &str = r#"import json
import sys
from pathlib import Path

args = sys.argv[1:]
cwd = args[args.index("--cwd") + 1]
directory = Path(args[args.index("--session-dir") + 1])
directory.mkdir(parents=True, exist_ok=True)
session = Path(args[args.index("--session") + 1]) if "--session" in args else directory / "conversation.jsonl"
if "--session" in args:
    entries = [json.loads(line) for line in session.read_text().splitlines()]
    assert entries[0]["type"] == "session"
else:
    assert not session.exists()
    session.write_text(json.dumps({"type": "session", "id": str(directory), "cwd": cwd,
                                  "timestamp": "2026-10-26T00:00:00Z"}) + "\n")
with session.open("a") as output:
    output.write(json.dumps({"type": "message", "message": {"role": "user", "content": args[-1]}}) + "\n")
"#;

fn session_profile(ctx: &mut Ctx) {
    let script = ctx.store.root().join("session-harness.py");
    std::fs::write(&script, HARNESS).unwrap();
    reviewer_profile(
        ctx,
        format!(
            "python3 '{}' --cwd '{{worktree}}' {{model_args}} {{prompt_ref}}",
            script.display()
        ),
    );
}

#[test]
fn default_reviewer_resumes_exact_conversation_in_a_reopened_store() {
    // Given an independent fake reviewer with a persisted conversation.
    let (_tmp, mut ctx, id, _) = task_in_review();
    session_profile(&mut ctx);
    let first = corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();
    let first_task = ctx.store.get(&id).unwrap();
    let first_run = first_task.current_run().unwrap();
    let first_child = serde_json::to_value(first_run.review_runs.last().unwrap()).unwrap();
    assert!(first_child["session"]["id"].is_string(), "{first_child}");
    let callback = corral_ops::review_callback_token(&first).unwrap();
    corral_ops::verdict_bound(
        &ctx,
        &id,
        Verdict::Changes,
        "repair the widget",
        Some(&callback),
    )
    .unwrap();

    // When a later process requests the next review.
    ctx.store = Store::open(ctx.store.root()).unwrap();
    corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();

    // Then its new attempt resumes the same transcript without reusing callback authority.
    let task = ctx.store.get(&id).unwrap();
    let run = task.current_run().unwrap();
    let child = serde_json::to_value(run.review_runs.last().unwrap()).unwrap();
    assert_eq!(child["session"]["id"], first_child["session"]["id"]);
    assert_eq!(
        child["session"]["transcript"],
        first_child["session"]["transcript"]
    );
    assert_eq!(child["session"]["action"], "resumed");
    assert_ne!(
        corral_ops::review_callback_token(run.review.as_ref().unwrap()).unwrap(),
        callback
    );
    assert!(
        corral_ops::verdict_bound(&ctx, &id, Verdict::Approve, "old verdict", Some(&callback))
            .is_err()
    );
    let transcript = PathBuf::from(child["session"]["transcript"].as_str().unwrap());
    assert_eq!(
        std::fs::read_to_string(transcript).unwrap().lines().count(),
        3
    );
}

fn session_candidate() -> (tempfile::TempDir, Ctx, String, PathBuf) {
    let (tmp, mut ctx, _, _) = task_in_review();
    session_profile(&mut ctx);
    let harness = ctx.store.config.harnesses["omo"].clone();
    let mut harness = harness;
    harness.flags = Some("omo".into());
    ctx.store
        .config
        .harnesses
        .insert("fake-omo".into(), harness);
    let mut worker = ctx.store.config.profiles["rev"].clone();
    worker.harness = "fake-omo".into();
    worker.model = Some("worker-model".into());
    ctx.store.config.profiles.insert("fake".into(), worker);
    ctx.store.save_config().unwrap();
    let (id, worktree) = review_candidate(&ctx, "Keep both conversations", "initial.txt");
    (tmp, ctx, id, worktree)
}

fn resubmit(ctx: &Ctx, id: &str, worktree: &Path) {
    std::fs::write(worktree.join("fixed.txt"), "fixed\n").unwrap();
    corral_git::commit_all(worktree, "Fix the review finding").unwrap();
    let state = corral_ops::progress_readback(ctx, id).unwrap();
    let revision = state
        .checkpoints
        .last()
        .map(|c| c.checkpoint.revision)
        .unwrap_or(0);
    corral_ops::save_checkpoint(
        ctx,
        id,
        &corral_core::CheckpointUpdate {
            task: state.task,
            run: state.run,
            spec_revision: state.spec_revision,
            expected_revision: revision,
            progress_revision: 0,
            head: state.head,
            base: state.base,
            tree: state.tree,
            criterion_ids: vec![],
            evidence_refs: vec![],
            next_action: "Review the fixed candidate".into(),
            decisions: vec![],
            waits: vec![],
            resources: vec![],
        },
    )
    .unwrap();
    corral_ops::done(ctx, id, "fixed the widget").unwrap();
    corral_ops::gate_start(ctx, id).unwrap();
    corral_ops::gate_wait(ctx, id).unwrap();
}

#[test]
fn private_review_cycle_retains_distinct_worker_and_reviewer_conversations() {
    // Given a worker and separate reviewer that wrote private fake transcripts.
    let (_tmp, mut ctx, id, worktree) = session_candidate();
    let first = corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();
    let before = ctx.store.get(&id).unwrap();
    let run = before.current_run().unwrap();
    let worker = run.session.clone().unwrap();
    let reviewer = run.review_runs.last().unwrap().session.clone().unwrap();
    assert!(worker.id.is_some() && reviewer.id.is_some());
    assert_ne!(worker.id, reviewer.id);
    let old_callback = corral_ops::review_callback_token(&first).unwrap();
    corral_ops::verdict_bound(
        &ctx,
        &id,
        Verdict::Changes,
        "fix the widget",
        Some(&old_callback),
    )
    .unwrap();

    // When another process requests changes, the worker fixes and resubmits, then review runs again.
    ctx.store = Store::open(ctx.store.root()).unwrap();
    corral_ops::request_changes(&ctx, &id, "fix the widget").unwrap();
    resubmit(&ctx, &id, &worktree);
    let current = corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();

    // Then both conversations survive while all review authority belongs to the new candidate.
    let task = ctx.store.get(&id).unwrap();
    let run = task.current_run().unwrap();
    assert_eq!(run.id, "R1");
    assert_eq!(run.session.as_ref().unwrap().id, worker.id);
    assert_eq!(
        run.session.as_ref().unwrap().action,
        corral_core::SessionAction::Reused
    );
    assert_eq!(
        run.review_runs.last().unwrap().session.as_ref().unwrap().id,
        reviewer.id
    );
    assert_ne!(first.sha, current.sha);
    let rejected =
        corral_ops::verdict_bound(&ctx, &id, Verdict::Approve, "stale", Some(&old_callback));
    assert!(format!("{:#}", rejected.unwrap_err()).contains("stale review"));
    let callback = corral_ops::review_callback_token(&current).unwrap();
    corral_ops::verdict_bound(&ctx, &id, Verdict::Approve, "fixed", Some(&callback)).unwrap();
    println!(
        "private fake cycle {id}: worker={}, reviewer={}, old_attempt={old_callback}, current_attempt={callback}",
        worker.id.unwrap(),
        reviewer.id.unwrap()
    );
}

#[test]
fn exited_worker_resumes_the_recorded_session_instead_of_starting_empty() {
    // Given a finished worker whose process has exited back to its pane shell.
    let (_tmp, mut ctx, id, _) = session_candidate();
    let before = ctx.store.get(&id).unwrap();
    let session = before.current_run().unwrap().session.clone().unwrap();
    let receipt = ctx
        .store
        .state_dir()
        .join("launch-exits")
        .join(format!("{id}-R1.exited"));
    std::fs::write(receipt, "0\n").unwrap();
    ctx.store = Store::open(ctx.store.root()).unwrap();

    // When changes reopen the run.
    corral_ops::request_changes(&ctx, &id, "continue with the finding").unwrap();

    // Then a new process opened the exact same file and appended to its conversation.
    let task = ctx.store.get(&id).unwrap();
    let resumed = task.current_run().unwrap().session.as_ref().unwrap();
    assert_eq!(resumed.id, session.id);
    assert_eq!(resumed.action, corral_core::SessionAction::Resumed);
    assert_eq!(
        std::fs::read_to_string(resumed.transcript.as_ref().unwrap())
            .unwrap()
            .lines()
            .count(),
        3
    );
}

#[test]
fn explicit_fresh_mode_starts_new_conversations_for_each_role() {
    // Given independent fresh policies for both roles.
    let (_tmp, mut ctx, id, worktree) = session_candidate();
    ctx.store.config.review.worker_session = Some(corral_core::SessionPolicy::Fresh);
    ctx.store.config.review.reviewer_session = Some(corral_core::SessionPolicy::Fresh);
    ctx.store.save_config().unwrap();
    let first = corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();
    corral_ops::verdict_bound(
        &ctx,
        &id,
        Verdict::Changes,
        "fix",
        corral_ops::review_callback_token(&first).as_deref(),
    )
    .unwrap();
    let before = ctx.store.get(&id).unwrap();
    let old = before.current_run().unwrap();

    // When the changes cycle starts both roles again.
    corral_ops::request_changes(&ctx, &id, "fresh context").unwrap();
    resubmit(&ctx, &id, &worktree);
    corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();

    // Then neither role claims to have reused its previous conversation.
    let after = ctx.store.get(&id).unwrap();
    let new = after.current_run().unwrap();
    assert_ne!(
        new.session.as_ref().unwrap().id,
        old.session.as_ref().unwrap().id
    );
    assert_ne!(
        new.review_runs.last().unwrap().session.as_ref().unwrap().id,
        old.review_runs.last().unwrap().session.as_ref().unwrap().id
    );
    assert_eq!(
        new.session.as_ref().unwrap().action,
        corral_core::SessionAction::New
    );
    assert_eq!(
        new.review_runs
            .last()
            .unwrap()
            .session
            .as_ref()
            .unwrap()
            .action,
        corral_core::SessionAction::New
    );
}

#[test]
fn missing_worker_transcript_refuses_recovery_without_changing_the_run() {
    // Given a process exit and a lost recorded conversation.
    let (_tmp, ctx, id, _) = session_candidate();
    let before = ctx.store.get(&id).unwrap();
    let run = before.current_run().unwrap();
    std::fs::remove_file(run.session.as_ref().unwrap().transcript.as_ref().unwrap()).unwrap();
    std::fs::write(
        ctx.store
            .state_dir()
            .join("launch-exits")
            .join(format!("{id}-R1.exited")),
        "0\n",
    )
    .unwrap();

    // When changes would need to resume that conversation.
    let error = corral_ops::request_changes(&ctx, &id, "fix").unwrap_err();

    // Then recovery is bounded by a diagnostic, not a silent empty replacement.
    assert!(format!("{error:#}").contains("worker context cannot be resumed"));
    assert_eq!(ctx.store.get(&id).unwrap(), before);
}

#[test]
fn incompatible_reviewer_gets_one_explicit_new_session_and_diagnostic() {
    for adapter_changed in [false, true] {
        // Given a reviewer conversation on the previous model and adapter family.
        let (_tmp, mut ctx, id, _) = task_in_review();
        session_profile(&mut ctx);
        corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();
        if adapter_changed {
            ctx.store.config.harnesses.get_mut("omo").unwrap().flags = Some("omp".into());
        } else {
            ctx.store.config.profiles.get_mut("rev").unwrap().model =
                Some("different-model".into());
        }
        ctx.store.save_config().unwrap();

        // When routing selects an incompatible model or adapter for the next attempt.
        corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();

        // Then one new session explicitly reports context incompatibility.
        let task = ctx.store.get(&id).unwrap();
        let children = &task.current_run().unwrap().review_runs;
        let new = children.last().unwrap().session.as_ref().unwrap();
        assert_eq!(new.action, corral_core::SessionAction::New);
        assert_ne!(new.id, children[0].session.as_ref().unwrap().id);
        assert!(new.diagnostic.as_deref().unwrap().contains("incompatible"));
    }
}
