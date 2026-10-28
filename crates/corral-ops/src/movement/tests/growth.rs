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

#[test]
fn a_later_positioned_earlier_approval_does_not_hide_a_landing_from_a_new_run() {
    // Given a relevant landing after the run started, then, later in the event sequence, an
    // approval stamped before the run started.
    let fx = Fx::new();
    fx.worker_change("file.rs");
    let landing = fx.land("file.rs", "fn landed() {}\n");
    let earlier = corral_core::Event {
        seq: 0,
        ts: fx.now - SignedDuration::from_hours(24),
        task: Some("T-999".into()),
        kind: "approved".into(),
        actor: None,
        via: None,
        detail: json!({"project": "app", "base": "main", "run": "R1", "sha": "f".repeat(40),
                       "previous_sha": "e".repeat(40), "strategy": "fast_forward"}),
    };
    fx.ctx
        .store
        .fixture_append_events(&(serde_json::to_string(&earlier).unwrap() + "\n"))
        .unwrap();
    // When the run's first tick reads its window.
    fx.tick(0);
    // Then the landing is observed for the run.
    let state = fx.state();
    assert!(state.seen.contains(&landing.sha), "{:?}", state.seen);
    assert_eq!(state.pending.len(), 1);
}
