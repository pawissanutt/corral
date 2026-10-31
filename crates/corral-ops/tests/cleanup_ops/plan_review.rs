use super::*;

#[test]
fn sync_done_reaps_every_run_including_old_gate_and_reviewer() {
    terminal_history(Status::Done);
}

#[test]
fn sync_dropped_reaps_every_run_including_old_gate_and_reviewer() {
    terminal_history(Status::Dropped);
}

#[test]
fn finished_task_sweep_keeps_pending_plan_reviewer_and_logs_closed_review() {
    let fx = Fx::new();
    let socket = fx.ctx.herdr.socket_path().unwrap();
    fx.edit(|state| {
        state["workspaces"]["wp"] = json!({"workspace_id":"wp", "label":"plan"});
        for pane in ["wp:pending", "wp:finished"] {
            state["panes"][pane] = json!({"workspace_id":"wp", "pane_id":pane, "tab_id":"wp:t1", "agent_status":"idle"});
        }
    });
    fx.ctx.store.update(&fx.id, |task| {
        task.status = Status::Done;
        task.plans.push(serde_json::from_value(json!({
            "id":"P1", "spec_revision":"fixture",
            "artifact":{"run":"P1","kind":"plan","title":"plan","path":"plan.md", "source":"plan.md","sha256":"sha","bytes":0,"added":"2026-10-15T11:00:00Z"},
            "document":{"goal":"fixture","non_goals":[],"decisions":[],"affected_paths":[],"contracts":[],"dependencies":[],"acceptance":[],"verification":[],"owner_decisions":[]},
            "reviews":[
                {"id":"PR1","reviewer":"rev","policy_digest":"x","at":"2026-10-15T11:00:00Z","verdict":"changes","herdr":{"workspace_id":"wp","pane_id":"wp:finished","socket":socket}},
                {"id":"PR2","reviewer":"rev","policy_digest":"x","at":"2026-10-15T11:00:00Z","herdr":{"workspace_id":"wp","pane_id":"wp:pending","socket":socket}}
            ]
        }))?);
        Ok(())
    }).unwrap();
    fx.sync();
    assert!(fx.pane_exists("wp:pending"));
    assert!(!fx.pane_exists("wp:finished"));
    assert!(fx.workspace_exists("wp"));
    assert!(fx.ctx.store.events().unwrap().iter().any(|event| event.kind
        == "cleanup_review_closed"
        && event.detail["pane"] == "wp:finished"));
}
