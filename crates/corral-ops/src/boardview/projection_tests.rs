use super::*;
use corral_core::work::{self, Counter, HotScope};
use corral_core::{Run, Task};
use corral_protocol::board::RailGlyph;
use serde_json::json;

fn fixture() -> (tempfile::TempDir, Ctx) {
    let root = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(Store::init(root.path()).unwrap());
    (root, ctx)
}

fn task(ctx: &Ctx, title: &str) -> Task {
    ctx.store.create(title, None, Status::Ready, "").unwrap()
}

fn run(n: usize) -> Run {
    serde_json::from_value(json!({
        "id": format!("R{n}"), "agent": "fixture", "worktree": "/unused",
        "branch": "fixture", "base": "main", "base_sha": "base",
        "started": "2026-11-01T00:00:00Z"
    }))
    .unwrap()
}

fn native_questions(task: &str, run: &str) -> Box<corral_core::agent::AgentRunState> {
    let mut host =
        crate::agent::native_test_support::host(corral_core::agent::AgentRole::Worker, 0);
    host.questions.push(
        serde_json::from_value(json!({
            "version": 1, "key": format!("{task}/{run}/Q1"), "revision": 1,
            "asked": { "request_id": "ask", "payload_sha256": "00", "session_id": "fixed-session",
                "generation": 999, "at": "2026-11-01T00:00:00Z" },
            "kind": "clarification", "blocking": false, "text": "question",
            "state": "open", "wake": { "route": "none", "key": "wake", "state": "pending" }
        }))
        .unwrap(),
    );
    Box::new(host)
}

#[test]
fn row_computation_does_not_read_more_as_run_history_grows() {
    let mut measured = Vec::new();
    for history in [2, 4] {
        let (_root, ctx) = fixture();
        // DecodedDocBytes includes timestamps: wall-clock fractional precision must not
        // change the serialized fixture size between the two history measurements.
        ctx.store
            .fixture_pin_now("2026-11-01T00:00:00Z".parse().unwrap())
            .unwrap();
        let mut task = task(&ctx, "same");
        task.status = Status::Running;
        for n in 1..=history {
            let mut old = run(n);
            old.outcome = Some(corral_core::RunOutcome::Superseded);
            old.ended = Some("2026-11-01T00:00:01Z".parse().unwrap());
            // Its inbox is unsettled and its question remains recorded-pending even
            // though supersession makes the container semantically closed.
            old.agent_host = Some(native_questions(&task.id, &old.id));
            task.runs.push(old);
        }
        let mut current = run(history + 1);
        current.agent_host = Some(native_questions(&task.id, &current.id));
        task.runs.push(current);
        ctx.store.fixture_put_task(&task).unwrap();
        let _hot = HotScope::enter();
        let before = work::snapshot();
        let row = compute_row(&ctx, &task.id).unwrap().unwrap();
        assert_eq!(row.group, corral_protocol::board::BoardGroup::Producing);
        assert_eq!(row.questions, 1);
        let after = work::snapshot();
        assert_eq!(
            after.total(Counter::HistoryRowDecodes) - before.total(Counter::HistoryRowDecodes),
            0
        );
        assert_eq!(
            after.total(Counter::FullTaskHydrations) - before.total(Counter::FullTaskHydrations),
            0
        );
        measured.push(Counter::ALL.map(|counter| after.total(counter) - before.total(counter)));
    }
    assert_eq!(measured[0], measured[1]);
}

#[test]
fn fold_preserves_untouched_revisions_and_resets_after_feed_retention() {
    let (root, ctx) = fixture();
    let mut changed = task(&ctx, "first");
    let untouched = task(&ctx, "second");
    let mut projection = Projection::rebuild(&ctx).unwrap();
    let initial_rev = projection.rows()[&untouched.id].rev;
    changed.title = "renamed".into();
    ctx.store.fixture_put_task(&changed).unwrap();
    let outcome = projection.fold(&ctx).unwrap();
    assert_eq!(outcome.changed, [changed.id.clone()]);
    assert!(!outcome.reset);
    assert!(outcome.left.is_empty());
    assert_eq!(projection.rows()[&changed.id].rev, outcome.folded_through.0);
    assert!(projection.rows()[&changed.id].rev > initial_rev);
    assert_eq!(projection.rows()[&untouched.id].rev, initial_rev);
    // A task write whose displayed content is identical must not bump the row revision.
    let before = projection.rows()[&changed.id].rev;
    ctx.store.fixture_put_task(&changed).unwrap();
    assert!(projection.fold(&ctx).unwrap().changed.is_empty());
    assert_eq!(projection.rows()[&changed.id].rev, before);

    let epoch = projection.epoch;
    changed.title = "after retention".into();
    ctx.store.fixture_put_task(&changed).unwrap();
    let through = ctx.store.view().unwrap().seq();
    let backend = corral_core::sqlite::SqliteBackend::open(
        &root.path().join(".corral"),
        corral_core::sqlite::SqliteOptions::default(),
    )
    .unwrap();
    backend.prune_changes_through(through).unwrap();
    let outcome = projection.fold(&ctx).unwrap();
    assert!(outcome.reset);
    assert_ne!(projection.epoch, epoch);
    assert_eq!(projection.rows()[&changed.id].title, changed.title);
    assert!(matches!(
        projection.delta_since(
            epoch,
            &BoardCursor {
                rev: before,
                after: None
            },
            10
        ),
        DeltaOutcome::Reset { .. }
    ));
}

#[test]
fn a_passing_gate_and_review_on_an_older_candidate_are_stale() {
    let (_root, ctx) = fixture();
    let mut task = task(&ctx, "candidate");
    task.status = Status::Review;
    let mut run = run(1);
    run.outcome = Some(corral_core::RunOutcome::Done);
    run.autopilot = Some(
        serde_json::from_value(json!({
            "stage": "reviewing", "sha": "bbbbbbbb", "at": "2026-11-01T00:00:02Z"
        }))
        .unwrap(),
    );
    run.gate = Some(
        serde_json::from_value(json!({
            "result": "pass", "sha": "aaaaaaaa", "at": "2026-11-01T00:00:01Z"
        }))
        .unwrap(),
    );
    run.review = Some(serde_json::from_value(json!({
        "verdict": "approve", "reviewer": "owner", "sha": "aaaaaaaa", "at": "2026-11-01T00:00:01Z"
    })).unwrap());
    task.runs.push(run);
    ctx.store.fixture_put_task(&task).unwrap();
    let row = compute_row(&ctx, &task.id).unwrap().unwrap();
    assert_eq!(row.rail[1].glyph, RailGlyph::Stale);
    assert_eq!(row.rail[2].glyph, RailGlyph::Stale);
}

#[test]
fn delta_paging_and_cache_reload_cannot_lose_same_revision_rows_or_removals() {
    let (root, ctx) = fixture();
    let first = task(&ctx, "first");
    let mut second = task(&ctx, "second");
    let mut projection = Projection::rebuild(&ctx).unwrap();
    let DeltaOutcome::Delta(initial) =
        projection.delta_since(projection.epoch, &BoardCursor::default(), 1)
    else {
        panic!("reset");
    };
    assert_eq!(initial.rows.len(), 1);
    assert_eq!(initial.rows[0].id, first.id);
    assert!(initial.more);
    // The second row leaves before the client has reached it in this revision.
    second.status = Status::Done;
    ctx.store.fixture_put_task(&second).unwrap();
    let outcome = projection.fold(&ctx).unwrap();
    assert_eq!(outcome.left, [second.id.clone()]);
    let path = root.path().join(CACHE_FILE);
    projection.save(&path).unwrap();
    let restored = Projection::load(&path).unwrap();
    let DeltaOutcome::Delta(delta) = restored.delta_since(restored.epoch, &initial.cursor, 1)
    else {
        panic!("reset");
    };
    assert_eq!(delta.left, [second.id]);
    assert!(delta.rows.is_empty());
    assert!(!delta.more);
    let DeltaOutcome::Delta(repeat) = restored.delta_since(restored.epoch, &delta.cursor, 1) else {
        panic!("reset");
    };
    assert!(repeat.rows.is_empty() && repeat.left.is_empty());
    assert_eq!(restored.rows()[&first.id].rev, initial.rows[0].rev);
    std::fs::write(&path, b"broken").unwrap();
    assert!(Projection::load(&path).is_none());
    std::fs::write(
        &path,
        serde_json::to_vec(&json!({"version": 2, "projection": projection})).unwrap(),
    )
    .unwrap();
    assert!(Projection::load(&path).is_none());
}

#[test]
fn rebuild_cursor_pages_same_revision_rows_without_duplicates_or_omissions() {
    let (_root, ctx) = fixture();
    let expected = (0..3)
        .map(|n| task(&ctx, &format!("task {n}")).id)
        .collect::<BTreeSet<_>>();
    let projection = Projection::rebuild(&ctx).unwrap();
    let mut cursor = BoardCursor::default();
    let mut seen = BTreeSet::new();
    loop {
        let DeltaOutcome::Delta(delta) = projection.delta_since(projection.epoch, &cursor, 2)
        else {
            panic!("reset");
        };
        assert!(delta.rows.len() + delta.left.len() <= 2);
        for row in delta.rows {
            assert!(seen.insert(row.id));
        }
        assert_ne!(cursor, delta.cursor);
        cursor = delta.cursor;
        if !delta.more {
            break;
        }
    }
    assert_eq!(seen, expected);
}
