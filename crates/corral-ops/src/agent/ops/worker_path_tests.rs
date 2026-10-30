use super::*;
use corral_core::RunOutcome;
use corral_core::work::{self, Counter, HotScope};

#[test]
fn artifact_paths_key_current_and_historical_runs_without_history_growth() {
    let mut measured = Vec::new();
    for scale in [1, 10] {
        let fx = crate::agent::test_support::fx();
        let id = crate::agent::test_support::worker_task(&fx, "p1");
        let mut task = fx.ctx.store.get(&id).unwrap();
        let at = "2026-01-01T00:00:00Z".parse().unwrap();
        task.created = at;
        task.updated = at;
        task.status_since = Some(at);
        let current = task.current_run().unwrap().clone();
        for n in 0..2 * scale {
            let mut old = current.clone();
            old.id = format!("old-{n:04}");
            old.outcome = Some(RunOutcome::Done);
            old.ended = Some(at);
            old.inbox.settled = Some(at);
            task.runs.insert(0, old);
        }
        fx.ctx.store.fixture_put_task(&task).unwrap();
        let absolute = fx.ctx.store.root().join("absolute");
        let _hot = HotScope::enter();
        let before = work::snapshot();
        assert_eq!(
            artifact_path(&fx.ctx, &id, "absent", absolute.clone()).unwrap(),
            absolute
        );
        for run in ["R1", "old-0000"] {
            assert_eq!(
                artifact_path(&fx.ctx, &id, run, "proof.txt".into()).unwrap(),
                current.worktree.join("proof.txt")
            );
        }
        let after = work::snapshot();
        measured.push(
            [
                Counter::TaskHeadDecodes,
                Counter::NestedRowDecodes,
                Counter::HistoryRowDecodes,
                Counter::FullTaskHydrations,
                Counter::DecodedDocBytes,
                Counter::ViewsOpened,
            ]
            .map(|counter| after.total(counter) - before.total(counter)),
        );
    }
    assert_eq!(measured[0], measured[1]);
    assert_eq!(measured[0][3], 0);
}
