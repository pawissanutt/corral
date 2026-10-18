use corral_core::{Artifact, ArtifactKind, Event, Origin, PinRef, Run, Status, Store, Task};
use corral_ops::{Ctx, collect_sprint_review, review_sprint};
use jiff::Timestamp;
use serde_json::{Value, json};

fn ts(value: &str) -> Timestamp {
    value.parse().unwrap()
}

fn fixture() -> (tempfile::TempDir, Ctx) {
    let temp = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(Store::init(temp.path()).unwrap());
    (temp, ctx)
}

fn run(id: &str) -> Run {
    serde_json::from_value(json!({
        "id":id,"agent":"worker","profile":"fast",
        "worktree":"/must-not-read","branch":"unused","base":"main","base_sha":"base",
        "started":"2026-01-01T00:00:00Z",
        "ended":"2026-01-01T00:10:00Z",
        "summary":"summary-fixture",
        "review":{"reviewer":"pm","sha":"abc","at":"2026-01-01T00:09:00Z","verdict":"approve"},
        "usage":{"input":10,"output":5,"cache_read":0,"cache_write":0,"reported_usd":1.25}
    }))
    .unwrap()
}

fn event(task: &Task, at: &str, kind: &str, detail: Value) -> Event {
    Event {
        ts: ts(at),
        task: Some(task.id.clone()),
        kind: kind.into(),
        actor: None,
        via: None,
        detail,
    }
}

fn write_events(ctx: &Ctx, events: &[Event]) {
    let lines: String = events
        .iter()
        .map(|event| format!("{}\n", serde_json::to_string(event).unwrap()))
        .collect();
    std::fs::write(ctx.store.dir().join("events.jsonl"), lines).unwrap();
}

#[test]
fn collects_all_sections_from_recorded_evidence() {
    // Given saved execution evidence, an owner reduction, and a pending owner question.
    let (_temp, ctx) = fixture();
    let mut landed = ctx.store.create("landed", None, Status::Done, "").unwrap();
    landed.runs.push(run("R1"));
    landed.artifacts.push(
        serde_json::from_value::<Artifact>(json!({
            "run":"R1","kind":"screenshot","title":"demo","path":"artifacts/demo.png",
            "source":"/must-not-read/demo.png","sha256":"abc","bytes":10,
            "added":"2026-01-01T00:08:00Z"
        }))
        .unwrap(),
    );
    let mut owner = ctx
        .store
        .create("owner", None, Status::Dropped, "")
        .unwrap();
    owner.origin = Some(Origin::Owner);
    let mut active = ctx
        .store
        .create("active", None, Status::NeedsMe, "")
        .unwrap();
    let mut active_run = run("R1");
    active_run.ended = None;
    active_run.usage = None;
    active_run.progress = Some(
        serde_json::from_value(json!({
            "revision":1,"spec_revision":"spec","phases":[]
        }))
        .unwrap(),
    );
    active_run.agent_state = Some("blocked".into());
    active_run.obligations = serde_json::from_value(json!({
        "decisions":[{
            "request":{"version":1,"id":"choice","run":"R1","question":"question-fixture",
                "requested_action":"action-fixture","owner":"owner","source":"worker","deadline":null},
            "registered_at":"2026-01-01T00:01:00Z",
            "observation":{"at":"2026-01-01T00:01:00Z","state":"pending","detail":""},
            "receipt":null
        }]
    })).unwrap();
    active.runs.push(active_run);
    let events = vec![
        event(
            &landed,
            "2026-01-01T00:02:00Z",
            "gate",
            json!({"run":"R1","result":"fail","failed_cmd":"cmd-fixture"}),
        ),
        event(
            &landed,
            "2026-01-01T00:03:00Z",
            "changes_requested",
            json!({"text":"reason-fixture"}),
        ),
        event(
            &owner,
            "2026-01-01T00:04:00Z",
            "idea_decided",
            json!({"action":"decline","reason":"reason-fixture","actor":"pm","owner_ok":false}),
        ),
        event(
            &active,
            "2026-01-01T00:05:00Z",
            "pm_recommendation",
            json!({"recommendation":"recommendation-fixture"}),
        ),
        event(
            &landed,
            "2026-01-01T00:06:00Z",
            "lessons_applied",
            json!({"run":"R1","lessons":[{"id":"L1"}]}),
        ),
        event(
            &landed,
            "2026-01-01T00:10:00Z",
            "approved",
            json!({"run":"R1","sha":"abc","strategy":"fast_forward"}),
        ),
    ];
    // When collecting at a fixed snapshot without touching provider or artifact source paths.
    let sections = collect_sprint_review(
        &[landed.clone(), owner.clone(), active.clone()],
        &events,
        None,
        ts("2026-01-01T01:00:00Z"),
    );
    // Then all seven sections expose structured evidence, not prose-derived assertions.
    let row = &sections.landed[0];
    assert_eq!(row.task.as_deref(), Some(landed.id.as_str()));
    assert_eq!(row.summary.as_deref(), landed.runs[0].summary.as_deref());
    assert_eq!(row.sha.as_deref(), Some("abc"));
    assert_eq!(row.verdict.as_deref(), Some("approve"));
    assert_eq!(
        row.demos,
        vec![PinRef::of_artifact(&landed.id, &landed.artifacts[0]).to_string()]
    );
    assert_eq!(sections.sent_back_failed.len(), 2);
    assert_eq!(
        sections.sent_back_failed[1].resolution.as_deref(),
        Some("approved")
    );
    let reduction = &sections.dropped_rejected_escalated[0];
    assert_eq!(reduction.owner_ok, Some(false));
    assert!(reduction.needs_owner_ok);
    assert_eq!(reduction.actor.as_deref(), Some("pm"));
    assert_eq!(sections.in_flight[0].stage.as_deref(), Some("blocked"));
    assert_eq!(sections.in_flight[0].progress, active.runs[0].progress);
    assert_eq!(sections.owner_ideas[0].status, Status::Dropped);
    assert_eq!(
        sections.decisions[0].question,
        active.runs[0].obligations.decisions[0].request.question
    );
    assert_eq!(
        sections.decisions[0].pm_recommendation.as_deref(),
        events[3].detail["recommendation"].as_str()
    );
    assert_eq!(sections.numbers.review_rounds, 2);
    assert_eq!(sections.numbers.gate_failures, 1);
    assert_eq!(sections.numbers.lessons, vec![events[4].clone()]);
    assert_eq!(
        sections.numbers.by_task[&landed.id].reported_usd,
        Some(1.25)
    );
    assert_eq!(sections.numbers.by_profile["fast"].wall_seconds, Some(600));
    assert_eq!(sections.numbers.by_task[&active.id].reported_usd, None);
    assert_eq!(sections.numbers.by_task[&active.id].unknown_cost_runs, 1);
}

#[test]
fn strict_window_excludes_exact_lower_boundary_and_includes_snapshot() {
    // Given history straddling both boundaries and snapshots without recent events.
    let (_temp, ctx) = fixture();
    let mut owner = ctx.store.create("owner", None, Status::Done, "").unwrap();
    owner.origin = Some(Origin::Owner);
    owner.runs.push(run("R1"));
    let ready = ctx.store.create("ready", None, Status::Ready, "").unwrap();
    let lower = ts("2026-01-01T00:10:00Z");
    let upper = ts("2026-01-01T00:20:00Z");
    let events = vec![
        event(&owner, "2026-01-01T00:09:00Z", "failed", json!({})),
        event(
            &owner,
            "2026-01-01T00:10:00Z",
            "approved",
            json!({"sha":"old"}),
        ),
        event(
            &owner,
            "2026-01-01T00:10:00Z",
            "idea_decided",
            json!({"action":"drop"}),
        ),
        event(
            &owner,
            "2026-01-01T00:10:00Z",
            "gate",
            json!({"result":"fail"}),
        ),
        event(&owner, "2026-01-01T00:10:00Z", "lessons_applied", json!({})),
        event(
            &owner,
            "2026-01-01T00:20:00Z",
            "approved",
            json!({"sha":"new"}),
        ),
        event(
            &owner,
            "2026-01-01T00:20:00.000000001Z",
            "failed",
            json!({}),
        ),
    ];
    // When applying an exclusive lower and inclusive upper bound.
    let sections = collect_sprint_review(&[owner, ready], &events, Some(lower), upper);
    // Then only the exact upper event is historical; unchanged current snapshots remain.
    assert_eq!(sections.landed.len(), 1);
    assert_eq!(sections.landed[0].sha.as_deref(), Some("new"));
    assert!(sections.sent_back_failed.is_empty());
    assert!(sections.dropped_rejected_escalated.is_empty());
    assert_eq!(sections.in_flight.len(), 1);
    assert_eq!(sections.owner_ideas.len(), 1);
    assert!(sections.numbers.by_task.is_empty());
    assert!(sections.numbers.lessons.is_empty());
    assert_eq!(sections.numbers.gate_failures, 0);
}

#[test]
fn deterministic_collection_orders_tasks_and_prioritizes_owner_reductions() {
    // Given tasks in different enumeration orders and approved/unapproved reductions.
    let (_temp, ctx) = fixture();
    let mut owner = ctx.store.create("owner", None, Status::Ready, "").unwrap();
    owner.origin = Some(Origin::Owner);
    let agent = ctx
        .store
        .create("agent", None, Status::Running, "")
        .unwrap();
    let events = vec![
        event(
            &agent,
            "2026-01-01T00:01:00Z",
            "autopilot",
            json!({"to":"escalated"}),
        ),
        event(
            &owner,
            "2026-01-01T00:02:00Z",
            "idea_decided",
            json!({"action":"narrow","actor":"owner","owner_ok":true}),
        ),
        event(
            &owner,
            "2026-01-01T00:03:00Z",
            "idea_decided",
            json!({"action":"defer","actor":"pm"}),
        ),
    ];
    let at = ts("2026-01-02T00:00:00Z");
    // When collecting twice with the same content in a different task enumeration order.
    let a = collect_sprint_review(&[owner.clone(), agent.clone()], &events, None, at);
    let b = collect_sprint_review(&[agent, owner.clone()], &events, None, at);
    // Then machine output is stable and owner origin takes precedence over event chronology.
    assert_eq!(
        serde_json::to_vec(&a).unwrap(),
        serde_json::to_vec(&b).unwrap()
    );
    assert_eq!(
        a.dropped_rejected_escalated[0].task.as_deref(),
        Some(owner.id.as_str())
    );
    assert_eq!(a.dropped_rejected_escalated[0].owner_ok, Some(true));
    assert!(!a.dropped_rejected_escalated[0].needs_owner_ok);
    assert!(a.dropped_rejected_escalated[1].needs_owner_ok);
    assert!(!a.dropped_rejected_escalated[2].owner_origin);
}

#[test]
fn owner_ideas_cover_every_state_and_preserve_merge_links_after_mark() {
    // Given explicit owner provenance across every state and a legacy Idea.
    let (_temp, ctx) = fixture();
    let mut tasks = Vec::new();
    for status in [
        Status::Idea,
        Status::Backlog,
        Status::Ready,
        Status::Running,
        Status::NeedsMe,
        Status::Review,
        Status::Done,
        Status::Dropped,
    ] {
        let mut task = ctx.store.create(status.as_str(), None, status, "").unwrap();
        task.origin = Some(Origin::Owner);
        tasks.push(task);
    }
    let legacy = ctx.store.create("legacy", None, Status::Idea, "").unwrap();
    tasks.push(legacy);
    tasks[7].idea_decision = Some(corral_core::IdeaDecision {
        revision: 1,
        action: "merge".into(),
        reason: "reason-fixture".into(),
        into: Some(tasks[6].id.clone()),
    });
    let events = vec![event(
        &tasks[7],
        "2026-01-01T00:00:00Z",
        "idea_decided",
        json!({"action":"merge","into":tasks[6].id,"owner_ok":true,"actor":"owner"}),
    )];
    // When collecting a window after all merge events.
    let sections = collect_sprint_review(
        &tasks,
        &events,
        Some(ts("2026-02-01T00:00:00Z")),
        ts("2026-03-01T00:00:00Z"),
    );
    // Then every idea remains visible and the source and destination remain linked.
    assert_eq!(sections.owner_ideas.len(), 9);
    assert_eq!(sections.owner_ideas[7].disposition, tasks[7].idea_decision);
    assert!(sections.owner_ideas[7].links.contains(&tasks[6].id));
    assert!(sections.owner_ideas[6].links.contains(&tasks[7].id));
    assert!(sections.dropped_rejected_escalated.is_empty());
}

#[test]
fn reviewer_children_use_their_own_profile_without_double_counting() {
    // Given worker and reviewer costs, plus an unpriced reviewer.
    let (_temp, ctx) = fixture();
    let mut task = ctx.store.create("work", None, Status::Done, "").unwrap();
    let mut worker = run("R1");
    let mut reviewer = run("V1");
    reviewer.profile = Some("reviewer".into());
    reviewer.usage.as_mut().unwrap().reported_usd = Some(0.5);
    let mut unknown = run("V2");
    unknown.profile = Some("reviewer".into());
    unknown.usage = None;
    worker.review_runs = vec![reviewer, unknown];
    task.runs = vec![worker];
    // When reading only recorded accounting.
    let sections = collect_sprint_review(&[task.clone()], &[], None, ts("2026-02-01T00:00:00Z"));
    // Then child costs are counted once, with unknown costs represented separately.
    assert_eq!(sections.numbers.by_task[&task.id].runs, 3);
    assert_eq!(sections.numbers.by_task[&task.id].reported_usd, Some(1.75));
    assert_eq!(sections.numbers.by_profile["fast"].reported_usd, Some(1.25));
    assert_eq!(
        sections.numbers.by_profile["reviewer"].reported_usd,
        Some(0.5)
    );
    assert_eq!(sections.numbers.by_profile["reviewer"].unknown_cost_runs, 1);
}

#[test]
fn marked_report_is_snapshotted_and_pinned_before_watermark_advances() {
    // Given recorded landing and current work, without a live herdr session.
    let (_temp, ctx) = fixture();
    let done = ctx.store.create("done", None, Status::Done, "").unwrap();
    let ready = ctx.store.create("ready", None, Status::Ready, "").unwrap();
    let events = vec![event(
        &done,
        "2026-01-01T00:10:00Z",
        "approved",
        json!({"sha":"abc"}),
    )];
    write_events(&ctx, &events);
    // When persisting a marked report and then collecting since last.
    let first = review_sprint(&ctx, None, true).unwrap();
    let second = review_sprint(&ctx, Some("last"), false).unwrap();
    // Then artifact bytes match, the mark contains the collection cutoff, and history is excluded.
    assert_eq!(first.artifact.kind, ArtifactKind::Report);
    assert_eq!(
        std::fs::read_to_string(ctx.store.dir().join(&first.artifact.path)).unwrap(),
        first.markdown
    );
    assert!(
        ctx.store
            .pins()
            .unwrap()
            .iter()
            .any(|p| p.target == first.artifact_ref)
    );
    assert_eq!(first.sections.landed.len(), 1);
    assert!(second.sections.landed.is_empty());
    assert_eq!(second.since, Some(first.snapshot_at));
    assert_eq!(second.sections.in_flight[0].task, ready.id);
    assert_eq!(ctx.store.list().unwrap().len(), 2);
    let logged = ctx.store.events().unwrap();
    let mark = logged
        .iter()
        .position(|e| e.kind == "sprint_review_mark")
        .unwrap();
    assert!(logged[..mark].iter().any(|e| e.kind == "artifact"));
    assert!(logged[..mark].iter().any(|e| e.kind == "pin"));
    assert_eq!(logged[mark].detail["snapshot_at"], json!(first.snapshot_at));
    assert_eq!(logged[mark].detail["artifact"], first.artifact_ref);
}

#[test]
fn artifact_failure_does_not_advance_mark() {
    // Given an existing watermark and an artifact size policy that rejects the report.
    let (_temp, mut ctx) = fixture();
    let task = ctx.store.create("anchor", None, Status::Done, "").unwrap();
    let old = event(
        &task,
        "2026-01-01T00:00:00Z",
        "sprint_review_mark",
        json!({}),
    );
    write_events(&ctx, std::slice::from_ref(&old));
    ctx.store.config.artifacts.max_bytes = 0;
    // When artifact persistence fails.
    let result = review_sprint(&ctx, None, true);
    // Then the previous mark remains authoritative and no artifact or pin was added.
    assert!(result.is_err());
    assert_eq!(ctx.store.events().unwrap(), vec![old]);
    assert!(ctx.store.get(&task.id).unwrap().artifacts.is_empty());
    assert!(ctx.store.pins().unwrap().is_empty());
}

#[test]
fn pin_failure_does_not_advance_mark() {
    // Given an existing task and an unwritable pin target (a directory).
    let (_temp, ctx) = fixture();
    let task = ctx.store.create("anchor", None, Status::Done, "").unwrap();
    std::fs::create_dir(ctx.store.dir().join("pins.toml")).unwrap();
    // When artifact creation succeeds but pin persistence fails.
    let result = review_sprint(&ctx, None, true);
    // Then the unpinned artifact is retained, but no watermark hides unreported changes.
    assert!(result.is_err());
    assert_eq!(ctx.store.get(&task.id).unwrap().artifacts.len(), 1);
    assert!(
        !ctx.store
            .events()
            .unwrap()
            .iter()
            .any(|e| e.kind == "sprint_review_mark")
    );
}

#[test]
fn empty_workspace_reuses_single_completed_anchor() {
    // Given an empty workspace.
    let (_temp, ctx) = fixture();
    // When generating multiple reports.
    let first = review_sprint(&ctx, None, true).unwrap();
    let second = review_sprint(&ctx, None, false).unwrap();
    // Then a single completed task hosts both artifacts and does not appear as work.
    let tasks = ctx.store.list().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].status, Status::Done);
    assert_eq!(
        PinRef::parse(&first.artifact_ref).unwrap().task(),
        tasks[0].id
    );
    assert_eq!(
        PinRef::parse(&second.artifact_ref).unwrap().task(),
        tasks[0].id
    );
    assert!(second.sections.in_flight.is_empty());
}

#[test]
fn explicit_timestamp_overrides_mark_and_invalid_since_has_no_side_effects() {
    // Given a last mark later than the landing and an explicit earlier timestamp.
    let (_temp, ctx) = fixture();
    let task = ctx.store.create("work", None, Status::Done, "").unwrap();
    write_events(
        &ctx,
        &[
            event(
                &task,
                "2026-01-01T00:01:00Z",
                "approved",
                json!({"sha":"abc"}),
            ),
            event(
                &task,
                "2026-02-01T00:00:00Z",
                "sprint_review_mark",
                json!({}),
            ),
        ],
    );
    // When selecting an explicit timestamp, then rejecting invalid input.
    let report = review_sprint(&ctx, Some("2026-01-01T00:00:00Z"), false).unwrap();
    let before = ctx.store.events().unwrap();
    let invalid = review_sprint(&ctx, Some("not-a-timestamp"), true);
    // Then the explicit window wins and the invalid request does not change evidence.
    assert_eq!(report.sections.landed.len(), 1);
    assert!(invalid.is_err());
    assert_eq!(ctx.store.events().unwrap(), before);
}

#[test]
fn last_mark_uses_snapshot_cutoff_not_later_persistence_time() {
    // Given a change between collection cutoff and mark persistence, plus an exact-boundary change.
    let (_temp, ctx) = fixture();
    let task = ctx.store.create("work", None, Status::Done, "").unwrap();
    let cutoff = ts("2026-01-01T00:10:00Z");
    write_events(
        &ctx,
        &[
            event(
                &task,
                "2026-01-01T00:10:00Z",
                "approved",
                json!({"sha":"old"}),
            ),
            event(
                &task,
                "2026-01-01T00:11:00Z",
                "approved",
                json!({"sha":"new"}),
            ),
            event(
                &task,
                "2026-01-01T00:12:00Z",
                "sprint_review_mark",
                json!({"snapshot_at":cutoff}),
            ),
        ],
    );
    // When reading the default window.
    let report = review_sprint(&ctx, None, false).unwrap();
    // Then the intervening change is not lost, and the exact old boundary is excluded.
    assert_eq!(report.since, Some(cutoff));
    assert_eq!(report.sections.landed.len(), 1);
    assert_eq!(report.sections.landed[0].sha.as_deref(), Some("new"));
}

#[test]
fn ready_snapshot_keeps_dependencies_and_recorded_queue_reason_without_recent_events() {
    // Given Ready work waiting on a dependency and another task with a recorded queue reason.
    let (_temp, ctx) = fixture();
    let dependency = ctx.store.create("dep", None, Status::Backlog, "").unwrap();
    let mut blocked = ctx
        .store
        .create("blocked", None, Status::Ready, "")
        .unwrap();
    blocked.depends_on.push(dependency.id.clone());
    let mut queued = ctx.store.create("queued", None, Status::Ready, "").unwrap();
    queued.queued = true;
    let events = vec![event(
        &queued,
        "2026-01-01T00:00:00Z",
        "queued",
        json!({"reason":"queue-reason-fixture"}),
    )];
    // When the historical window contains no events.
    let sections = collect_sprint_review(
        &[dependency, blocked.clone(), queued.clone()],
        &events,
        Some(ts("2026-02-01T00:00:00Z")),
        ts("2026-03-01T00:00:00Z"),
    );
    // Then current dependency and wait evidence survives the watermark.
    assert_eq!(sections.in_flight.len(), 2);
    assert_eq!(sections.in_flight[0].depends_on, blocked.depends_on);
    assert!(sections.in_flight[0].wait_reason.is_some());
    assert_eq!(
        sections.in_flight[1].wait_reason.as_deref(),
        events[0].detail["reason"].as_str()
    );
}

#[test]
fn keep_branch_approval_is_not_a_landing() {
    // Given a completed review that explicitly kept its branch off the base.
    let (_temp, ctx) = fixture();
    let task = ctx.store.create("work", None, Status::Done, "").unwrap();
    let events = vec![event(
        &task,
        "2026-01-01T00:10:00Z",
        "approved",
        json!({"strategy":"keep_branch","sha":"abc"}),
    )];
    // When collecting historical landings.
    let sections = collect_sprint_review(&[task], &events, None, ts("2026-02-01T00:00:00Z"));
    // Then approval does not claim integration into the base.
    assert!(sections.landed.is_empty());
}

#[test]
fn discarded_work_appears_in_both_failed_and_dropped_history() {
    // Given a recorded discard without a separate idea disposition.
    let (_temp, ctx) = fixture();
    let task = ctx
        .store
        .create("discarded", None, Status::Dropped, "")
        .unwrap();
    let events = vec![event(
        &task,
        "2026-01-01T00:01:00Z",
        "discarded",
        json!({"run":"R1"}),
    )];
    // When collecting outcomes.
    let sections = collect_sprint_review(&[task], &events, None, ts("2026-02-01T00:00:00Z"));
    // Then the rejected work cannot disappear from the dropped section.
    assert_eq!(sections.sent_back_failed.len(), 1);
    assert_eq!(sections.dropped_rejected_escalated.len(), 1);
    assert_eq!(
        sections.dropped_rejected_escalated[0].action.as_deref(),
        Some("drop")
    );
}

#[test]
fn original_change_verdict_keeps_its_notes_after_later_approval() {
    // Given an original changes verdict and a later, different approval on the same run.
    let (_temp, ctx) = fixture();
    let mut task = ctx
        .store
        .create("reviewed", None, Status::Done, "")
        .unwrap();
    task.runs.push(run("R1"));
    task.runs[0].review.as_mut().unwrap().notes = Some("approval-fixture".into());
    let events = vec![
        event(
            &task,
            "2026-01-01T00:03:00Z",
            "verdict",
            json!({"run":"R1","verdict":"changes","reviewer":"reviewer","notes":"caught-fixture"}),
        ),
        event(
            &task,
            "2026-01-01T00:10:00Z",
            "approved",
            json!({"run":"R1","sha":"abc"}),
        ),
    ];
    // When collecting review history after approval overwrote the latest review record.
    let sections = collect_sprint_review(&[task], &events, None, ts("2026-02-01T00:00:00Z"));
    // Then the original finding and its resolution are retained.
    assert_eq!(sections.sent_back_failed.len(), 1);
    assert_eq!(
        sections.sent_back_failed[0].reason.as_deref(),
        Some("caught-fixture")
    );
    assert_eq!(
        sections.sent_back_failed[0].actor.as_deref(),
        Some("reviewer")
    );
    assert_eq!(
        sections.sent_back_failed[0].resolution.as_deref(),
        Some("approved")
    );
}
