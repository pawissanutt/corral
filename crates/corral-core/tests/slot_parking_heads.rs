//! Slot accounting uses owner-aware facts, not a walk of a run's wait history.
use corral_core::work::{self, Counter, HotScope};
use corral_core::{Run, Status, Store, TaskMut, WaitRecord, WaitState};
use jiff::Timestamp;
use serde_json::json;

fn at() -> Timestamp {
    "2026-10-28T03:00:00Z".parse().unwrap()
}
fn run() -> Run {
    serde_json::from_value(json!({
        "id":"R100", "agent":"fixture", "worktree":"/fixture/worktree",
        "branch":"fixture", "base":"main", "base_sha":"abc", "started":at()
    }))
    .unwrap()
}
fn wait(id: &str, target: &str) -> WaitRecord {
    serde_json::from_value(json!({
        "request": {"version":1, "id":id, "run":"R100", "condition":"lands",
            "owner":"worker", "source":"worker", "deadline":null,
            "handle":{"kind":"channel", "channel_id":format!("task:{target}:landed"), "capability":"wait_v1"}},
        "registered_at":at(), "observation":{"at":at(),"state":"pending","detail":""}, "receipt":null
    })).unwrap()
}
fn close(wait: &mut WaitRecord) {
    wait.receipt = Some(
        serde_json::from_value(json!({
            "version":1,"id":"ready","run":"R100","outcome":"ready","delivery":"acknowledged",
            "observed_at":at(),"answer":null,"data":null
        }))
        .unwrap(),
    );
    wait.observation =
        corral_core::obligations::receipt_observation(wait.receipt.as_ref().unwrap(), at());
}
fn same(store: &Store, id: &str, expected: bool) {
    let head = store.head(id).unwrap();
    let task = store.get(id).unwrap();
    assert_eq!(head.holds_dispatch_slot(), expected);
    assert_eq!(head.holds_dispatch_slot(), task.holds_dispatch_slot());
    let run = task.current_run().unwrap();
    assert_eq!(head.current_run.as_ref().unwrap().facts.owner_task, id);
    assert_eq!(
        head.current_run.as_ref().unwrap().facts.dependency_waits,
        corral_core::ContainerFacts::of_run(id, "000001/000000", run)
            .unwrap()
            .dependency_waits
    );
}

#[test]
fn head_slot_parking_tracks_keyed_wait_changes_and_self_waits() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let mut task = store.create("Slots", None, Status::Running, "").unwrap();
    task.runs = vec![run()];
    store.fixture_put_task(&task).unwrap();
    same(&store, &task.id, true);
    store
        .update_head(&task.id, |head| {
            head.push_wait("R100", wait("dep", "T-999"))
        })
        .unwrap();
    same(&store, &task.id, false);
    store
        .update_head(&task.id, |head| {
            head.push_wait("R100", wait("self", &task.id))
        })
        .unwrap();
    same(&store, &task.id, true);
    store
        .update_head(&task.id, |head| {
            head.edit_wait("R100", "self", |w| {
                close(w);
                Ok(())
            })
        })
        .unwrap();
    same(&store, &task.id, false);
    store
        .update_head(&task.id, |head| {
            head.edit_wait("R100", "dep", |w| {
                w.observation.state = WaitState::Unsupported;
                Ok(())
            })
        })
        .unwrap();
    same(&store, &task.id, false);
    store
        .update_head(&task.id, |head| {
            head.edit_wait("R100", "dep", |w| {
                close(w);
                Ok(())
            })
        })
        .unwrap();
    same(&store, &task.id, true);
    store
        .update_head(&task.id, |head| {
            head.edit_run("R100", |run| {
                run.obligations.slot_wait = Some(corral_core::SlotWait {
                    since: at(),
                    on: vec!["T-999".into()],
                });
                Ok(())
            })
        })
        .unwrap();
    same(&store, &task.id, false);
}

#[test]
fn slot_head_reads_do_not_decode_nested_or_terminal_history() {
    let mut observations = Vec::new();
    for scale in [1, 10] {
        let root = tempfile::tempdir().unwrap();
        let store = Store::init(root.path()).unwrap();
        let mut task = store.create("Slots", None, Status::Running, "").unwrap();
        task.created = at();
        task.updated = at();
        task.status_since = Some(at());
        for n in 0..scale {
            let mut old = run();
            old.id = format!("R{n}");
            old.ended = Some(at());
            old.outcome = Some(corral_core::RunOutcome::Done);
            task.runs.push(old);
        }
        let mut current = run();
        for n in 0..scale {
            let mut old = wait(&format!("completed-{n:04}"), "T-999");
            close(&mut old);
            current.obligations.waits.push(old);
        }
        current.obligations.waits.push(wait("dep", "T-999"));
        task.runs.push(current);
        store.fixture_put_task(&task).unwrap();
        let _hot = HotScope::enter();
        let before = work::snapshot();
        let head = store.head(&task.id).unwrap();
        assert!(!head.holds_dispatch_slot());
        let after = work::snapshot();
        observations.push(
            [
                Counter::NestedRowDecodes,
                Counter::FullTaskHydrations,
                Counter::DecodedDocBytes,
            ]
            .map(|counter| after.total(counter) - before.total(counter)),
        );
    }
    assert_eq!(observations[0], observations[1]);
    assert_eq!(observations[0][0], 0);
    assert_eq!(observations[0][1], 0);
}
