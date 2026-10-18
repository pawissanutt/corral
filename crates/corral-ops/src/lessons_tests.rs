use super::*;
use crate::{LessonProposal, LessonRequest};
use corral_core::{LessonEvidence, LessonScope, Status, Store};

fn workspace() -> (tempfile::TempDir, Ctx) {
    let tmp = tempfile::tempdir().unwrap();
    let app = tmp.path().join("app");
    std::fs::create_dir_all(app.join(".git")).unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.lessons.profile = Some("generic".into());
    let ctx = Ctx::from_store(store);
    (tmp, ctx)
}

fn lesson(id: &str) -> Lesson {
    Lesson {
        id: id.into(),
        rule: format!("Rule {id}"),
        why: "Observed history".into(),
        evidence: vec![LessonEvidence {
            task: "T-001".into(),
            ts: "2026-10-15T00:00:00Z".parse().unwrap(),
            source: "event:changes_requested".into(),
        }],
        scope: LessonScope::default(),
        confidence: 0.8,
        pinned: false,
        contradicted: false,
        last_seen: "2026-10-15T00:00:00Z".parse().unwrap(),
    }
}

fn run(ctx: &Ctx) -> Run {
    serde_json::from_value(json!({
        "id":"R1","agent":"generic","harness":"generic","worktree":ctx.store.root().join("app"),
        "branch":"corral/T-001","base":"main","base_sha":"0".repeat(40),
        "started":"2026-10-15T00:00:00Z"
    }))
    .unwrap()
}

fn history(ctx: &Ctx) -> Task {
    let task = ctx
        .store
        .create("Fix startup", Some("app"), Status::Done, "")
        .unwrap();
    ctx.store
        .log_event(
            Some(&task.id),
            "changes_requested",
            json!({
                "text":"test startup_readiness ... FAILED","run":"R1"
            }),
        )
        .unwrap();
    task
}

fn proposal(ctx: &Ctx, task: &Task) -> LessonProposal {
    let request: LessonRequest = serde_json::from_str(
        &std::fs::read_to_string(
            ctx.store
                .state_dir()
                .join("lessons")
                .join(format!("{}.json", task.id)),
        )
        .unwrap(),
    )
    .unwrap();
    let evidence = request.signals[0].evidence[0].clone();
    let mut rule = lesson("L-startup");
    rule.last_seen = evidence.ts;
    rule.evidence = vec![evidence];
    LessonProposal {
        version: 1,
        project: "app".into(),
        task: task.id.clone(),
        source_digest: request.source_digest,
        lessons: vec![rule],
    }
}

#[test]
fn dispatch_snapshot_survives_owner_edits_and_reviewer_uses_same_data() {
    // Given a project lesson and a ready task.
    let (_tmp, ctx) = workspace();
    let original = lesson("L-original");
    ctx.store
        .save_lessons("app", std::slice::from_ref(&original))
        .unwrap();
    let task = ctx
        .store
        .create("Work", Some("app"), Status::Ready, "")
        .unwrap();
    let run = run(&ctx);
    // When launch records its exposure, then the owner replaces the rule.
    record_lessons(&ctx, &task, &run).unwrap();
    record_exposure(&ctx, &task, &run).unwrap();
    ctx.store.save_lessons("app", &[lesson("L-new")]).unwrap();
    record_lessons(&ctx, &task, &run).unwrap();
    // Then both generated prompts carry identical machine-readable historical data.
    assert_eq!(
        applied_lessons(&ctx, &task, &run).unwrap(),
        Some(vec![original])
    );
    let dispatch = crate::task_prompt(&ctx, &task, &run).unwrap();
    let review = crate::review_prompt(&ctx, &task, &run).unwrap();
    let parse = |prompt: &str| -> Vec<Lesson> {
        serde_json::from_str(
            prompt
                .split("<corral-lessons>\n")
                .nth(1)
                .unwrap()
                .split("\n</corral-lessons>")
                .next()
                .unwrap(),
        )
        .unwrap()
    };
    assert_eq!(parse(&dispatch), parse(&review));
    assert_eq!(
        ctx.store
            .events()
            .unwrap()
            .iter()
            .filter(|e| e.kind == "lessons_applied")
            .count(),
        1
    );
}

#[test]
fn preview_does_not_record_dispatch_exposure() {
    // Given a task with no applicable lessons.
    let (_tmp, ctx) = workspace();
    let task = ctx
        .store
        .create("Work", Some("app"), Status::Ready, "")
        .unwrap();
    let run = run(&ctx);
    // When a prompt is previewed.
    crate::task_prompt(&ctx, &task, &run).unwrap();
    // Then legacy/unknown exposure remains distinguishable from an empty recorded snapshot.
    assert_eq!(applied_lessons(&ctx, &task, &run).unwrap(), None);
}

#[test]
fn bounded_spike_is_filed_once_without_dispatching() {
    // Given history and an explicit strong profile.
    let (_tmp, ctx) = workspace();
    history(&ctx);
    // When the owner or PM requests the same pending distillation again.
    let first = crate::lessons_distill(&ctx, "app", None).unwrap();
    let second = crate::lessons_distill(&ctx, "app", None).unwrap();
    // Then one typed backlog spike exists, with its request artifact and no run.
    assert_eq!(first.id, second.id);
    assert_eq!(first.kind.as_deref(), Some("spike"));
    assert_eq!(first.profile.as_deref(), Some("generic"));
    assert_eq!(first.status, Status::Backlog);
    assert!(first.runs.is_empty());
    assert_eq!(first.artifacts.len(), 1);
}

#[test]
fn evidence_bound_proposal_updates_lessons_and_rejects_replay() {
    // Given a pending distillation and a proposal referencing its exact evidence.
    let (_tmp, ctx) = workspace();
    history(&ctx);
    let task = crate::lessons_distill(&ctx, "app", None).unwrap();
    let proposal = proposal(&ctx, &task);
    // When the model output is applied.
    crate::lessons_apply(&ctx, "app", &proposal).unwrap();
    // Then durable rules match the proposal and the same request cannot apply twice.
    assert_eq!(ctx.store.lessons("app").unwrap(), proposal.lessons);
    assert!(crate::lessons_apply(&ctx, "app", &proposal).is_err());
}

#[test]
fn fabricated_provenance_is_rejected_without_writing() {
    // Given a model response with an invented source.
    let (_tmp, ctx) = workspace();
    history(&ctx);
    let task = crate::lessons_distill(&ctx, "app", None).unwrap();
    let mut proposal = proposal(&ctx, &task);
    proposal.lessons[0].evidence[0].source = "invented".into();
    // When apply validates provenance.
    let result = crate::lessons_apply(&ctx, "app", &proposal);
    // Then no lesson is persisted.
    assert!(result.is_err());
    assert!(ctx.store.lessons("app").unwrap().is_empty());
}

#[test]
fn stale_model_output_cannot_overwrite_owner_edit() {
    // Given a request followed by an owner-authored lesson.
    let (_tmp, ctx) = workspace();
    history(&ctx);
    let task = crate::lessons_distill(&ctx, "app", None).unwrap();
    let proposal = proposal(&ctx, &task);
    let owner = lesson("L-owner");
    ctx.store
        .save_lessons("app", std::slice::from_ref(&owner))
        .unwrap();
    // When stale output arrives.
    let result = crate::lessons_apply(&ctx, "app", &proposal);
    // Then the owner edit survives.
    assert!(result.is_err());
    assert_eq!(ctx.store.lessons("app").unwrap(), vec![owner]);
}

#[test]
fn count_schedules_follow_request_order_when_timestamps_tie_or_reverse() {
    for (history_at, later_at) in [
        ("2026-10-17T12:00:00Z", "2026-10-17T10:00:00Z"),
        ("2026-10-17T11:00:00Z", "2026-10-17T11:00:00Z"),
    ] {
        for signal in ["gate", "approved"] {
            for new_signals in [0, 1] {
                // Given an append-ordered request between historical and new signals.
                let (_tmp, mut ctx) = workspace();
                ctx.store.config.lessons.after_failures = Some(1);
                ctx.store.config.lessons.after_landings = Some(1);
                let historical = history(&ctx);
                let pending = ctx
                    .store
                    .create("Distill", Some("app"), Status::Backlog, "")
                    .unwrap();
                let event = |kind: &str, task: &str, ts: &str, detail| corral_core::Event {
                    ts: ts.parse().unwrap(),
                    task: Some(task.into()),
                    kind: kind.into(),
                    actor: None,
                    via: None,
                    detail,
                };
                let detail = json!({"result":"fail","strategy":"fast_forward"});
                let mut events = vec![
                    event(signal, &historical.id, history_at, detail.clone()),
                    event(
                        "lessons_distill_requested",
                        &pending.id,
                        "2026-10-17T11:00:00Z",
                        json!({"project":"app"}),
                    ),
                ];
                for _ in 0..new_signals {
                    events.push(event(signal, &historical.id, later_at, detail.clone()));
                }
                let lines = events
                    .iter()
                    .map(|event| serde_json::to_string(event).unwrap())
                    .collect::<Vec<_>>()
                    .join("\n");
                std::fs::write(ctx.store.dir().join("events.jsonl"), format!("{lines}\n")).unwrap();

                // When count triggers are evaluated independently of wall-clock order.
                let due =
                    crate::lessons_due(&ctx, "2026-10-17T11:00:00Z".parse().unwrap()).unwrap();

                // Then only signals appended after the request spend the new threshold.
                assert_eq!(due.len(), 1);
                assert_eq!(due[0].pending.as_deref(), Some(pending.id.as_str()));
                assert_eq!(
                    due[0].reasons.len(),
                    new_signals,
                    "{signal}, history {history_at}, later {later_at}"
                );
            }
        }
    }
}

#[test]
fn failure_schedule_is_read_only_and_requests_a_separate_spike() {
    // Given the failure threshold is reached, with no other actionable task.
    let (_tmp, mut ctx) = workspace();
    ctx.store.config.lessons.after_failures = Some(1);
    history(&ctx);
    let before = ctx.store.events().unwrap();
    // When PM evaluates the board.
    let action = crate::pm_next(&ctx).unwrap();
    // Then it recommends filing a spike and writes no event or task.
    assert_eq!(action.kind, crate::PmActionKind::LessonsDistill);
    assert!(!action.requires_owner);
    assert_eq!(ctx.store.events().unwrap(), before);
    assert_eq!(ctx.store.list().unwrap().len(), 1);
}

#[test]
fn interval_and_landing_schedule_use_request_baseline() {
    // Given an interval and landing threshold with completed history.
    let (_tmp, mut ctx) = workspace();
    let task = history(&ctx);
    ctx.store.config.lessons.every = Some("1d".into());
    ctx.store.config.lessons.after_landings = Some(1);
    ctx.store
        .log_event(
            Some(&task.id),
            "approved",
            json!({"run":"R1","strategy":"rebase"}),
        )
        .unwrap();
    let now = Timestamp::now();
    // When the scheduler observes history without an earlier request.
    let due = crate::lessons_due(&ctx, now).unwrap();
    // Then both triggers are reported deterministically.
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].reasons.len(), 2);
    assert!(due[0].pending.is_none());
}

#[test]
fn similar_history_supplies_files_missing_from_new_task_spec() {
    // Given a completed similar task with a retained diff and a file-scoped lesson.
    let (tmp, ctx) = workspace();
    let old = ctx
        .store
        .create("Fix startup readiness", Some("app"), Status::Done, "")
        .unwrap();
    let diff = tmp.path().join("landed.diff");
    std::fs::write(&diff, "--- a/src/startup.rs\n+++ b/src/startup.rs\n").unwrap();
    crate::add_artifact(
        &ctx,
        &old.id,
        &diff,
        Some(corral_core::ArtifactKind::Diff),
        None,
    )
    .unwrap();
    let mut rule = lesson("L-history");
    rule.scope.files = vec!["src/startup.rs".into()];
    ctx.store.save_lessons("app", &[rule]).unwrap();
    let task = ctx
        .store
        .create("Improve startup readiness", Some("app"), Status::Ready, "")
        .unwrap();
    // When matching a new task whose title names the area but no file.
    let matched = lessons_for(&ctx, &task, None).unwrap();
    // Then retained historical paths provide the file match.
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].id, "L-history");
}

#[test]
fn sync_schedule_announces_once_and_rearms_after_the_next_interval() {
    // Given completed history and an interval schedule without a pending spike.
    let (_tmp, mut ctx) = workspace();
    let task = history(&ctx);
    ctx.store.config.lessons.every = Some("1d".into());
    crate::lesson_distill::announce_due(&ctx, Timestamp::now()).unwrap();
    crate::lesson_distill::announce_due(&ctx, Timestamp::now()).unwrap();
    let announcements = || {
        ctx.store
            .events()
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "lessons_due")
            .collect::<Vec<_>>()
    };
    assert_eq!(announcements().len(), 1);
    assert_eq!(
        corral_core::actionable_kind(&announcements()[0]),
        Some("lessons_due")
    );
    ctx.store
        .log_event(
            Some(&task.id),
            "lessons_distill_requested",
            json!({"project":"app"}),
        )
        .unwrap();
    let baseline = ctx.store.events().unwrap().last().unwrap().ts;
    // When the clock advances across the exact next interval boundary.
    crate::lesson_distill::announce_due(
        &ctx,
        baseline
            .checked_add(jiff::SignedDuration::from_hours(23))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(announcements().len(), 1);
    crate::lesson_distill::announce_due(
        &ctx,
        baseline
            .checked_add(jiff::SignedDuration::from_hours(24))
            .unwrap(),
    )
    .unwrap();
    // Then one new generation is announced, with no dispatched task.
    assert_eq!(announcements().len(), 2);
    assert_eq!(ctx.store.list().unwrap().len(), 1);
}

#[test]
fn unrelated_completed_tasks_cannot_supply_likely_files_through_shared_clusters() {
    // Given unrelated completed tasks whose paths share the success:done cluster.
    let (tmp, ctx) = workspace();
    for (title, area) in [
        ("Repair alpha engine", "alpha"),
        ("Repair beta cache", "beta"),
    ] {
        let task = ctx
            .store
            .create(title, Some("app"), Status::Done, "")
            .unwrap();
        let diff = tmp.path().join(format!("{area}.diff"));
        std::fs::write(&diff, format!("--- a/src/{area}.rs\n+++ b/src/{area}.rs\n")).unwrap();
        crate::add_artifact(
            &ctx,
            &task.id,
            &diff,
            Some(corral_core::ArtifactKind::Diff),
            None,
        )
        .unwrap();
    }
    let signals = crate::lesson_signals(&ctx, "app").unwrap();
    let shared = signals.iter().find(|s| s.key == "success:done").unwrap();
    assert_eq!(shared.files, ["src/alpha.rs", "src/beta.rs"]);
    let mut beta = lesson("L-beta");
    beta.scope.files = vec!["src/beta.rs".into()];
    ctx.store.save_lessons("app", &[beta]).unwrap();
    let task = ctx
        .store
        .create("Improve alpha engine", Some("app"), Status::Ready, "")
        .unwrap();
    // When a task is similar only to the alpha task.
    let matched = lessons_for(&ctx, &task, None).unwrap();
    // Then the unrelated beta path cannot select its lesson.
    assert!(matched.is_empty(), "{matched:?}");
}

#[test]
fn landing_schedule_excludes_unmerged_and_unproven_approvals() {
    for strategy in [Some("keep_branch"), None, Some("unknown")] {
        // Given only an approval that does not prove a supported landing.
        let (_tmp, mut ctx) = workspace();
        let task = history(&ctx);
        ctx.store.config.lessons.after_landings = Some(1);
        let mut detail = json!({"run":"R1"});
        if let Some(strategy) = strategy {
            detail["strategy"] = strategy.into();
        }
        ctx.store
            .log_event(Some(&task.id), "approved", detail)
            .unwrap();
        // When only the landing-count schedule is evaluated.
        let due = crate::lessons_due(&ctx, Timestamp::now()).unwrap();
        // Then keep-branch, legacy missing-strategy and unknown approvals do not spend it.
        assert!(due.is_empty(), "strategy {strategy:?}: {due:?}");
    }
}

#[test]
fn landing_schedule_counts_fast_forward_and_rebase_approvals() {
    for (strategy, landings, expected) in [
        ("fast_forward", 1, 0),
        ("fast_forward", 2, 1),
        ("rebase", 1, 0),
        ("rebase", 2, 1),
    ] {
        // Given recorded landings and a threshold of two.
        let (_tmp, mut ctx) = workspace();
        let task = history(&ctx);
        ctx.store.config.lessons.after_landings = Some(2);
        for _ in 0..landings {
            ctx.store
                .log_event(
                    Some(&task.id),
                    "approved",
                    json!({"run":"R1","strategy":strategy}),
                )
                .unwrap();
        }
        // When the landing threshold is evaluated.
        let due = crate::lessons_due(&ctx, Timestamp::now()).unwrap();
        // Then both strategies trigger only once the threshold is met.
        assert_eq!(
            due.len(),
            expected,
            "strategy {strategy}, landings {landings}"
        );
    }
}

#[test]
fn failure_schedule_counts_only_escalated_autopilot_transitions() {
    for (transition, expected) in [("escalated", 1), ("reviewing", 0), ("gating", 0)] {
        // Given only one producer-shaped autopilot transition and a failure threshold.
        let (_tmp, mut ctx) = workspace();
        ctx.store.config.lessons.after_failures = Some(1);
        let task = ctx
            .store
            .create("Review transition", Some("app"), Status::Done, "")
            .unwrap();
        ctx.store
            .log_event(
                Some(&task.id),
                "autopilot",
                json!({
                    "run":"R1","from":"reviewing","to":transition,"round":0,
                    "max_rounds":2,"sha":null,
                    "reason":"the reviewer closed without a verdict","note":"handed to the owner"
                }),
            )
            .unwrap();
        // When evaluating the configured failure schedule.
        let due = crate::lessons_due(&ctx, Timestamp::now()).unwrap();
        // Then actual escalations count, but ordinary autopilot progress does not.
        assert_eq!(due.len(), expected, "transition {transition}");
    }
}
