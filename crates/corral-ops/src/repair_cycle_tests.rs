use super::*;
use corral_core::{CompletionDriver, Run, Status};
use serde_json::json;

#[test]
fn issued_cycle_keeps_findings_and_one_round_across_transfers_and_run_replacement() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let task = store
        .create("candidate", None, Status::Review, "scope")
        .unwrap();
    let run: Run = serde_json::from_value(json!({"id":"R1","agent":"fake", "worktree":root.path(),
        "branch":"b","base":"main","base_sha":"base","started":task.created,"outcome":"done",
        "autopilot":{"stage":"changes","sha":"head","at":task.created}}))
    .unwrap();
    store
        .update(&task.id, |task| {
            task.runs.push(run);
            task.rounds = 1;
            Ok(())
        })
        .unwrap();
    let ctx = Ctx::from_store(store.clone());
    let assistant = CompletionDriver::Assistant {
        session: "A-1".into(),
        scratch: "fix".into(),
    };
    let key = RepairKey {
        cycle: Some(1),
        driver: Some(assistant.clone()),
        task: task.id.clone(),
        run: "R1".into(),
        head: "head".into(),
        base: "base".into(),
        reason: "required findings".into(),
    };
    let mut ordinary = key.clone();
    ordinary.driver = None;
    ordinary.cycle = None;
    let legacy = format!(
        r#"{{"task":"{}","run":"R1","head":"head","base":"base","reason":"required findings"}}"#,
        task.id
    );
    assert_eq!(
        ordinary.digest().unwrap(),
        crate::verification::digest(legacy.as_bytes()),
        "ordinary board preparations must retain their pre-completion receipt identity"
    );
    prepare(&ctx, &key, 1, "findings", "precise required fixes").unwrap();
    store
        .update(&task.id, |task| issue(&store, task, &key, 1))
        .unwrap();
    for (run, driver) in [
        ("R1", CompletionDriver::Board),
        ("R2", assistant),
        ("R2", CompletionDriver::Board),
    ] {
        let active_worker = run == "R2" && matches!(&driver, CompletionDriver::Board);
        let task = store.update(&task.id, |task| {
            task.status = if active_worker { Status::Running } else { Status::NeedsMe };
            task.current_run_mut().unwrap().outcome = if active_worker { None } else { Some(corral_core::RunOutcome::Done) };
            if task.current_run().unwrap().id != run {
                let mut replacement = task.current_run().unwrap().clone();
                replacement.id = run.into();
                task.runs.push(replacement);
            }
            task.cowork = Some(serde_json::from_value(json!({"session":"A-1","scratch":"fix","run":"R1",
                "review":"completion_v1","owner_confirmed":true,"completion":{"version":1,
                "contract_revision":task.contract_revision()?,"intent":"ship","driver":driver,"instruction":"ship"}}))?);
            Ok(())
        }).unwrap();
        assert_eq!(task.holds_dispatch_slot(), active_worker);
        assert_eq!(
            store.head(&task.id).unwrap().holds_dispatch_slot(),
            active_worker
        );
        let cycle = pending(&store.reopen().unwrap(), &task).unwrap().unwrap();
        assert_eq!(
            (cycle.round, cycle.details.as_str()),
            (1, "precise required fixes")
        );
        let transferred = RepairKey {
            driver: Some(driver),
            run: run.into(),
            ..key.clone()
        };
        assert_eq!(key.digest().unwrap(), transferred.digest().unwrap());
        store
            .update(&task.id, |task| issue(&store, task, &transferred, 1))
            .unwrap();
        assert_eq!(store.get(&task.id).unwrap().rounds, 1);
        assert!(
            pending(&store, &store.get(&task.id).unwrap())
                .unwrap()
                .is_none()
        );
    }
    // A real author submission ends the cycle even with no code change. Repeating its
    // finding after that must consume another bounded cycle, unlike transfer alone.
    let task = store
        .update(&task.id, |task| {
            task.status = Status::Review;
            task.current_run_mut().unwrap().outcome = Some(corral_core::RunOutcome::Done);
            submitted(&store, task)
        })
        .unwrap();
    let facts = RepairKey {
        cycle: None,
        driver: Some(CompletionDriver::Board),
        run: "R2".into(),
        ..key.clone()
    };
    let (next, reused) = bind(&store, &facts, task.rounds).unwrap();
    assert!(reused.is_none());
    assert_eq!(next.cycle, Some(2));
    assert_ne!(key.digest().unwrap(), next.digest().unwrap());
    prepare(&ctx, &next, 2, "findings", "still unresolved").unwrap();
    store
        .update(&task.id, |task| {
            task.rounds = 2;
            issue(&store, task, &next, 2)
        })
        .unwrap();
    assert_eq!(store.get(&task.id).unwrap().rounds, 2);
}
