use super::*;

#[test]
fn coding_reservation_blocks_next_claim_until_landing() {
    let mut fx = Fixture::new();
    fx.ctx.store.config.limits.wip = 1;
    fx.save();
    let first = fx.task("first", Status::Ready);
    let second = fx.task("second", Status::Ready);
    let backlog = fx.task("backlog", Status::Backlog);
    fx.ctx
        .store
        .update(&first.id, |t| {
            let run: Run = serde_json::from_value(serde_json::json!({
                "id":"R1", "agent":"fake", "worktree":"/tmp/w", "branch":"b",
                "base":"main", "base_sha":"head", "started":Fixture::now()
            }))
            .unwrap();
            t.runs.push(run);
            t.launch = Some(corral_core::LaunchReservation {
                authorization: None,
                approved_plan: None,
                socket: None,
                stage: corral_core::LaunchStage::Reserved,
                reserved: true,
                error: None,
            });
            Ok(())
        })
        .unwrap();
    let backlog = fx.ctx.store.get(&backlog.id).unwrap();
    assert_eq!(
        crate::queue::blocked_reason(&fx.ctx, &fx.ctx.store, &backlog, false).unwrap(),
        None
    );
    fx.grant(&[&backlog]);
    advance(&fx.ctx, Fixture::now(), &crate::sync::SyncScope::All).unwrap();
    assert_eq!(fx.ctx.store.get(&backlog.id).unwrap().status, Status::Ready);
    let candidate = fx.ctx.store.get(&second.id).unwrap();
    assert_eq!(
        crate::wip::reason(&fx.ctx.store, &candidate)
            .unwrap()
            .as_deref(),
        Some("waiting: work in progress 1/1")
    );
    let check = || {
        let head = fx.ctx.store.head(&second.id).unwrap();
        crate::dispatch::admission::Admission::Manual
            .reason(&fx.ctx, &fx.ctx.store, &head, &DispatchOpts::default())
            .unwrap()
    };
    assert_eq!(check().as_deref(), Some("waiting: work in progress 1/1"));
    fx.ctx
        .store
        .update(&first.id, |t| {
            t.launch = None;
            t.status = Status::Review;
            Ok(())
        })
        .unwrap();
    assert!(
        crate::wip::reason(&fx.ctx.store, &candidate)
            .unwrap()
            .is_some()
    );
    fx.ctx
        .store
        .update(&first.id, |t| {
            t.status = Status::Done;
            Ok(())
        })
        .unwrap();
    assert_eq!(crate::wip::reason(&fx.ctx.store, &candidate).unwrap(), None);
    assert_eq!(check(), None);
}
