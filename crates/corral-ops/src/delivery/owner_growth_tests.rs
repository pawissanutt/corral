//! One owner delivery pass on an inbox run holding 1x and 10x earlier applied effects and
//! observe cursors (plan T-253 section 4, nested growth): the pass reads the unfinished
//! messages, the effect applied per message and the cursor per binding, so it delivers the same
//! and does the same work at both sizes.
use corral_core::Status;
use corral_core::inbox::Inbox;
use corral_core::work::{self, Counter, HotScope};
use serde_json::json;

use super::*;
use crate::delivery::omo_fixture::OmoWorld;

/// An inbox run whose `earlier` messages' effects were applied and whose earlier bindings left
/// cursors, with one owner reply waiting for delivery.
fn world(earlier: u64) -> OmoWorld {
    let world = OmoWorld::new(Status::NeedsMe);
    // Every stamp written from here on, including the pass's own receipts, is this instant.
    let now: jiff::Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
    world.ctx.store.fixture_pin_now(now).unwrap();
    world
        .ctx
        .store
        .update(&world.task.id, |task| {
            task.created = now;
            task.status_since = Some(now);
            let mut inbox = serde_json::to_value(&task.runs[0].inbox)?;
            inbox["next_seq"] = json!(1000);
            inbox["effects_applied"] = json!((1..=earlier).collect::<Vec<_>>());
            inbox["observe_cursors"] = (0..earlier)
                .map(|c| {
                    json!({"adapter": "pi", "binding": {"kind": "pi", "session": format!("s-{c}")},
                           "cursor": {"offset": c, "last_complete_line": c, "coverage": "complete"}})
                })
                .collect();
            task.runs[0].inbox = serde_json::from_value(inbox)?;
            Ok(())
        })
        .unwrap();
    // Complete actual bounded legacy identity indexing during cold fixture preparation.
    // Keep every earlier effect/cursor; the measured pass must never bypass this guard.
    let inbox = Inbox::new(&world.ctx.store, world.recipient()).unwrap();
    let mut indexed = false;
    for _ in 0..32 {
        if inbox.index_owner(64).unwrap().legacy_debt.is_none() {
            indexed = true;
            break;
        }
    }
    assert!(
        indexed,
        "both phases index the actual 999 reserved legacy slots"
    );
    // Post without the gateway, so the measured pass is the one that delivers.
    let registry = world.temp.path().join("agent/rpc-host-daemon");
    let hidden = world.temp.path().join("agent/registry-hidden");
    std::fs::rename(&registry, &hidden).unwrap();
    crate::reply(&world.ctx, &world.task.id, "use the second option").unwrap();
    std::fs::rename(&hidden, &registry).unwrap();
    world
}

/// Every unattributed cell of the pass, stored bytes included: the fixture's clock is pinned.
fn pass(world: &OmoWorld) -> Vec<(&'static str, u64)> {
    let _hot = HotScope::enter();
    let before = work::snapshot();
    reconcile_all_with(&world.ctx, &world.adapter()).unwrap();
    let delta = work::snapshot().since(&before);
    assert!(delta.sites.is_empty(), "hydrations at {:?}", delta.sites);
    assert!(delta.check_identity().is_ok(), "{delta:?}");
    Counter::ALL
        .iter()
        .map(|c| (c.name(), delta.cells(*c).unattributed))
        .collect()
}

fn delivered(world: &OmoWorld) -> (usize, Option<String>, Status, Vec<bool>) {
    let task = world.ctx.store.get(&world.task.id).unwrap();
    let inbox = Inbox::new(&world.ctx.store, world.recipient()).unwrap();
    let applied = inbox
        .messages()
        .unwrap()
        .iter()
        .map(|message| inbox.effect_applied(message.seq).unwrap())
        .collect();
    (
        world.sends().len(),
        task.runs[0].pending_reply.clone(),
        task.status,
        applied,
    )
}

#[test]
fn a_delivery_pass_does_not_grow_with_applied_effects_and_earlier_cursors() {
    let (one, ten) = (world(1), world(10));
    assert_eq!(pass(&one), pass(&ten));
    let outcome = delivered(&one);
    assert_eq!(outcome, delivered(&ten));
    assert_eq!(outcome.0, 1, "the pass delivers the waiting reply once");
    assert_eq!(outcome.3, [true], "and applies its effect");
}
