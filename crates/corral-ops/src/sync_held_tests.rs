use super::*;
use corral_core::{HerdrRef, Store};

#[test]
fn held_run_reconciliation_closes_only_gone_or_idle_backlog_runs() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("app/.git")).unwrap();
    let ctx = crate::resources::FixedResourceProbe::ctx(Store::init(tmp.path()).unwrap());
    let now: Timestamp = "2026-10-30T00:00:00Z".parse().unwrap();
    for (status, held, parked, pane_status, closes) in [
        (Status::Backlog, true, false, None, true),
        (Status::Backlog, true, false, Some(AgentStatus::Idle), true),
        (
            Status::Backlog,
            true,
            false,
            Some(AgentStatus::Working),
            false,
        ),
        (Status::Backlog, false, false, None, false),
        (Status::Running, true, false, None, false),
        (Status::Running, true, true, Some(AgentStatus::Idle), false),
        (Status::Review, true, false, Some(AgentStatus::Idle), false),
    ] {
        let task = ctx
            .store
            .create("test", Some("app"), status, "body")
            .unwrap();
        let run: Run = serde_json::from_value(json!({
            "id":"R1", "agent":"generic", "worktree":"/tmp/unused", "branch":"main",
            "base":"main", "base_sha":"0", "started":now,
        }))
        .unwrap();
        let task = ctx
            .store
            .update(&task.id, |t| {
                t.dispatch_hold = held;
                let mut run = run.clone();
                if parked {
                    run.obligations.slot_wait = Some(corral_core::SlotWait {
                        since: now,
                        on: vec!["T-999".into()],
                    });
                }
                run.herdr = Some(HerdrRef {
                    workspace_id: "w".into(),
                    pane_id: "p".into(),
                    socket: None,
                });
                t.runs.push(run);
                Ok(())
            })
            .unwrap();
        let mut panes = HashMap::new();
        if let Some(status) = pane_status {
            panes.insert(
                "p".into(),
                PaneInfo {
                    pane_id: "p".into(),
                    workspace_id: "w".into(),
                    tab_id: "t".into(),
                    agent: None,
                    status,
                    title: None,
                    cwd: None,
                },
            );
        }
        let events = ctx.store.events().unwrap().len();
        close_held_run(&ctx, &task, &panes, now).unwrap();
        let saved = ctx.store.get(&task.id).unwrap();
        let run = saved.current_run().unwrap();
        assert_eq!(run.outcome == Some(RunOutcome::Abandoned), closes);
        assert_eq!(run.ended, closes.then_some(now));
        assert_eq!(
            ctx.store.events().unwrap().len(),
            events + usize::from(closes)
        );
        close_held_run(&ctx, &saved, &panes, now).unwrap();
        assert_eq!(
            ctx.store.events().unwrap().len(),
            events + usize::from(closes)
        );
    }
}
