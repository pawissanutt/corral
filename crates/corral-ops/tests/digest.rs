use corral_core::{ArtifactKind, Event, Run, Status, Store, Task};
use corral_ops::{Ctx, collect_digest, digest_at, scheduled_digest_at};
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
        "id":id,"agent":"worker","worktree":"/must-not-read","branch":"unused",
        "base":"main","base_sha":"base",
        "started":"2026-01-01T00:00:00Z","ended":"2026-01-01T00:10:00Z",
        "review":{"reviewer":"pm","sha":"abc","at":"2026-01-01T00:09:00Z","verdict":"approve"},
        "usage":{"input":10,"output":5,"cache_read":0,"cache_write":0,
            "reported_usd":1.25,"estimated_usd":2.5}
    }))
    .unwrap()
}

fn event(task: &Task, at: &str, kind: &str, detail: Value) -> Event {
    Event {
        ts: ts(at),
        task: Some(task.id.clone()),
        kind: kind.into(),
        detail,
    }
}

#[test]
fn digest_collects_outcomes_and_counts_reviewer_cost_once() {
    // Given all requested outcomes plus two control events that are not landings.
    let (_temp, ctx) = fixture();
    let mut task = ctx
        .store
        .create("outcomes", None, Status::Done, "")
        .unwrap();
    let mut worker = run("R1");
    let mut reviewer = run("R1-review");
    reviewer.usage = None;
    worker.review_runs.push(reviewer);
    task.runs.push(worker);
    let events = [
        event(
            &task,
            "2026-01-01T00:01:00Z",
            "changes_requested",
            json!({"run":"R1","text":"fix","by":"autopilot","submitted":true}),
        ),
        event(
            &task,
            "2026-01-01T00:02:00Z",
            "autopilot",
            json!({"run":"R1","to":"changes"}),
        ),
        event(
            &task,
            "2026-01-01T00:03:00Z",
            "autopilot",
            json!({"run":"R1","to":"escalated","reason":"limit"}),
        ),
        event(
            &task,
            "2026-01-01T00:04:00Z",
            "needs_me",
            json!({"run":"R1","reason":"owner"}),
        ),
        event(
            &task,
            "2026-01-01T00:05:00Z",
            "moved",
            json!({"to":"dropped"}),
        ),
        event(
            &task,
            "2026-01-01T00:10:00Z",
            "approved",
            json!({"run":"R1","sha":"abc","strategy":"rebase"}),
        ),
        event(
            &task,
            "2026-01-01T00:11:00Z",
            "approved",
            json!({"run":"R1","sha":"kept","strategy":"keep_branch"}),
        ),
        event(
            &task,
            "2026-01-01T00:12:00Z",
            "autopilot",
            json!({"run":"R1","to":"done","sha":"abc"}),
        ),
    ];
    // When collecting twice at the same clock, without reading source paths.
    let collect = || {
        collect_digest(
            &[task.clone()],
            &events,
            ts("2026-01-01T00:00:00Z"),
            ts("2026-01-02T00:00:00Z"),
        )
    };
    let digest = collect();
    // Then the report is deterministic and recorded/estimated costs remain separate.
    assert_eq!(
        serde_json::to_vec(&digest).unwrap(),
        serde_json::to_vec(&collect()).unwrap()
    );
    assert_eq!(digest.landed.len(), 1);
    assert_eq!(digest.landed[0].sha.as_deref(), Some("abc"));
    assert_eq!(digest.landed[0].task.as_deref(), Some(task.id.as_str()));
    assert_eq!(digest.landed[0].verdict.as_deref(), Some("approve"));
    assert_eq!(digest.escalated.len(), 2);
    assert_eq!(digest.sent_back.len(), 1);
    assert_eq!(digest.dropped.len(), 1);
    assert_eq!(digest.totals.runs, 2);
    assert_eq!(digest.totals.unknown_cost_runs, 1);
    assert_eq!(digest.totals.reported_usd, Some(1.25));
    assert_eq!(digest.totals.estimated_usd, Some(2.5));
    assert_eq!(digest.totals.wall_seconds, Some(1200));
    assert_eq!(digest.by_task[&task.id], digest.totals);
}

#[test]
fn digest_window_is_exclusive_start_inclusive_end_and_uses_historical_verdict() {
    // Given old, boundary and future outcomes, and a review overwritten after landing.
    let (_temp, ctx) = fixture();
    let mut task = ctx.store.create("history", None, Status::Done, "").unwrap();
    let mut worker = run("R1");
    worker.review.as_mut().unwrap().verdict = Some(corral_core::Verdict::Escalate);
    worker.review.as_mut().unwrap().at = ts("2026-01-03T00:00:00Z");
    task.runs.push(worker);
    let events = [
        event(
            &task,
            "2026-01-01T00:00:00Z",
            "verdict",
            json!({"run":"R1","verdict":"approve"}),
        ),
        event(
            &task,
            "2026-01-01T00:00:00Z",
            "approved",
            json!({"sha":"old","run":"R1"}),
        ),
        event(
            &task,
            "2026-01-02T00:00:00Z",
            "approved",
            json!({"sha":"boundary","run":"R1"}),
        ),
        event(
            &task,
            "2026-01-03T00:00:00Z",
            "verdict",
            json!({"run":"R1","verdict":"escalate"}),
        ),
        event(
            &task,
            "2026-01-03T00:00:00Z",
            "approved",
            json!({"sha":"future","run":"R1"}),
        ),
    ];
    // When reporting only the day ending on the second boundary.
    let digest = collect_digest(
        &[task],
        &events,
        ts("2026-01-01T00:00:00Z"),
        ts("2026-01-02T00:00:00Z"),
    );
    // Then the cutoff excludes both the old approval and the future review.
    assert_eq!(digest.landed.len(), 1);
    assert_eq!(digest.landed[0].sha.as_deref(), Some("boundary"));
    assert_eq!(digest.landed[0].verdict.as_deref(), Some("approve"));
    assert!(digest.escalated.is_empty());
}

#[test]
fn accounting_excludes_runs_outside_window_and_unknown_is_not_zero() {
    // Given an old run, an unknown run ending exactly at the cutoff, and a future run.
    let (_temp, ctx) = fixture();
    let mut task = ctx.store.create("costs", None, Status::Done, "").unwrap();
    let old = run("R1");
    let mut boundary = run("R2");
    boundary.started = ts("2026-01-01T23:00:00Z");
    boundary.ended = Some(ts("2026-01-02T00:00:00Z"));
    boundary.usage = None;
    let mut future = run("R3");
    future.ended = Some(ts("2026-01-03T00:00:00Z"));
    task.runs = vec![old, boundary, future];
    // When the old end is exactly the excluded start boundary.
    let digest = collect_digest(
        &[task],
        &[],
        ts("2026-01-01T00:10:00Z"),
        ts("2026-01-02T00:00:00Z"),
    );
    // Then only the boundary run contributes time, with explicitly unknown costs.
    assert_eq!(digest.totals.runs, 1);
    assert_eq!(digest.totals.unknown_cost_runs, 1);
    assert_eq!(digest.totals.wall_seconds, Some(3600));
    assert_eq!(digest.totals.reported_usd, None);
    assert_eq!(digest.totals.estimated_usd, None);
}

#[test]
fn send_backs_include_manual_delivery_and_queued_retries_not_failed_delivery() {
    // Given a manual send, a failed delivery, and an autopilot retry with no live pane.
    let (_temp, ctx) = fixture();
    let task = ctx
        .store
        .create("repairs", None, Status::Review, "")
        .unwrap();
    let events = [
        event(
            &task,
            "2026-01-01T01:00:00Z",
            "changes_requested",
            json!({"text":"manual","by":"owner","submitted":true}),
        ),
        event(
            &task,
            "2026-01-01T02:00:00Z",
            "changes_requested",
            json!({"text":"failed","by":"owner","submitted":false}),
        ),
        event(
            &task,
            "2026-01-01T03:00:00Z",
            "autopilot",
            json!({"run":"R2","to":"changes","reason":"queued retry"}),
        ),
    ];
    // When collecting the event window.
    let digest = collect_digest(
        &[task],
        &events,
        ts("2026-01-01T00:00:00Z"),
        ts("2026-01-02T00:00:00Z"),
    );
    // Then only the successful manual request and queued retry are reported.
    assert_eq!(digest.sent_back.len(), 2);
    assert_eq!(digest.sent_back[0].kind, "changes_requested");
    assert_eq!(digest.sent_back[1].run.as_deref(), Some("R2"));
}

#[test]
fn digest_snapshots_a_protected_report_and_pins_it() {
    // Given an empty store and a fixed report clock.
    let (_temp, ctx) = fixture();
    let now = ts("2026-01-02T00:00:00Z");
    // When explicitly creating a daily digest.
    let report = digest_at(&ctx, "1d", now).unwrap();
    // Then the report is a real protected artifact reachable by a pin.
    assert_eq!(report.digest.since, ts("2026-01-01T00:00:00Z"));
    assert_eq!(report.artifact.kind, ArtifactKind::Report);
    assert_eq!(ctx.store.pins().unwrap()[0].target, report.artifact_ref);
    assert_eq!(
        corral_ops::artifact_read(&ctx, &report.artifact).unwrap(),
        report.markdown.as_bytes()
    );
    assert_eq!(ctx.store.list().unwrap().len(), 1);
    assert!(digest_at(&ctx, "0d", now).is_err());
}

#[test]
fn scheduled_digest_is_opt_in_and_resumes_without_window_gaps() {
    // Given an enabled schedule and a manual report that must not advance its cursor.
    let (_temp, mut ctx) = fixture();
    let first = ts("2026-01-02T00:00:00Z");
    assert!(scheduled_digest_at(&ctx, first).unwrap().is_none());
    ctx.store.config.digest.every = Some("1d".into());
    digest_at(&ctx, "1h", first).unwrap();
    // When sync calls the schedule at repeated, pre-due, due and catch-up clocks.
    let initial = scheduled_digest_at(&ctx, first).unwrap().unwrap();
    let repeat = scheduled_digest_at(&ctx, first).unwrap();
    let early = scheduled_digest_at(&ctx, ts("2026-01-02T23:59:59Z")).unwrap();
    let due = scheduled_digest_at(&ctx, ts("2026-01-03T00:00:00Z"))
        .unwrap()
        .unwrap();
    let catchup = scheduled_digest_at(&ctx, ts("2026-01-06T00:00:00Z"))
        .unwrap()
        .unwrap();
    // Then there are no duplicate scheduled snapshots or missed downtime intervals.
    assert_eq!(initial.digest.since, ts("2026-01-01T00:00:00Z"));
    assert!(repeat.is_none());
    assert!(early.is_none());
    assert_eq!(due.digest.since, initial.digest.snapshot_at);
    assert_eq!(catchup.digest.since, due.digest.snapshot_at);
    assert_eq!(
        ctx.store
            .events()
            .unwrap()
            .iter()
            .filter(|e| e.kind == "digest" && e.detail["scheduled"] == true)
            .count(),
        3
    );
    assert_eq!(ctx.store.pins().unwrap().len(), 4);
    assert_eq!(ctx.store.list().unwrap().len(), 1);
}

#[test]
fn digest_schedule_configuration_rejects_invalid_ages() {
    // Given the default configuration, including an omitted digest schedule.
    let (temp, ctx) = fixture();
    let mut config = ctx.store.config.clone();
    assert!(config.digest.every.is_none());
    // When validating both enabled and malformed schedule values.
    config.digest.every = Some("1d".into());
    assert!(config.validate(temp.path()).is_ok());
    config.digest.every = Some("0d".into());
    let error = config.validate(temp.path()).unwrap_err();
    // Then invalid schedules fail at the configuration boundary.
    assert!(
        error
            .0
            .iter()
            .any(|problem| problem.starts_with("digest.every:"))
    );
}

#[test]
fn idea_drops_are_reported_once_when_move_has_an_owner_audit() {
    // Given a declined idea and a dropped task with paired owner audit/move events.
    let (_temp, ctx) = fixture();
    let task = ctx
        .store
        .create("drops", None, Status::Dropped, "")
        .unwrap();
    let events = [
        event(
            &task,
            "2026-01-01T01:00:00Z",
            "idea_decided",
            json!({"action":"decline","reason":"not needed"}),
        ),
        event(
            &task,
            "2026-01-01T02:00:00Z",
            "idea_decided",
            json!({"action":"drop","owner_ok":true,"task_updated":"revision"}),
        ),
        event(
            &task,
            "2026-01-01T02:00:01Z",
            "moved",
            json!({"to":"dropped","task_updated":"revision"}),
        ),
    ];
    // When collecting the day.
    let digest = collect_digest(
        &[task],
        &events,
        ts("2026-01-01T00:00:00Z"),
        ts("2026-01-02T00:00:00Z"),
    );
    // Then each disposition appears once, without double-counting its move.
    assert_eq!(digest.dropped.len(), 2);
    assert!(digest.dropped.iter().all(|row| row.kind == "idea_decided"));
}
