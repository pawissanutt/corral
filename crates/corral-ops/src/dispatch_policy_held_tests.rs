use super::*;
use corral_core::{HerdrRef, RunOutcome};

#[test]
fn sync_closes_held_runs_for_missing_and_idle_panes() {
    let fx = Fixture::new();
    std::fs::write(fx._tmp.path().join("engine/visible"), "").unwrap();
    for (pane, closes) in [("w1:p1", true), ("missing", true), ("w0:p1", false)] {
        let task = fx.task(pane, Status::Backlog);
        fx.ctx
            .store
            .update(&task.id, |t| {
                t.dispatch_hold = true;
                let mut run: Run = serde_json::from_value(serde_json::json!({
                    "id": "R1", "agent": "fake", "worktree": "/tmp/unused", "branch": "main",
                    "base": "main", "base_sha": "0", "started": Fixture::now(),
                }))
                .unwrap();
                run.herdr = Some(HerdrRef {
                    workspace_id: "w1".into(),
                    pane_id: pane.into(),
                    socket: Some(fx.ctx.herdr.socket_path().unwrap()),
                });
                t.runs.push(run);
                Ok(())
            })
            .unwrap();
        crate::sync_at(&fx.ctx, Fixture::now()).unwrap();
        let saved = fx.ctx.store.get(&task.id).unwrap();
        assert_eq!(
            saved.current_run().unwrap().outcome == Some(RunOutcome::Abandoned),
            closes,
            "{pane}"
        );
    }
}
