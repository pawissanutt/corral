use super::*;
use corral_core::{Run, Status, Store, work};
use serde_json::{Value, json};

thread_local! {
    static RUN_ROWS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

pub(super) fn record_run_rows(count: usize) {
    RUN_ROWS.with(|rows| rows.set(rows.get() + count as u64));
}

fn fixture_run(id: &str, started: &str, ended: Option<&str>) -> Run {
    serde_json::from_value(json!({
        "id": id, "agent": "fixture", "worktree": "/unused", "branch": "fixture",
        "base": "main", "base_sha": "fixture", "started": started, "ended": ended,
    }))
    .unwrap()
}

fn historical_window(history: usize) -> Result<Vec<(TimelinePage, u64, u64)>> {
    let dir = tempfile::tempdir()?;
    let store = Store::init(dir.path())?;
    let mut task = store.create("Window", None, Status::Running, "")?;
    task.created = "2025-01-01T00:00:00Z".parse()?;
    task.runs = (0..history)
        .map(|n| {
            fixture_run(
                &format!("old-{n}"),
                "2025-01-02T00:00:00Z",
                Some("2025-01-03T00:00:00Z"),
            )
        })
        .collect();
    let mut current = fixture_run("R1", "2026-11-01T00:00:00Z", Some("2026-11-06T00:00:00Z"));
    let mut reviewer = fixture_run(
        "review-1",
        "2026-11-07T01:00:00Z",
        Some("2026-11-07T02:00:00Z"),
    );
    reviewer.session = Some(serde_json::from_value(json!({
        "id":"actual-review-session", "policy":"reuse", "directory":"/unused",
        "harness":"pi", "action":"new",
    }))?);
    current.review_runs.push(reviewer);
    task.runs.push(current);
    store.fixture_put_task(&task)?;
    let event = |at: &str, kind: &str, detail: Value, actor: Option<&str>, task: Option<&str>| json!({"ts":at, "kind":kind, "task":task, "detail":detail, "actor":actor});
    let mut events = vec![
        event(
            "2026-11-01T00:00:00Z",
            "dispatched",
            json!({"run":"R1"}),
            None,
            Some(&task.id),
        ),
        event(
            "2026-11-06T00:00:00Z",
            "done",
            json!({"run":"R1"}),
            None,
            Some(&task.id),
        ),
        event(
            "2026-11-07T00:00:00Z",
            "gate_started",
            json!({"run":"R1"}),
            None,
            Some(&task.id),
        ),
        event(
            "2026-11-07T01:00:00Z",
            "review_started",
            json!({"run":"R1"}),
            None,
            Some(&task.id),
        ),
        event(
            "2026-11-07T03:00:00Z",
            "needs_me",
            json!({"run":"R1","decision":"d1"}),
            None,
            Some(&task.id),
        ),
        event(
            "2026-11-07T04:00:00Z",
            "needs_me",
            json!({"decision":"d2"}),
            None,
            Some(&task.id),
        ),
        event(
            "2026-11-07T05:00:00Z",
            "run_resumed",
            json!({"run":"R1","decision":"d1","outcome":"answered"}),
            None,
            Some(&task.id),
        ),
        event(
            "2026-11-07T06:00:00Z",
            "autopilot",
            json!({"run":"R1","to":"reviewed"}),
            None,
            Some(&task.id),
        ),
        event(
            "2026-11-07T07:00:00Z",
            "autopilot",
            json!({"to":"changes"}),
            None,
            Some(&task.id),
        ),
        event(
            "2026-11-07T08:00:00Z",
            "autopilot",
            json!({"to":"reviewed"}),
            None,
            Some(&task.id),
        ),
        event(
            "2026-11-07T09:00:00Z",
            "autopilot",
            json!({"run":"R1","to":"reviewed"}),
            None,
            Some(&task.id),
        ),
        event(
            "2026-11-14T08:00:00Z",
            "autopilot",
            json!({"run":"R1","to":"landing"}),
            None,
            Some(&task.id),
        ),
        event(
            "2026-11-14T09:00:00Z",
            "run_resumed",
            json!({"decision":"d2","outcome":"answered"}),
            None,
            Some(&task.id),
        ),
        event(
            "2026-11-14T10:00:00Z",
            "gate",
            json!({"run":"R1","result":"pass"}),
            None,
            Some(&task.id),
        ),
        event(
            "2026-11-14T11:00:00Z",
            "verdict",
            json!({"run":"R1","verdict":"approve"}),
            None,
            Some(&task.id),
        ),
        event(
            "2026-11-14T12:00:00Z",
            "commented",
            json!({}),
            Some("owner"),
            Some(&task.id),
        ),
        event(
            "2026-11-14T13:00:00Z",
            "pm_checked",
            json!({}),
            Some("pm"),
            None,
        ),
        event(
            "2026-11-15T00:00:00Z",
            "whole_second",
            json!({}),
            Some("owner"),
            None,
        ),
        event(
            "2026-11-15T00:00:00.122999999Z",
            "fractional_before_to",
            json!({}),
            Some("owner"),
            None,
        ),
        event(
            "2026-11-15T00:00:00.123Z",
            "excluded_at_to",
            json!({}),
            Some("owner"),
            None,
        ),
    ];
    // Old timestamps deliberately have later sequences. Neither historical rows
    // nor completed attempts of this same run belong in the window's work budget.
    for n in 0..history {
        events.push(event(
            "2025-01-02T00:00:00Z",
            "dispatched",
            json!({"run":format!("old-{n}")}),
            None,
            Some(&task.id),
        ));
        events.push(event(
            "2025-01-03T00:00:00Z",
            "done",
            json!({"run":format!("old-{n}")}),
            None,
            Some(&task.id),
        ));
        events.push(event(
            "2026-11-02T00:00:00Z",
            "gate_started",
            json!({"run":"R1"}),
            None,
            Some(&task.id),
        ));
        events.push(event(
            "2026-11-03T00:00:00Z",
            "gate",
            json!({"run":"R1","result":"fail"}),
            None,
            Some(&task.id),
        ));
        events.push(event(
            "2025-01-02T00:00:00Z",
            "commented",
            json!({}),
            Some("owner"),
            None,
        ));
    }
    store.fixture_replace_events(
        &events
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )?;
    // Exercise the existing-database additive-index path as well as initial creation.
    drop(store);
    let ctx = Ctx::open(dir.path())?;
    let mut found = Vec::new();
    for from in ["2026-11-14T00:00:00Z", "2026-11-08T00:00:00Z"] {
        let before = work::snapshot();
        let rows_before = RUN_ROWS.with(|rows| rows.get());
        let hot = work::HotScope::enter();
        let DetailBody::Timeline(page) =
            timeline(&ctx, from, "2026-11-15T00:00:00.123Z", TimelineBy::Agent)?
        else {
            panic!("wrong detail");
        };
        drop(hot);
        let after = work::snapshot();
        found.push((
            page,
            after.total(work::Counter::EventDecodes) - before.total(work::Counter::EventDecodes),
            RUN_ROWS.with(|rows| rows.get()) - rows_before,
        ));
    }
    Ok(found)
}

#[test]
fn timeline_window_ignores_historical_growth_and_preserves_crossing_phase_context() -> Result<()> {
    let small = historical_window(2)?;
    let larger = historical_window(12)?;
    assert_eq!(
        small, larger,
        "historical growth must not change the data or decode work"
    );
    for (page, events, rows) in small {
        assert!(events > 0 && rows > 0, "work counters must be active");
        let gate = page
            .lanes
            .iter()
            .flat_map(|lane| &lane.segments)
            .find(|span| span.kind == TimelineKind::Gate)
            .unwrap();
        assert_eq!(gate.from.as_deref(), Some("2026-11-07T00:00:00Z"));
        assert_eq!(gate.to.as_deref(), Some("2026-11-14T10:00:00Z"));
        let reviewer = page
            .lanes
            .iter()
            .find(|lane| lane.id == "actual-review-session")
            .unwrap();
        let review = reviewer
            .segments
            .iter()
            .find(|span| span.kind == TimelineKind::Review)
            .unwrap();
        assert_eq!(review.from.as_deref(), Some("2026-11-07T01:00:00Z"));
        assert_eq!(review.to.as_deref(), Some("2026-11-14T11:00:00Z"));
        for (kind, start, end) in [
            (
                TimelineKind::Ask,
                "2026-11-07T03:00:00Z",
                "2026-11-14T09:00:00Z",
            ),
            (
                TimelineKind::Held,
                "2026-11-07T08:00:00Z",
                "2026-11-14T08:00:00Z",
            ),
        ] {
            let span = page
                .lanes
                .iter()
                .flat_map(|lane| &lane.segments)
                .find(|span| span.kind == kind)
                .unwrap();
            assert_eq!(span.from.as_deref(), Some(start));
            assert_eq!(span.to.as_deref(), Some(end));
        }
        assert_eq!(page.owner_actions.len(), 3);
        assert_eq!(
            page.owner_actions
                .iter()
                .map(|action| action.text.as_str())
                .collect::<Vec<_>>(),
            ["commented", "whole_second", "fractional_before_to"]
        );
        assert_eq!(page.pm.len(), 1);
        assert_eq!(page.pm[0].text, "pm_checked");
    }
    Ok(())
}
