use super::*;
use crate::{plan_spec_revision, progress_spec_revision, verification};
use corral_core::{GateRecord, GateResult, HerdrRef, ReviewRecord, Run, Store};
use serde_json::json;

#[test]
fn custom_dispatch_and_review_templates_keep_exact_comment_payloads() {
    // Given custom templates without comment placeholders and a note containing template syntax.
    let (_tmp, ctx, task) = fixture();
    std::fs::write(ctx.store.prompts_dir().join("run.md"), "{{title}}").unwrap();
    std::fs::write(ctx.store.prompts_dir().join("review.md"), "{{title}}").unwrap();
    let payload = "Decision 046: preserve {{title}} literally.\nhttps://example.invalid/decision";
    let task = comment(&ctx, &task.id, payload, CommentAuthor::Pm, false).unwrap();
    let run = task.current_run().unwrap();

    // When either agent-facing prompt is rendered.
    let prompts = [
        crate::task_prompt(&ctx, &task, run).unwrap(),
        crate::review_prompt(&ctx, &task, run).unwrap(),
    ];

    // Then the stored owner-supplied payload and its durable source ship without expansion.
    for prompt in prompts {
        assert!(prompt.contains(payload));
        assert!(prompt.contains(task.path.to_str().unwrap()));
    }
}

fn fixture() -> (tempfile::TempDir, Ctx, Task) {
    let tmp = tempfile::tempdir().unwrap();
    let app = tmp.path().join("app");
    std::fs::create_dir(&app).unwrap();
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    ] {
        let out = crate::test_support::command("git", tmp.path())
            .arg("-C")
            .arg(&app)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let store = Store::init(tmp.path()).unwrap();
    let ctx = Ctx {
        resource_probe: crate::resources::FixedResourceProbe::arc(),
        herdr: corral_herdr::Herdr::with_bin(tmp.path().join("herdr"), Some("corral-test".into())),
        store,
    };
    let task = ctx
        .store
        .create("Comments", Some("app"), Status::Running, "Spec.\n")
        .unwrap();
    let run: Run = serde_json::from_value(json!({
        "id":"R1", "agent":"fake", "worktree":app, "branch":"main", "base":"main",
        "base_sha":corral_git::rev_sha(&app, "HEAD").unwrap(),
        "started":"2026-10-15T00:00:00Z",
        "herdr":{"workspace_id":"w1","pane_id":"w1:p1"}
    }))
    .unwrap();
    let task = ctx
        .store
        .update(&task.id, |task| {
            task.runs.push(run);
            Ok(())
        })
        .unwrap();
    (tmp, ctx, task)
}

#[test]
fn adding_comments_preserves_spec_candidate_gate_and_review_identities() {
    // Given a candidate with a gate and review bound to its current inputs.
    let (_tmp, ctx, task) = fixture();
    let run = task.current_run().unwrap();
    let identity = verification::capture_identity(&ctx, &task, run, "attempt-1").unwrap();
    let spec = progress_spec_revision(&task).unwrap();
    let plan_spec = plan_spec_revision(&task).unwrap();
    let gate = GateRecord {
        socket: None,
        identity: Some(identity.clone()),
        commands: vec![],
        log_sha256: None,
        result: GateResult::Pass,
        process_lost: false,
        failed_cmd: None,
        at: run.started,
        sha: identity.head.clone(),
        pane: None,
        tier: None,
    };
    let review = ReviewRecord {
        route: None,
        identity: Some(identity.clone()),
        evidence_digest: Some(verification::evidence_digest(run).unwrap()),
        verdict: Some(corral_core::Verdict::Approve),
        process_lost: false,
        reviewer: "owner".into(),
        notes: None,
        sha: identity.head.clone(),
        at: run.started,
        pane: None,
    };
    let task = ctx
        .store
        .update(&task.id, |task| {
            let run = task.current_run_mut().unwrap();
            run.gate = Some(gate.clone());
            run.review = Some(review.clone());
            Ok(())
        })
        .unwrap();

    // When owner context is appended through the public write path.
    let saved = comment(
        &ctx,
        &task.id,
        "Keep the public interface.",
        CommentAuthor::Owner,
        false,
    )
    .unwrap();

    // Then no contract identity or existing gate/review becomes stale.
    let run = saved.current_run().unwrap();
    assert_eq!(progress_spec_revision(&saved).unwrap(), spec);
    assert_eq!(plan_spec_revision(&saved).unwrap(), plan_spec);
    assert_eq!(
        verification::identity(&ctx, &saved, run, &identity).unwrap(),
        identity
    );
    assert!(verification::gate_fresh(&ctx, &saved, run, &gate).unwrap());
    assert!(verification::review_fresh(&ctx, &saved, run).unwrap());
    assert_eq!(saved.body, task.body);
    assert_eq!(saved.runs, task.runs);
}

#[test]
fn comments_round_trip_with_authors_and_events_in_append_order() {
    // Given a task with an owner note.
    let (tmp, ctx, task) = fixture();
    comment(
        &ctx,
        &task.id,
        "First\nhttps://example.invalid",
        CommentAuthor::Owner,
        false,
    )
    .unwrap();
    // When a PM note is appended and the store is reopened.
    comment(&ctx, &task.id, "Second", CommentAuthor::Pm, false).unwrap();
    let store = Store::open(tmp.path()).unwrap();
    let saved = store.get(&task.id).unwrap();
    // Then the thread and its events preserve exact text, attribution and timestamps.
    assert_eq!(saved.comments.len(), 2);
    assert_eq!(saved.comments[0].text, "First\nhttps://example.invalid");
    assert_eq!(saved.comments[0].author, CommentAuthor::Owner);
    assert_eq!(saved.comments[1].author, CommentAuthor::Pm);
    let events: Vec<_> = store
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "commented")
        .collect();
    for (note, event) in saved.comments.iter().zip(&events) {
        assert_eq!(event.task.as_deref(), Some(task.id.as_str()));
        assert_eq!(event.detail["comment"], serde_json::to_value(note).unwrap());
        assert_eq!(event.detail["send"], false);
    }
    assert_eq!(events.len(), 2);
}

#[cfg(unix)]
fn transport(tmp: &tempfile::TempDir) {
    std::os::unix::fs::symlink(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/progress/herdr.sh"),
        tmp.path().join("herdr"),
    )
    .unwrap();
}

#[test]
#[cfg(unix)]
fn send_uses_native_reply_delivery_with_durable_task_reference() {
    // Given an isolated transport fixture and a running agent.
    let (tmp, ctx, task) = fixture();
    transport(&tmp);
    // When the owner saves and sends a comment.
    let saved = comment(
        &ctx,
        &task.id,
        "Use the stable API.",
        CommentAuthor::Owner,
        true,
    )
    .unwrap();
    // Then the existing reply path submits the exact note and source reference.
    let sent = std::fs::read_to_string(tmp.path().join("request.txt")).unwrap();
    assert!(sent.contains(task.path.to_str().unwrap()));
    assert!(sent.contains(&saved.comments[0].text));
    assert!(saved.current_run().unwrap().pending_reply.is_none());
    let receipt = ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .find(|e| e.kind == "replied")
        .unwrap();
    assert_eq!(receipt.detail["text"], sent);
    assert_eq!(receipt.detail["submitted"], true);
    assert_eq!(receipt.detail["via"], "agent_prompt");
}

#[test]
#[cfg(unix)]
fn failed_send_preserves_comment_and_exact_pending_note_for_resend() {
    // Given a transport that refuses submission.
    let (tmp, ctx, task) = fixture();
    transport(&tmp);
    std::fs::write(tmp.path().join("reject"), "").unwrap();
    // When save-and-send fails at delivery.
    assert!(
        comment(
            &ctx,
            &task.id,
            "Retain this decision.",
            CommentAuthor::Pm,
            true
        )
        .is_err()
    );
    // Then both the durable comment and resendable exact delivery survive.
    let saved = ctx.store.get(&task.id).unwrap();
    assert_eq!(saved.comments.len(), 1);
    let pending = saved.current_run().unwrap().pending_reply.as_ref().unwrap();
    assert_eq!(
        *pending,
        std::fs::read_to_string(tmp.path().join("request.txt")).unwrap()
    );
    std::fs::remove_file(tmp.path().join("reject")).unwrap();
    let resent = crate::resend_reply(&ctx, &task.id).unwrap();
    assert!(resent.current_run().unwrap().pending_reply.is_none());
    assert_eq!(resent.comments, saved.comments);
}

#[test]
fn send_without_running_agent_is_rejected_without_appending() {
    // Given a Ready task without an agent.
    let (_tmp, ctx, task) = fixture();
    ctx.store
        .update(&task.id, |task| {
            task.status = Status::Ready;
            task.runs.clear();
            Ok(())
        })
        .unwrap();
    // When sending is requested.
    assert!(comment(&ctx, &task.id, "Later", CommentAuthor::Owner, true).is_err());
    // Then a rejected send does not add an ambiguous saved note.
    assert!(ctx.store.get(&task.id).unwrap().comments.is_empty());
}

#[test]
fn stuck_agent_queues_comment_through_existing_reply_path() {
    // Given a stuck worker whose transport must not be contacted.
    let (_tmp, ctx, task) = fixture();
    ctx.store
        .update(&task.id, |task| {
            task.current_run_mut().unwrap().stuck = Some(corral_core::StuckProcess {
                pid: 123,
                argv: vec!["fixture".into()],
                started: "1".into(),
            });
            Ok(())
        })
        .unwrap();
    // When a comment is sent.
    let saved = comment(
        &ctx,
        &task.id,
        "Continue after recovery",
        CommentAuthor::Owner,
        true,
    )
    .unwrap();
    // Then the durable reply is queued rather than lost or sent to a foreign process.
    assert!(saved.current_run().unwrap().pending_reply.is_some());
    assert_eq!(saved.comments.len(), 1);
    let receipt = ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .find(|e| e.kind == "reply_queued")
        .unwrap();
    assert_eq!(receipt.detail["submitted"], false);
    assert_eq!(
        saved.current_run().unwrap().herdr,
        Some(HerdrRef {
            workspace_id: "w1".into(),
            pane_id: "w1:p1".into(),
            socket: None,
        })
    );
}
