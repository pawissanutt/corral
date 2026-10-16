use super::*;

#[test]
fn doctor_cli_reports_unfinished_terminal_runs_without_repairing_them() {
    // Given terminal tasks with missing end/outcome and a working agent state.
    let s = Sandbox::new();
    let store = corral_core::Store::open(&s.root).unwrap();
    let mut ids = Vec::new();
    for status in [
        corral_core::Status::Done,
        corral_core::Status::Dropped,
        corral_core::Status::Running,
    ] {
        let task = store.create("unfinished", None, status, "").unwrap();
        store
            .update(&task.id, |task| {
                task.runs.push(serde_json::from_value(serde_json::json!({
                    "id":"R1", "agent":"generic", "worktree":s.root.join("missing"),
                    "branch":"corral/fixture", "base":"main", "base_sha":"abc",
                    "started":"2026-10-15T00:00:00Z", "agent_state":"working"
                }))?);
                Ok(())
            })
            .unwrap();
        ids.push(task.id);
    }
    let before = serde_json::to_value(store.list().unwrap()).unwrap();

    // When the public CLI inspects the stored task records.
    let out = s.run(&["doctor", "--json"]);

    // Then JSON contains a non-startup-blocking warning identifying both terminal runs.
    let report = report(&out);
    let warning = check(&report, "terminal_runs");
    assert_eq!(warning["ok"], false);
    assert_eq!(warning["hard"], false);
    let message = warning["message"].as_str().unwrap();
    for id in &ids[..2] {
        assert!(message.contains(id), "{message}");
    }
    assert!(!message.contains(&ids[2]));
    assert!(message.contains("R1"));
    assert!(message.contains("ended"));
    assert!(message.contains("outcome"));
    assert!(message.contains("agent_state=working"));
    assert_eq!(serde_json::to_value(store.list().unwrap()).unwrap(), before);
}
