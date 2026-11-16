use super::*;
use corral_core::Store;
use serde_json::{Value, json};

fn record(id: &str, updated: &str) -> Value {
    let usage = json!({"input": 2, "output": 3});
    json!({
        "version": 1, "id": id, "kind": "build", "title": id,
        "created": "2026-11-15T00:00:00Z", "updated": updated,
        "repo_root": "/private/repo", "worktree": "/private/worktree",
        "branch": "build/one", "base": "base", "outcome": {"state": "open"},
        "intent": [], "commits": [], "slices": [], "verification": [],
        "reviews": [], "messages": [], "decisions": [], "timeline": [],
        "usage": {"parent": usage, "children": usage, "reviewers": usage, "total": usage},
        "transcripts": {"parent": "/private/transcript", "children": {"S1": "/private/child"}}
    })
}

fn context() -> Result<(tempfile::TempDir, Ctx)> {
    let dir = tempfile::tempdir()?;
    let ctx = Ctx::from_store(Store::init(dir.path())?);
    Ok((dir, ctx))
}

#[test]
fn rows_keep_newest_order_and_unknown_cost_absent() -> Result<()> {
    let (_dir, ctx) = context()?;
    let older = record("B-older", "2026-11-15T00:01:00Z");
    let mut newer = record("B-newer", "2026-11-15T00:02:00Z");
    newer["usage"]["total"]["cost"] = json!(1.5);
    newer["outcome"] = json!({
        "state": "landed", "landed": "head", "override_reason": "Owner accepted open findings"
    });
    ctx.store.publish_build_record(&older.to_string())?;
    ctx.store.publish_build_record(&newer.to_string())?;
    let rows = builds_rows(&ctx)?;
    assert_eq!(
        rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        ["B-newer", "B-older"]
    );
    assert_eq!(rows[0].cost_usd, Some(1.5));
    assert_eq!(rows[1].tokens, 5);
    assert_eq!(rows[1].rev, 1);
    assert!(serde_json::to_value(&rows[1])?.get("cost_usd").is_none());
    assert_eq!(
        rows[0].override_reason.as_deref(),
        Some("Owner accepted open findings")
    );
    assert!(
        serde_json::to_value(&rows[1])?
            .get("override_reason")
            .is_none()
    );
    Ok(())
}

#[test]
fn detail_keeps_newest_messages_and_commits_with_source_totals() -> Result<()> {
    let (_dir, ctx) = context()?;
    let mut source = record("B-history", "2026-11-15T00:02:00Z");
    source["messages"] = json!(
        (0..101)
            .map(|index| json!({
                "at": format!("2026-11-15T00:{:02}:{:02}Z", index / 60, index % 60),
                "from": "parent", "to": "S1",
                "text": index.to_string(), "delivery": "delivered"
            }))
            .collect::<Vec<_>>()
    );
    source["messages"].as_array_mut().unwrap().reverse();
    source["commits"] = json!(
        (0..51)
            .map(|index| json!({
                "sha": index.to_string(), "subject": "Commit"
            }))
            .collect::<Vec<_>>()
    );
    source["verification"] = json!([{
        "attempt": "G1", "tier": "quick", "head": "head", "result": "pass",
        "started": "2026-11-15T00:01:00Z", "ended": "2026-11-15T00:02:00Z",
        "log": "/private/gate-log"
    }]);
    ctx.store.publish_build_record(&source.to_string())?;
    let detail = build_detail(&ctx, "B-history")?;
    assert_eq!(detail.messages_total, 101);
    assert_eq!(detail.messages.len(), 100);
    assert_eq!(detail.messages.first().unwrap().text, "1");
    assert_eq!(detail.messages.last().unwrap().text, "100");
    assert_eq!(detail.commits_total, 51);
    assert_eq!(detail.commits.len(), 50);
    assert_eq!(detail.commits.first().unwrap().sha, "1");
    assert_eq!(detail.commits.last().unwrap().sha, "50");
    assert_eq!(detail.verification.len(), 1);
    assert!(!serde_json::to_string(&detail)?.contains("/private/"));
    Ok(())
}

#[test]
fn detail_reports_omitted_messages_under_the_wire_byte_budget() -> Result<()> {
    let (_dir, ctx) = context()?;
    let mut source = record("B-large", "2026-11-15T00:02:00Z");
    source["messages"] = json!(
        (0..100)
            .map(|index| json!({
                "at": "2026-11-15T00:01:00Z", "from": "parent", "to": "S1",
                "text": format!("{index:03}{}", "\\".repeat(2045)), "delivery": "delivered"
            }))
            .collect::<Vec<_>>()
    );
    ctx.store.publish_build_record(&source.to_string())?;
    let detail = build_detail(&ctx, "B-large")?;
    assert_eq!(detail.messages_total, 100);
    assert!(!detail.messages.is_empty());
    assert!(detail.messages.len() < 100);
    assert!(detail.messages.last().unwrap().text.starts_with("099"));
    assert!(super::super::paging::json_len(&DetailBody::Build(detail))? <= APP_CONTROL_MAX - 4096);
    Ok(())
}

#[test]
fn detail_revision_and_etag_change_on_record_republication() -> Result<()> {
    let (_dir, ctx) = context()?;
    let mut source = record("B-revised", "2026-11-15T00:02:00Z");
    ctx.store.publish_build_record(&source.to_string())?;
    let mut request = DetailGet {
        target: DetailRef::Build {
            id: "B-revised".into(),
        },
        etag: None,
        page: None,
    };
    let initial = super::super::read(&ctx, &request)?;
    request.etag = Some(initial.etag.clone());
    assert!(super::super::read(&ctx, &request)?.body.is_none());
    source["title"] = json!("Revised");
    ctx.store.publish_build_record(&source.to_string())?;
    let revised = super::super::read(&ctx, &request)?;
    assert_ne!(revised.etag, initial.etag);
    let Some(DetailBody::Build(detail)) = revised.body else {
        panic!("missing build detail")
    };
    assert_eq!(detail.rev, 2);
    assert_eq!(builds_rows(&ctx)?[0].rev, detail.rev);
    Ok(())
}

#[test]
fn unknown_build_error_names_the_requested_id() -> Result<()> {
    let (_dir, ctx) = context()?;
    let error = build_detail(&ctx, "B-missing").unwrap_err();
    assert!(error.to_string().contains("B-missing"));
    Ok(())
}
