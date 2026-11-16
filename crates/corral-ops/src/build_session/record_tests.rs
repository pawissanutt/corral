use super::*;
use corral_core::Store;
use corral_core::build_record::OutcomeState;

fn record() -> BuildRecord {
    serde_json::from_value(serde_json::json!({
        "version": 1, "id": "B-a", "kind": "build", "project": "corral",
        "title": "Keep build results in session",
        "intent": [{"at": "2026-11-15T10:00:00Z", "text": "Keep my results here."}],
        "created": "2026-11-15T10:00:00Z", "updated": "2026-11-15T10:00:01Z",
        "repo_root": "/repo", "worktree": "/repo/worktree", "branch": "build-a", "base": null,
        "outcome": {"state": "open"},
        "diff": {"commits": 1, "files": 1, "added": 2, "removed": 1},
        "commits": [{"sha": "abc", "subject": "Keep results", "slice": "S1"}],
        "slices": [{
            "id": "S1", "title": "Store results", "kind": "slice", "owns": ["src/store.rs"],
            "model": "provider/worker", "effort": "high", "state": "done",
            "started": "2026-11-15T10:00:00Z", "ended": "2026-11-15T10:00:01Z",
            "usage": {"input": 2, "output": 3, "cost": 0.25, "turns": 1},
            "outside": ["src/lib.rs"], "report": "Stored results."
        }],
        "verification": [{
            "attempt": "G1", "tier": "quick", "head": "abc", "result": "fail",
            "failed_command": "check", "started": "2026-11-15T10:00:00Z",
            "ended": "2026-11-15T10:00:01Z", "log": "/logs/G1.log"
        }],
        "reviews": [{
            "round": 1, "reviewer": "V1", "model": "provider/reviewer", "head": "abc",
            "verdict": "changes", "at": "2026-11-15T10:00:01Z", "summary": "One finding",
            "findings": [{"id": "F1", "severity": "minor", "path": "src/store.rs", "line": 1,
                "text": "Check the edge case", "resolution": {
                    "state": "declined", "commit": "abc", "reason": "Owner accepts the tradeoff"
                }}]
        }],
        "messages": [{"at": "2026-11-15T10:00:00Z", "from": "S1", "to": "parent",
            "text": "Result is ready.", "delivery": "delivered"}],
        "decisions": [{"at": "2026-11-15T10:00:01Z", "kind": "override",
            "text": "Keep this finding?", "answer": "Yes"}],
        "usage": {
            "parent": {"input": 1, "output": 1}, "children": {"input": 2, "output": 3, "cost": 0.25},
            "reviewers": {"input": 1, "output": 2, "cost": 0.1}, "total": {"input": 4, "output": 6}
        },
        "timeline": [{"at": "2026-11-15T10:00:00Z", "phase": "started", "detail": "Owner request"}],
        "transcripts": {"parent": "/logs/parent.jsonl", "children": {"S1": "/logs/S1.jsonl"}}
    })).unwrap()
}

#[test]
fn round_trip_preserves_wire_data_revisions_events_and_task_isolation() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(Store::init(temp.path()).unwrap());
    let mut record = record();
    let json = serde_json::to_string(&record).unwrap();
    let first = publish(&ctx, &json).unwrap();
    assert_eq!((first.id.as_str(), first.revision), (record.id.as_str(), 1));
    assert_eq!(
        serde_json::to_string(&show(&ctx, &record.id).unwrap()).unwrap(),
        json
    );
    let events = ctx.store.events().unwrap();
    let recorded: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "build_recorded")
        .collect();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].task, None);
    assert_eq!(
        recorded[0].detail,
        serde_json::json!({
            "id": "B-a", "kind": "build", "project": "corral", "title": record.title
        })
    );
    assert!(!events.iter().any(|e| e.kind == "build_outcome"));
    assert_eq!(publish(&ctx, &json).unwrap().revision, 1);
    assert_eq!(ctx.store.events().unwrap().len(), events.len());

    record.outcome.state = OutcomeState::Landed;
    record.outcome.at = Some("2026-11-15T10:00:02Z".into());
    record.outcome.head = Some("abc".into());
    record.outcome.landed = Some("def".into());
    record.outcome.base_branch = Some("main".into());
    record.outcome.override_reason = Some("Owner accepts the tradeoff".into());
    record.updated = "2026-11-15T10:00:02Z".into();
    let json = serde_json::to_string(&record).unwrap();
    assert_eq!(publish(&ctx, &json).unwrap().revision, 2);
    assert_eq!(
        serde_json::to_string(&show(&ctx, &record.id).unwrap()).unwrap(),
        json
    );
    let events = ctx.store.events().unwrap();
    let outcomes: Vec<_> = events
        .iter()
        .filter(|e| e.kind == "build_outcome")
        .collect();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].task, None);
    assert_eq!(
        outcomes[0].detail,
        serde_json::json!({
            "id": "B-a", "kind": "build", "project": "corral", "title": record.title,
            "state": "landed", "head": "abc", "landed": "def",
            "override_reason": "Owner accepts the tradeoff"
        })
    );
    assert_eq!(publish(&ctx, &json).unwrap().revision, 2);
    assert_eq!(ctx.store.events().unwrap().len(), events.len());

    let rows = list(&ctx).unwrap();
    assert_eq!(rows, vec![record.summary(2)]);
    assert_eq!(rows[0].gate_attempts, 1);
    assert_eq!(rows[0].gate_failures, 1);
    assert_eq!(rows[0].review_rounds, 1);
    assert_eq!(rows[0].messages, 1);
    let mut older = record.clone();
    older.id = "A-1-scratch".into();
    older.kind = corral_core::build_record::BuildKind::Assistant;
    older.updated = "2026-11-15T11:30:00+02:00".into();
    publish(&ctx, &serde_json::to_string(&older).unwrap()).unwrap();
    assert_eq!(
        list(&ctx)
            .unwrap()
            .iter()
            .map(|r| r.id.as_str())
            .collect::<Vec<_>>(),
        vec!["B-a", "A-1-scratch"]
    );
    assert!(ctx.store.list().unwrap().is_empty());
    assert!(
        show(&ctx, "B-missing")
            .unwrap_err()
            .to_string()
            .contains("B-missing")
    );
}

#[test]
fn publishing_cannot_reassign_an_existing_records_repository() {
    let temp = tempfile::tempdir().unwrap();
    let ctx = Ctx::from_store(Store::init(temp.path()).unwrap());
    let original = record();
    publish(&ctx, &serde_json::to_string(&original).unwrap()).unwrap();
    let events = ctx.store.events().unwrap().len();
    let mut changed = original.clone();
    changed.repo_root = "/another-repo".into();
    changed.outcome.state = OutcomeState::Kept;
    let error = publish(&ctx, &serde_json::to_string(&changed).unwrap())
        .err()
        .unwrap();
    assert!(format!("{error:#}").contains("repo_root"));
    assert_eq!(show(&ctx, &original.id).unwrap(), original);
    assert_eq!(list(&ctx).unwrap()[0].revision, 1);
    assert_eq!(ctx.store.events().unwrap().len(), events);
}
