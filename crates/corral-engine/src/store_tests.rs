use crate::{
    as_refusal,
    model::{Actor, Check, CheckKind, CheckResult, Outcome, Role},
    store::{SCHEMA_VERSION, Store, TaskFilter},
    tests::{run, task},
};

#[test]
fn task_run_check_roundtrips_and_transaction_rollback() {
    let root = crate::tests::tempdir();
    let store = Store::open(&root.path().join("corral.db")).unwrap();
    store.set_first_id(42).unwrap();
    let mut task = store
        .transaction(|store| {
            let task = task(&store.next_task_id()?);
            store.insert_task(&task)?;
            Ok(task)
        })
        .unwrap();
    assert_eq!(task.id, "T-042");
    assert_eq!(store.get_task(&task.id).unwrap(), Some(task.clone()));
    assert!(
        store
            .transaction::<()>(|store| {
                store.next_task_id()?;
                anyhow::bail!("abort")
            })
            .is_err()
    );
    assert_eq!(
        store.transaction(|store| store.next_task_id()).unwrap(),
        "T-043"
    );
    task.title = "edited".to_owned();
    store.update_task(&task).unwrap();
    assert_eq!(
        store.list_tasks(&TaskFilter::default()).unwrap(),
        [task.clone()]
    );
    assert!(
        store
            .list_tasks(&TaskFilter {
                project: Some("other".to_owned()),
                ..Default::default()
            })
            .unwrap()
            .is_empty()
    );
    let mut run = run(&task.id, &store.next_run_id(&task.id).unwrap());
    store.insert_run(&run).unwrap();
    assert_eq!(store.get_run(&task.id, "R1").unwrap(), Some(run.clone()));
    assert_eq!(store.list_runs(&task.id).unwrap(), [run.clone()]);
    assert_eq!(store.live_runs(Some(Role::Worker)).unwrap(), [run.clone()]);
    assert_eq!(store.next_run_id(&task.id).unwrap(), "R2");
    run.ended = Some(jiff::Timestamp::now());
    run.outcome = Some(Outcome::Done);
    store.update_run(&run).unwrap();
    assert!(store.live_runs(None).unwrap().is_empty());
    assert_eq!(store.latest_run(&task.id, Role::Worker).unwrap(), Some(run));
    let mut check = Check {
        task: task.id.clone(),
        kind: CheckKind::Gate,
        key: "sha".to_owned(),
        result: CheckResult::Fail,
        notes: Some("red".to_owned()),
        log: None,
        by: Actor::Owner,
        at: jiff::Timestamp::now(),
    };
    store.insert_check(&check).unwrap();
    check.result = CheckResult::Pass;
    store.insert_check(&check).unwrap();
    assert_eq!(
        store.list_latest_checks(&task.id).unwrap(),
        std::slice::from_ref(&check)
    );
    assert_eq!(
        store
            .latest_check(&task.id, CheckKind::Gate, "sha")
            .unwrap(),
        Some(check)
    );
    assert_eq!(store.list_checks(&task.id).unwrap().len(), 2);
    assert!(
        store
            .latest_check(&task.id, CheckKind::Gate, "other")
            .unwrap()
            .is_none()
    );
}

#[test]
fn newer_schema_is_a_typed_refusal() {
    let root = crate::tests::tempdir();
    let path = root.path().join("corral.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
        .unwrap();
    let error = Store::open(&path).err().unwrap();
    assert!(as_refusal(&error).unwrap().reason.contains("newer"));
}
