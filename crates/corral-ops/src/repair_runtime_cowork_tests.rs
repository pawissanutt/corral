use super::*;
use crate::integration::RepairKey;
use corral_core::{AutoStage, AutopilotState, CompletionDriver, RunOutcome};

#[test]
fn transferred_cycle_continues_on_policy_profile_without_another_coding_charge() {
    let _clock = FixedClock::new();
    let (_root, ctx, id) = fixture();
    begin(&ctx, &id, "launch:R1", "general", false).unwrap();
    repair_finding(&ctx, &id, report("defect", "rollback-test")).unwrap();
    begin(&ctx, &id, "earlier-cycle", "general", false).unwrap();
    let key = RepairKey {
        cycle: Some(1),
        task: id.clone(),
        run: "R1".into(),
        head: "candidate".into(),
        base: "base".into(),
        reason: "required fixes".into(),
        driver: Some(CompletionDriver::Assistant {
            session: "A-1".into(),
            scratch: "work".into(),
        }),
    };
    crate::repair_cycle::prepare(&ctx, &key, 2, "required fixes", "preserve exact findings")
        .unwrap();
    begin(
        &ctx,
        &id,
        &format!("repair:{}", key.digest().unwrap()),
        "general",
        false,
    )
    .unwrap();
    let before = repair_status(&ctx, &id).unwrap().unwrap();
    assert_eq!(
        before
            .active()
            .unwrap()
            .history
            .counters()
            .unwrap()
            .coding_repairs,
        2
    );
    for driver in [
        CompletionDriver::Board,
        CompletionDriver::Assistant {
            session: "A-2".into(),
            scratch: "adopted".into(),
        },
    ] {
        ctx.store.update(&id, |task| {
            task.rounds = 2;
            let previous = task.current_run().unwrap().id.clone();
            let transferred = RepairKey { driver:Some(driver.clone()), run:previous.clone(), ..key.clone() };
            crate::repair_cycle::issue(&ctx.store, task, &transferred, 2)?;
            task.cowork = Some(serde_json::from_value(json!({"session":"A-1","scratch":"work","run":"R1", "review":"completion_v1","owner_confirmed":true,
                "completion":{"version":1,"contract_revision":task.contract_revision()?,"intent":"ship","driver":driver,"instruction":"continue this exact scope"}}))?);
            prepare_continuation(&ctx.store, task, &previous, false)?;
            let mut next = task.current_run().unwrap().clone();
            task.current_run_mut().unwrap().outcome = Some(RunOutcome::Superseded);
            next.id = format!("R{}", task.runs.len()+1);
            next.agent = "deep".into(); // next policy stage is eligible; the issued cycle is unchanged
            next.outcome = None;
            next.autopilot = Some(AutopilotState { stage:AutoStage::Changes, sha:Some(key.head.clone()), reason:Some("required fixes".into()), at:ctx.store.now() });
            task.runs.push(next);
            start_launch(&ctx.store, task)
        }).unwrap();
        let reopened = Ctx::from_store(Store::open(ctx.store.root()).unwrap());
        let after = repair_status(&reopened, &id).unwrap().unwrap();
        assert_eq!(
            after, before,
            "a new driver/run must not consume or reset coding allowance/findings"
        );
        assert_eq!(ctx.store.get(&id).unwrap().rounds, 2);
    }
    assert_eq!(
        ctx.store
            .events()
            .unwrap()
            .iter()
            .filter(|e| e.kind == "repair_cycle_continued")
            .count(),
        2
    );
    let task = ctx
        .store
        .update(&id, |task| {
            task.status = corral_core::Status::Review;
            task.current_run_mut().unwrap().outcome = Some(RunOutcome::Done);
            crate::repair_cycle::submitted(&ctx.store, task)
        })
        .unwrap();
    let facts = RepairKey {
        cycle: None,
        driver: RepairKey::driver(&task),
        run: task.current_run().unwrap().id.clone(),
        ..key
    };
    let (next, reused) = crate::repair_cycle::bind(&ctx.store, &facts, task.rounds).unwrap();
    assert!(reused.is_none());
    crate::repair_cycle::prepare(
        &ctx,
        &next,
        3,
        "defect remains",
        "same candidate/finding after author submission",
    )
    .unwrap();
    ctx.store
        .update(&id, |task| {
            task.rounds = 3;
            crate::repair_cycle::issue(&ctx.store, task, &next, 3)
        })
        .unwrap();
    begin(
        &ctx,
        &id,
        &format!("repair:{}", next.digest().unwrap()),
        "deep",
        false,
    )
    .unwrap();
    assert_eq!(
        repair_status(&ctx, &id)
            .unwrap()
            .unwrap()
            .active()
            .unwrap()
            .history
            .counters()
            .unwrap()
            .coding_repairs,
        3,
        "a newly issued cycle after a no-op submission is charged, unlike transfer continuation"
    );
}
