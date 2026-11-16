use crate::{Ctx, app_read};
use anyhow::Result;
use corral_core::{Accept, Status, Store};
use corral_protocol::rows::{AttentionActor, AttentionDestination, InboxKind};

fn fixture() -> Result<(tempfile::TempDir, Ctx)> {
    let root = tempfile::tempdir()?;
    let ctx = crate::resources::FixedResourceProbe::ctx(Store::init(root.path())?);
    Ok((root, ctx))
}

#[test]
fn owner_attention_counts_exclude_pm_moves_and_unscheduled_backlog() -> Result<()> {
    let (_root, ctx) = fixture()?;
    let owner = ctx.store.create("owner move", None, Status::NeedsMe, "")?;
    ctx.store.update(&owner.id, |task| {
        task.runs.push(serde_json::from_value(serde_json::json!({
            "id":"R1", "agent":"fixture", "worktree":"/unused", "branch":"fixture",
            "base":"main", "base_sha":"base", "started":"2026-11-01T00:00:00Z",
            "ended":"2026-11-01T00:01:00Z", "outcome":"stopped"
        }))?);
        Ok(())
    })?;
    let pm = ctx.store.create("pm move", None, Status::Running, "")?;
    let backlog = ctx
        .store
        .create("not scheduled", None, Status::Backlog, "")?;
    let snapshot = app_read::attention_snapshot(&ctx)?;
    assert_eq!(snapshot.items, app_read::inbox_rows(&ctx)?);
    assert_eq!(snapshot.counts.total, 1);
    assert_eq!(snapshot.counts.inbox, 1);
    assert_eq!(snapshot.counts.now, 1);
    assert_eq!(snapshot.counts.plans + snapshot.counts.review, 0);
    assert_eq!(
        snapshot
            .items
            .iter()
            .find(|row| row.task.as_deref() == Some(&owner.id))
            .and_then(|row| row.attention.as_ref())
            .map(|attention| attention.whose_move),
        Some(AttentionActor::Owner)
    );
    assert_eq!(
        snapshot
            .items
            .iter()
            .find(|row| row.task.as_deref() == Some(&pm.id))
            .and_then(|row| row.attention.as_ref())
            .map(|attention| attention.whose_move),
        Some(AttentionActor::Pm)
    );
    assert!(
        !snapshot
            .items
            .iter()
            .any(|row| row.task.as_deref() == Some(&backlog.id))
    );
    Ok(())
}

#[test]
fn attention_blocker_and_next_step_are_the_cli_condition_not_status_labels() -> Result<()> {
    let (_root, ctx) = fixture()?;
    let task = ctx.store.create("owner move", None, Status::NeedsMe, "")?;
    let cli = crate::attention::board_hot_with_admission(&ctx, corral_core::clock::now(), &None)?;
    let condition = cli
        .tasks
        .iter()
        .find(|row| row.task == task.id)
        .unwrap()
        .assessment
        .conditions
        .first()
        .unwrap();
    let rows = app_read::inbox_rows(&ctx)?;
    let row = rows
        .iter()
        .find(|row| row.task.as_deref() == Some(&task.id))
        .unwrap();
    let attention = row.attention.as_ref().unwrap();
    assert_eq!(attention.reason, condition.reason);
    assert_eq!(attention.next_step, condition.next_action.description);
    assert_eq!(attention.command, condition.next_action.command);
    Ok(())
}

#[test]
fn review_attention_is_explicitly_not_a_question_record() -> Result<()> {
    let (_root, mut ctx) = fixture()?;
    ctx.store.config.review.accept = Some(Accept::Owner);
    let mut task = ctx.store.create("candidate", None, Status::Review, "")?;
    task.runs.push(serde_json::from_value(serde_json::json!({
        "id":"R1", "agent":"fixture", "worktree":"/unused", "branch":"fixture",
        "base":"main", "base_sha":"base", "started":"2026-11-01T00:00:00Z",
        "ended":"2026-11-01T00:01:00Z", "outcome":"done",
        "autopilot":{"stage":"reviewed", "at":"2026-11-01T00:01:00Z"}
    }))?);
    ctx.store.fixture_put_task(&task)?;
    let snapshot = app_read::attention_snapshot(&ctx)?;
    let row = snapshot
        .items
        .iter()
        .find(|row| row.task.as_deref() == Some(&task.id))
        .unwrap();
    assert_eq!(row.kind, InboxKind::Review);
    assert_eq!(
        row.attention.as_ref().unwrap().destination,
        AttentionDestination::Review
    );
    assert!(row.question_rev.is_none());
    assert!(!row.answerable);
    assert_eq!(snapshot.counts.review, 1);
    Ok(())
}
