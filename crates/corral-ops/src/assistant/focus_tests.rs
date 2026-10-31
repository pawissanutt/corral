use super::*;

#[test]
fn brief_is_bounded_utf8_context_and_has_all_sections_without_mutation() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::init(temp.path()).unwrap();
    let task = store
        .create(
            "Focused task",
            None,
            corral_core::Status::Backlog,
            &"界".repeat(20000),
        )
        .unwrap();
    store.update(&task.id, |t| {
        t.plan = Some("docs/plans/focused.md".into());
        t.runs.push(serde_json::from_value(serde_json::json!({
            "id":"R1", "agent":"omo", "worktree":"/fixture/project",
            "branch":"test", "base":"main", "base_sha":"abc",
            "started":"2026-09-01T00:00:00Z", "summary":"achieved outcome",
            "review":{"reviewer":"reviewer", "sha":"abc", "at":"2026-09-01T00:00:00Z", "notes":"latest review feedback"}
        })).unwrap());
        Ok(())
    }).unwrap();
    let before = std::fs::read(&task.path).unwrap();
    let text = task_brief(&store, &task.id).unwrap();
    assert!(text.len() <= BRIEF_BYTES);
    for expected in [
        "Spec",
        "Plan summary",
        "Runs, costs and result",
        "Latest review notes",
        "Open questions",
        "Artifact refs",
        "truncated",
        "not authority",
        "--review",
        "achieved outcome",
        "latest review feedback",
        "focused.md",
    ] {
        assert!(text.contains(expected), "missing {expected}");
    }
    assert_eq!(before, std::fs::read(&task.path).unwrap());
}
