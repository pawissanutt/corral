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
        driver: Some(assistant.clone()),
        task: task.id.clone(),
        run: "R1".into(),
        head: "head".into(),
        base: "base".into(),
        reason: "required findings".into(),
    };
    let mut ordinary = key.clone();
    ordinary.driver = None;
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
            task.current_run_mut().unwrap().id = run.into();
            task.cowork = Some(serde_json::from_value(json!({"session":"A-1","scratch":"fix","run":"R1",
                "review":"required","owner_confirmed":true,"completion":{"version":1,
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
}
