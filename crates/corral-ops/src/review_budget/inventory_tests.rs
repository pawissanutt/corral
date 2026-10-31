//! The inventory ownership lease excludes competing mutations.
use super::*;

#[test]
fn sampled_inventory_excludes_ownership_changes_until_consumption_finishes() {
    // Given an unowned store with a retained, nonempty inventory response.
    let mut fx = Fixture::new(1);
    fx.ctx.herdr.set_session(Some("owner".into()));
    let id = fx.task();
    let inventory = crate::pane_liveness::inventory(&fx.ctx).unwrap();
    let before = serde_json::to_value(fx.ctx.store.list().unwrap()).unwrap();
    let events = fx.ctx.store.events().unwrap();

    // When binding or an auxiliary launch would change ownership during consumption.
    assert!(
        fx.ctx
            .store
            .bind_herdr_session(Path::new("/foreign/herdr.sock"), "foreign record")
            .is_err()
    );
    assert!(ReviewLaunch::reserve(&fx.ctx, &id, Kind::Gate).is_err());

    // Then neither admission wrote anything; the owner can launch after consumption ends.
    assert_eq!(
        serde_json::to_value(fx.ctx.store.list().unwrap()).unwrap(),
        before
    );
    assert_eq!(fx.ctx.store.events().unwrap(), events);
    assert!(load_pending(&fx.ctx).unwrap().is_none());
    assert!(fx.ctx.store.herdr_session_binding().unwrap().is_none());
    drop(inventory);
    crate::gate::gate_start(&fx.ctx, &id).unwrap();
}
