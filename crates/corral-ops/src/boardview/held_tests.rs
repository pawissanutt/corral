use crate::Ctx;
use anyhow::Result;
use corral_core::{Status, Store, TaskHold};
use corral_protocol::board::BoardGroup;

#[test]
fn held_requires_an_actual_hold_and_always_carries_its_cause() -> Result<()> {
    let root = tempfile::tempdir()?;
    let ctx = Ctx::from_store(Store::init(root.path())?);
    let mut task = ctx
        .store
        .create("not scheduled", None, Status::Backlog, "")?;
    let row = super::compute_row(&ctx, &task.id)?.unwrap();
    assert_eq!(row.group, BoardGroup::Queued);
    assert!(row.held_reason.is_none());

    task.status = Status::Review;
    task.runs.push(serde_json::from_value(serde_json::json!({
        "id":"R1", "agent":"fixture", "worktree":"/unused", "branch":"fixture",
        "base":"main", "base_sha":"base", "started":"2026-11-01T00:00:00Z",
        "ended":"2026-11-01T00:01:00Z", "outcome":"done",
        "autopilot":{"stage":"blocked", "at":"2026-11-01T00:01:00Z", "reason":"candidate stale"}
    }))?);
    ctx.store.fixture_put_task(&task)?;
    let row = super::compute_row(&ctx, &task.id)?.unwrap();
    assert_ne!(row.group, BoardGroup::Held);
    assert!(row.held_reason.is_none());

    task.status = Status::Ready;
    task.runs.clear();
    task.dispatch_hold = true;
    ctx.store.fixture_put_task(&task)?;
    let row = super::compute_row(&ctx, &task.id)?.unwrap();
    assert_eq!(row.group, BoardGroup::Held);
    assert!(
        row.held_reason
            .as_ref()
            .is_some_and(|reason| !reason.is_empty())
    );
    let cli = crate::attention::board_hot_with_admission(&ctx, corral_core::clock::now(), &None)?;
    assert!(
        cli.tasks
            .iter()
            .find(|row| row.task == task.id)
            .unwrap()
            .assessment
            .conditions
            .iter()
            .any(|condition| condition.class == corral_core::attention::WaitClass::Hold)
    );

    task.dispatch_hold = false;
    task.hold = Some(TaskHold {
        reason: "paused for focused work".into(),
        at: task.created,
        resume: None,
        focus: None,
    });
    ctx.store.fixture_put_task(&task)?;
    let row = super::compute_row(&ctx, &task.id)?.unwrap();
    assert_eq!(row.group, BoardGroup::Held);
    assert_eq!(
        row.held_reason.as_deref(),
        task.hold.as_ref().map(|hold| hold.reason.as_str())
    );
    Ok(())
}
