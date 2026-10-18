use super::*;
use corral_core::{DecisionRecord, Run, Store};

fn fixture() -> (tempfile::TempDir, Ctx, Task) {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let ctx = Ctx {
        store,
        // No executable exists: even a notification would fail if attempted.
        herdr: corral_herdr::Herdr::with_bin(tmp.path().join("no-herdr"), None),
        resource_probe: crate::resources::FixedResourceProbe::arc(),
    };
    let task = ctx
        .store
        .create("Acknowledgement", None, Status::NeedsMe, "")
        .unwrap();
    let run: Run = serde_json::from_value(json!({
        "id":"R1", "agent":"fake", "worktree":tmp.path(), "branch":"main", "base":"main",
        "base_sha":"fixture", "started":"2026-10-15T00:00:00Z",
        "herdr":{"workspace_id":"w1","pane_id":"w1:p1"},
        "pending_reply":"Exact\n answer ", "blocked_excerpt":"Owner question",
        "seen_working":true, "idle_since":"2026-10-15T00:00:00Z", "idle_no_report":true
    }))
    .unwrap();
    let task = ctx
        .store
        .update(&task.id, |task| {
            task.runs.push(run);
            task.rounds = 3;
            Ok(())
        })
        .unwrap();
    (tmp, ctx, task)
}

fn decision(id: &str) -> DecisionRecord {
    serde_json::from_value(json!({
        "request":{"version":1,"id":id,"run":"R1","question":"Which API?",
            "requested_action":"Choose","owner":"owner","source":"worker","deadline":null},
        "registered_at":"2026-10-15T00:00:00Z",
        "observation":{"at":"2026-10-15T00:00:00Z","state":"pending","detail":"Waiting"},
        "receipt":null
    }))
    .unwrap()
}

#[test]
fn acknowledgement_audits_both_authors_and_recovers_without_transport() {
    for author in [CommentAuthor::Owner, CommentAuthor::Pm] {
        // Given a pending live reply with an unavailable transport.
        let (_tmp, ctx, task) = fixture();
        let before = ctx.store.events().unwrap().len();
        // When the operator acknowledges external delivery.
        let saved =
            mark_reply_delivered(&ctx, &task.id, " Saw exact text in session ", author).unwrap();
        // Then only the audited recovery occurs, preserving policy.
        let run = saved.current_run().unwrap();
        assert!(run.pending_reply.is_none());
        assert!(run.blocked_excerpt.is_none());
        assert!(!run.seen_working);
        assert!(run.idle_since.is_none());
        assert!(!run.idle_no_report);
        assert_eq!(saved.status, Status::Running);
        assert_eq!(saved.rounds, 3);
        assert_eq!(run.review, task.current_run().unwrap().review);
        assert_eq!(run.autopilot, task.current_run().unwrap().autopilot);
        let events = ctx.store.events().unwrap();
        assert_eq!(events.len(), before + 1);
        let event = events.last().unwrap();
        assert_eq!(event.kind, "reply_marked_delivered");
        assert_eq!(event.task.as_deref(), Some(task.id.as_str()));
        assert_eq!(
            event.detail,
            json!({
                "task": task.id, "run":"R1","pane":"w1:p1","text":"Exact\n answer ",
                "by":author.as_str(),"reason":" Saw exact text in session ",
                "submitted":true,"attempts":0
            })
        );
    }
}

#[test]
fn empty_reason_is_rejected_without_mutation_or_audit() {
    // Given a pending reply.
    let (_tmp, ctx, task) = fixture();
    let before = ctx.store.events().unwrap();
    // When no useful audit reason is supplied.
    assert!(mark_reply_delivered(&ctx, &task.id, " \n ", CommentAuthor::Owner).is_err());
    // Then the pending value and audit remain unchanged.
    assert_eq!(ctx.store.events().unwrap(), before);
    assert_eq!(ctx.store.get(&task.id).unwrap(), task);
}

#[test]
fn invalid_pending_targets_are_rejected() {
    for case in [
        "missing", "empty", "no_run", "no_pane", "ended", "closed", "review", "ready", "planner",
    ] {
        // Given an ineligible pending target.
        let (_tmp, ctx, task) = fixture();
        let task = ctx
            .store
            .update(&task.id, |task| {
                match case {
                    "no_run" | "planner" => task.runs.clear(),
                    "review" => task.status = Status::Review,
                    "ready" => task.status = Status::Ready,
                    other => {
                        let run = task.current_run_mut().unwrap();
                        match other {
                            "missing" => run.pending_reply = None,
                            "empty" => run.pending_reply = Some("  ".into()),
                            "no_pane" => run.herdr = None,
                            "ended" => run.ended = Some(run.started),
                            "closed" => run.outcome = Some(corral_core::RunOutcome::Superseded),
                            _ => unreachable!(),
                        }
                    }
                }
                Ok(())
            })
            .unwrap();
        let events = ctx.store.events().unwrap();
        // When acknowledgement is requested.
        assert!(
            mark_reply_delivered(&ctx, &task.id, "Verified", CommentAuthor::Owner).is_err(),
            "{case}"
        );
        // Then neither state nor audit changes.
        assert_eq!(ctx.store.get(&task.id).unwrap(), task);
        assert_eq!(ctx.store.events().unwrap(), events);
    }
}

#[test]
fn delivery_lock_excludes_acknowledgement() {
    // Given another operation owns the same delivery lock.
    let (_tmp, ctx, task) = fixture();
    let _lock = ctx
        .store
        .try_lock(&format!("delivery-{}", task.id))
        .unwrap()
        .unwrap();
    // When acknowledging the pending reply.
    let error = mark_reply_delivered(&ctx, &task.id, "Verified", CommentAuthor::Pm).unwrap_err();
    // Then recovery is excluded without consuming the pending reply.
    assert!(error.to_string().contains("another reply"));
    assert_eq!(ctx.store.get(&task.id).unwrap(), task);
}

#[test]
fn guarded_update_rejects_changed_run_pane_or_text() {
    for case in ["run", "pane", "text", "completed"] {
        // Given a snapshot followed by a concurrent worker update.
        let (_tmp, ctx, task) = fixture();
        let current = ctx
            .store
            .update(&task.id, |task| {
                let run = task.current_run_mut().unwrap();
                match case {
                    "run" => run.id = "R2".into(),
                    "pane" => run.herdr.as_mut().unwrap().pane_id = "w1:p2".into(),
                    "text" => run.pending_reply = Some("New answer".into()),
                    "completed" => run.outcome = Some(corral_core::RunOutcome::Done),
                    _ => unreachable!(),
                }
                Ok(())
            })
            .unwrap();
        let events = ctx.store.events().unwrap();
        // When the old acknowledgement reaches its guarded update.
        assert!(acknowledge(&ctx, &task, "Verified", CommentAuthor::Owner, false).is_err());
        // Then no newer state is consumed or falsely audited.
        assert_eq!(ctx.store.get(&task.id).unwrap(), current);
        assert_eq!(ctx.store.events().unwrap(), events);
    }
}

#[test]
fn acknowledgement_answers_decision_without_notifications() {
    // Given an owner question on the current run.
    let (_tmp, ctx, task) = fixture();
    ctx.store
        .update(&task.id, |task| {
            task.current_run_mut()
                .unwrap()
                .obligations
                .decisions
                .push(decision("choose"));
            Ok(())
        })
        .unwrap();
    // When its already delivered answer is acknowledged.
    let saved = mark_reply_delivered(&ctx, &task.id, "Observed", CommentAuthor::Pm).unwrap();
    // Then the decision has the same exact answer and an acknowledged receipt.
    let record = &saved.current_run().unwrap().obligations.decisions[0];
    let receipt = record.receipt.as_ref().unwrap();
    assert_eq!(receipt.run, "R1");
    assert_eq!(receipt.answer.as_deref(), Some("Exact\n answer "));
    assert_eq!(receipt.outcome, ReceiptOutcome::Answered);
    assert_eq!(receipt.delivery, ResumeDelivery::Acknowledged);
    assert_eq!(receipt.data["by"], "pm");
    assert_eq!(receipt.data["reason"], "Observed");
    assert_eq!(saved.status, Status::Running);
}

#[test]
fn checkpoint_acknowledgement_does_not_answer_owner_question() {
    // Given pending checkpoint control text and an unrelated owner question.
    let (_tmp, ctx, task) = fixture();
    ctx.store
        .update(&task.id, |task| {
            task.current_run_mut()
                .unwrap()
                .obligations
                .decisions
                .push(decision("choose"));
            Ok(())
        })
        .unwrap();
    ctx.store
        .log_event(
            Some(&task.id),
            "checkpoint_requested",
            json!({
                "run":"R1","pane":"w1:p1","text":"Exact\n answer ","submitted":false
            }),
        )
        .unwrap();
    // When external delivery of the control request is acknowledged.
    let saved = mark_reply_delivered(&ctx, &task.id, "Observed", CommentAuthor::Owner).unwrap();
    // Then the retry pending blocker clears, but the owner question remains.
    let run = saved.current_run().unwrap();
    assert!(run.pending_reply.is_none());
    assert!(run.obligations.decisions[0].receipt.is_none());
    assert_eq!(run.blocked_excerpt.as_deref(), Some("Owner question"));
    assert_eq!(saved.status, Status::NeedsMe);
}
