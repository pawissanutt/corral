use super::*;

fn policy(limit: u32) -> RepairPolicy {
    RepairPolicy::V1 {
        current_profile_limit: limit,
        stronger_profile_limit: 0,
        overall_limit: limit,
        pm_diagnosis_allowance: 0,
    }
}

// This is the admission function used by launch::submit's final durable prepare,
// not a serialization-only test. IPC is permitted only after that transaction commits.
fn launch_next(ctx: &Ctx, id: &str) -> Result<()> {
    ctx.store.update(id, |task| {
        let previous = task.current_run().unwrap().id.clone();
        prepare_continuation(&ctx.store, task, &previous, false)?;
        let mut next = task.current_run().unwrap().clone();
        next.id = format!("R{}", task.runs.len() + 1);
        task.runs.push(next);
        start_launch(&ctx.store, task)
    })?;
    Ok(())
}

#[test]
fn pm_renewed_continuation_charges_third_before_ipc_and_refuses_fourth() {
    let _clock = FixedClock::new();
    let (_root, mut ctx, id) = fixture();
    ctx.store.config.routing.repair = Some(policy(2));
    ctx.store.save_config().unwrap();
    begin(&ctx, &id, "launch:R1", "general", false).unwrap();
    repair_finding(&ctx, &id, report("first", "rollback-test")).unwrap();
    launch_next(&ctx, &id).unwrap(); // first charged repair submitted as R2
    launch_next(&ctx, &id).unwrap(); // second charged repair submitted as R3
    let exhausted = repair_status(&ctx, &id).unwrap().unwrap();
    assert_eq!(
        exhausted
            .active()
            .unwrap()
            .history
            .counters()
            .unwrap()
            .coding_repairs,
        2
    );
    let renewal = corral_core::RepairRenewal {
        id: "fixture-pm-renewal".into(),
        previous_digest: exhausted.digest().unwrap(),
        reason: "one corrective continuation".into(),
        approved_by: "owner".into(),
        at: ctx.store.now(),
    };
    let mut invoker = crate::command::Invoker::process();
    invoker.owner_action = None;
    invoker.caller.actor = Some(corral_protocol::cmd::ActorName::Pm);
    invoker.caller.cwd = Some(ctx.store.root().to_string_lossy().into_owned());
    invoker.caller.env = corral_protocol::cmd::CallerEnv {
        corral_pm: Some("1".into()),
        ai_agent: Some("pi".into()),
        ..Default::default()
    };
    let renewed = repair_renew(&ctx, &id, renewal, policy(1), &invoker).unwrap();
    assert_eq!(
        renewed
            .active()
            .unwrap()
            .renewal
            .as_ref()
            .unwrap()
            .approved_by,
        "pm"
    );
    // A second PM renewal is the owner's, whatever its size.
    let again = corral_core::RepairRenewal {
        id: "fixture-pm-renewal-again".into(),
        previous_digest: renewed.digest().unwrap(),
        reason: "another continuation".into(),
        approved_by: "owner".into(),
        at: ctx.store.now(),
    };
    let refused = repair_renew(&ctx, &id, again, policy(1), &invoker).unwrap_err();
    assert!(
        refused.to_string().contains("pm_renewal_refused"),
        "{refused:#}"
    );
    launch_next(&ctx, &id).unwrap();
    // Inspect the committed charge at exactly the boundary where pane_run could start.
    let at_ipc = repair_status(&ctx, &id).unwrap().unwrap();
    assert_eq!(
        at_ipc
            .active()
            .unwrap()
            .history
            .counters()
            .unwrap()
            .coding_repairs,
        1,
        "the first renewed writer must be charged before IPC, never a second initial"
    );
    assert_eq!(at_ipc.allowances[0], exhausted.allowances[0]);
    assert_eq!(
        at_ipc
            .active()
            .unwrap()
            .history
            .counters()
            .unwrap()
            .initial_implementations,
        0
    );
    assert!(launch_next(&ctx, &id).is_err());
    assert_eq!(repair_status(&ctx, &id).unwrap().unwrap(), at_ipc);
    assert_eq!(ctx.store.get(&id).unwrap().runs.len(), 4);
}
