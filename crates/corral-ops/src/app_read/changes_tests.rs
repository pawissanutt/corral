use crate::{Ctx, app_read};
use anyhow::Result;
use corral_core::{Status, Store};
use corral_protocol::app::{ChangeKind, ChangesQuery, SeenCursor};

#[test]
fn owner_changes_hide_noise_preserve_moves_and_count_unseen_independently_of_pages() -> Result<()> {
    let root = tempfile::tempdir()?;
    let ctx = Ctx::from_store(Store::init(root.path())?);
    let task = ctx.store.create("changed", None, Status::Backlog, "")?;
    let query = ChangesQuery {
        before: None,
        after: None,
        limit: 1,
    };
    let first = app_read::changes(&ctx, &query, &SeenCursor::default())?;
    let seen = SeenCursor {
        through: first.head,
        at: None,
    };
    for (kind, detail) in [
        ("storage_reclaim_failed", serde_json::json!({})),
        ("moved", serde_json::json!({"from":"backlog", "to":"ready"})),
        ("attention_resolved", serde_json::json!({})),
        ("gate", serde_json::json!({"result":"fail"})),
        ("question_opened", serde_json::json!({})),
        ("agent_session_reclassified", serde_json::json!({})),
    ] {
        ctx.store.log_event(Some(&task.id), kind, detail)?;
    }
    let mut before = None;
    let mut lines = Vec::new();
    let mut total = None;
    loop {
        let page = app_read::changes(
            &ctx,
            &ChangesQuery {
                before,
                ..query.clone()
            },
            &seen,
        )?;
        assert_eq!(page.unseen, Some(3));
        assert_eq!(page.seen, seen);
        if let Some(total) = total {
            assert_eq!(page.total, Some(total));
        }
        total = page.total;
        lines.extend(page.lines);
        let Some(older) = page.older else { break };
        assert!(before.is_none_or(|previous| older < previous));
        before = Some(older);
    }
    assert_eq!(total, Some(lines.len() as u64));
    assert_eq!(
        lines.iter().filter(|line| line.seq > seen.through).count(),
        3
    );
    let moved = lines
        .iter()
        .find(|line| line.kind == ChangeKind::Moved)
        .unwrap();
    assert_eq!(moved.from.as_deref(), Some("backlog"));
    assert_eq!(moved.to.as_deref(), Some("ready"));
    assert!(
        lines
            .iter()
            .find(|line| line.kind == ChangeKind::Gate)
            .unwrap()
            .failed
    );
    assert!(!lines.iter().any(|line| line.kind == ChangeKind::Other));
    Ok(())
}
