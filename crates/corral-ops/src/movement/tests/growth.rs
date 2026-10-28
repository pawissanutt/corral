//! A main-movement tick does not replay the approvals accumulated during an open run (T-330
//! review R1-3): once a run's record has a cursor, a steady tick reads only the event tail's
//! position and no approval, so 10x
//! unrelated-project and other-base approvals leave every unattributed cell unchanged.
use corral_core::work::{self, Counter, HotScope};

use super::*;

fn approvals(fx: &Fx, n: usize) {
    for k in 0..n {
        for (project, base) in [("another-project", "main"), ("app", "another-base")] {
            fx.ctx
                .store
                .log_event(
                    None,
                    "approved",
                    json!({"project": project, "base": base, "run": "R1", "sha": format!("{k:040}"),
                           "previous_sha": format!("{:040}", k + 1), "strategy": "fast_forward"}),
                )
                .unwrap();
        }
    }
}

fn steady_tick(fx: &Fx, secs: i64) -> Vec<(&'static str, u64)> {
    let _hot = HotScope::enter();
    let before = work::snapshot();
    fx.tick(secs);
    let delta = work::snapshot().since(&before);
    assert_eq!(delta.unattributed_hydrations(), 0, "{:?}", delta.sites);
    assert_eq!(
        delta.get(Counter::EventDecodes),
        1,
        "a steady tick reads only the event tail's position, no approval"
    );
    Counter::ALL
        .iter()
        .map(|c| (c.name(), delta.cells(*c).unattributed))
        .collect()
}

#[test]
fn a_steady_tick_reads_no_approval_accumulated_during_the_run() {
    let fx = Fx::new();
    fx.tick(0);
    approvals(&fx, 1);
    fx.tick(1);
    let base = steady_tick(&fx, 2);
    approvals(&fx, 9);
    fx.tick(3);
    assert_eq!(base, steady_tick(&fx, 4));
    // Nothing here concerned the run: no landing was queued and no notice sent.
    let state = fx.state();
    assert!(state.pending.is_empty() && state.notices.is_empty());
    assert!(fx.sent().is_empty());
}
