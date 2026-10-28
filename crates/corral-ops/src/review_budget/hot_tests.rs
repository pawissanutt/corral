//! Reviewer liveness on ordinary sync ticks does not read the review child's nested history
//! (T-330 review R1-2): the idle-only watchdog update writes the child's projection, and a
//! lost reviewer's finish collects the child's usage as own-record work of its run. With 10x
//! usage entries and switches in the current child, every unattributed cell is unchanged.
use corral_core::work::{self, Counter, HotScope};
use corral_herdr::{AgentStatus, PaneInfo};

use super::*;

fn at(secs: i64) -> Timestamp {
    Timestamp::from_second(1_800_000_000 + secs).unwrap()
}

/// `n` zero-cost usage entries and switches in the current review child, so the child's
/// usage totals are equal at every size; created, updated and the review start are fixed so
/// equal states store equal bytes.
fn grow_child(ctx: &Ctx, id: &str, n: usize) {
    let mut task = ctx.store.get(id).unwrap();
    let run = task.current_run_mut().unwrap();
    run.review.as_mut().unwrap().at = at(0);
    let child = run.review_runs.last_mut().unwrap();
    child.started = at(0);
    let entries: Vec<corral_core::UsageEntry> = (0..n)
        .map(|_| {
            serde_json::from_value(json!({"at": null, "latency_ms": null, "model": "m",
                "input": 0, "output": 0, "cache_read": 0, "cache_write": 0,
                "reported_usd": null, "estimated_usd": null, "unpriced_tokens": 0}))
            .unwrap()
        })
        .collect();
    child.usage = Some(corral_core::Usage::from_entries(entries));
    child.switches = (0..n)
        .map(|i| {
            serde_json::from_value(
                json!({"from": format!("p{i:04}"), "to": format!("q{i:04}"), "reason": "switch"}),
            )
            .unwrap()
        })
        .collect();
    task.created = at(0);
    task.updated = at(0);
    ctx.store.fixture_put_task(&task).unwrap();
}

fn reviewing(n: usize) -> (Fixture, String, String) {
    let fx = Fixture::new(1);
    let id = fx.task();
    let review = crate::reviewer::review_run(&fx.ctx, &id, Some("rev")).unwrap();
    grow_child(&fx.ctx, &id, n);
    (fx, id, review.pane.unwrap())
}

fn measured(f: impl FnOnce()) -> (Vec<(&'static str, u64)>, u64) {
    let _hot = HotScope::enter();
    let before = work::snapshot();
    f();
    let delta = work::snapshot().since(&before);
    assert_eq!(delta.unattributed_hydrations(), 0, "{:?}", delta.sites);
    let cells = Counter::ALL
        .iter()
        .map(|c| (c.name(), delta.cells(*c).unattributed))
        .collect();
    (cells, delta.own_record_rows())
}

fn idle_panes(pane: &str) -> std::collections::HashMap<String, PaneInfo> {
    let info = PaneInfo {
        pane_id: pane.into(),
        workspace_id: "w1".into(),
        tab_id: "w1:t1".into(),
        agent: None,
        status: AgentStatus::Idle,
        title: None,
        cwd: None,
    };
    std::collections::HashMap::from([(pane.to_owned(), info)])
}

fn idle_tick(fx: &Fixture, id: &str, pane: &str) -> Vec<(&'static str, u64)> {
    let panes = idle_panes(pane);
    let snapshot = crate::pane_liveness::Snapshot::injected(&panes);
    let (cells, own) = measured(|| {
        let change = crate::reviewer::reconcile_review_sampled_at(&fx.ctx, id, at(1), snapshot);
        assert!(change.unwrap().is_none());
    });
    assert_eq!(own, 0, "liveness alone reads no child row");
    let run = fx.ctx.store.head(id).unwrap().current_run.unwrap();
    assert_eq!(run.idle_since, Some(at(1)));
    let child = run.current_child.unwrap();
    assert_eq!(
        (child.idle_since, child.agent_state.as_deref()),
        (Some(at(1)), Some("idle"))
    );
    cells
}

#[test]
fn an_idle_reviewer_update_does_not_read_the_child_history() {
    let (fx, id, pane) = reviewing(1);
    let base = idle_tick(&fx, &id, &pane);
    // The same idle transition again, now with 10x child history.
    grow_child(&fx.ctx, &id, 10);
    fx.ctx
        .store
        .update(&id, |t| {
            let run = t.current_run_mut().unwrap();
            run.idle_since = None;
            let child = run.review_runs.last_mut().unwrap();
            child.idle_since = None;
            child.agent_state = None;
            Ok(())
        })
        .unwrap();
    grow_child(&fx.ctx, &id, 10);
    assert_eq!(base, idle_tick(&fx, &id, &pane));
}

/// The ticks that release a reviewer whose pane is gone, through its finish.
fn release_tick(n: usize) -> (Vec<(&'static str, u64)>, u64) {
    let (fx, id, _) = reviewing(n);
    // An inventory without the reviewer's pane (an empty one is not a sample).
    let mut none = idle_panes("w1:p99");
    none.remove("w1:p1");
    let out = measured(|| {
        reconcile(&fx.ctx, &none, at(600)).unwrap();
        reconcile(&fx.ctx, &none, at(1200)).unwrap();
    });
    let task = fx.ctx.store.get(&id).unwrap();
    let child = task.current_run().unwrap().review_runs.last().unwrap();
    assert!(child.ended.is_some(), "the lost reviewer was finished");
    assert!(
        task.current_run()
            .unwrap()
            .review
            .as_ref()
            .unwrap()
            .process_lost
    );
    out
}

#[test]
fn a_lost_reviewer_finish_reads_the_child_only_as_own_record_work() {
    let (base, base_own) = release_tick(1);
    let (grown, grown_own) = release_tick(10);
    assert_eq!(base, grown);
    assert!(
        grown_own > base_own,
        "the child's usage is collected from its own rows"
    );
}
