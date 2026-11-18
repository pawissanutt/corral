use super::status::{Driver, project};
use crate::Ctx;
use corral_core::{Status, Store};
use serde_json::json;

fn fixture() -> (tempfile::TempDir, Ctx, corral_core::Task) {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store.create("Co-work", None, Status::Review, "").unwrap();
    task.runs.push(
        serde_json::from_value(json!({
            "id":"R1", "agent":"fake", "worktree":tmp.path(), "branch":"b", "base":"main",
            "base_sha":"fixture", "started":task.created, "outcome":"done"
        }))
        .unwrap(),
    );
    (tmp, Ctx::from_store(store), task)
}

#[test]
fn responsibility_comes_from_canonical_records_never_origin_or_label() {
    let (_tmp, ctx, mut task) = fixture();
    // Ordinary board work: no assistant responsibility, whatever its origin says.
    assert_eq!(project(&ctx, &task), None);

    task.cowork = Some(
        serde_json::from_value(json!({
            "session":"A-1", "scratch":"fix", "run":"R1", "review":"required", "owner_confirmed":true
        }))
        .unwrap(),
    );
    let led = project(&ctx, &task).unwrap();
    assert_eq!(led.driver, Driver::Assistant);
    assert_eq!(led.label, "Co-work · A-1 · Ship when checks pass");

    // A transfer fence without its journal is reconciling, never guessed as board-owned.
    task.cowork = Some(
        serde_json::from_value(json!({
            "session":"A-1", "scratch":"fix", "run":"R1", "review":"transfer_v1",
            "completion":{"version":1,"contract_revision":"c","intent":"ship",
                "driver":{"kind":"board"},"instruction":"x"}
        }))
        .unwrap(),
    );
    let fenced = project(&ctx, &task).unwrap();
    assert_eq!(fenced.driver, Driver::Reconciling);
    assert!(fenced.waiting.unwrap().contains("handoff_reconciling"));
}
