use super::*;
use corral_core::{Run, Status, Store};

#[test]
fn bounded_indexed_page_keeps_source_order_missing_nodes_and_blocked_state() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = crate::resources::FixedResourceProbe::ctx(Store::init(dir.path()).unwrap());
    let dep = ctx
        .store
        .create("blocked prerequisite", None, Status::Backlog, "")
        .unwrap();
    let task = ctx.store.create("root", None, Status::Running, "").unwrap();
    let run: Run = serde_json::from_value(serde_json::json!({
        "id":"R1", "agent":"generic", "worktree":dir.path(), "branch":"candidate", "base":"main", "base_sha":"base",
        "started":"2026-01-01T00:00:00Z"
    })).unwrap();
    ctx.store
        .update(&task.id, |t| {
            t.runs.push(run);
            t.depends_on = vec![dep.id.clone(), "T-999".into()];
            t.parent = Some(dep.id.clone());
            Ok(())
        })
        .unwrap();
    for i in 0..2000 {
        ctx.store
            .log_event(
                Some(&task.id),
                "panel_history",
                serde_json::json!({"run":"R1", "index":i}),
            )
            .unwrap();
    }
    let before = ctx.store.event_tail().unwrap();
    let hot = corral_core::work::HotScope::enter();
    let measured_before = corral_core::work::snapshot();
    let result = inspect(&ctx, &task.id, "R1").unwrap();
    let work = corral_core::work::snapshot().since(&measured_before);
    drop(hot);
    assert_eq!(
        work.total(corral_core::work::Counter::FullTaskHydrations),
        0
    );
    assert!(work.total(corral_core::work::Counter::EventDecodes) <= u64::from(MAX_EVENTS));
    println!(
        "2000 events: full task hydrations=0; event decodes={}",
        work.total(corral_core::work::Counter::EventDecodes)
    );
    assert!(result.event_gap);
    assert_eq!(result.events.len(), MAX_EVENTS as usize);
    assert_eq!(result.events.last().unwrap().event.detail["index"], 1999);
    assert!(
        result
            .events
            .windows(2)
            .all(|w| w[0].sequence < w[1].sequence)
    );
    assert!(
        result
            .nodes
            .iter()
            .any(|n| n.id == "T-999" && n.missing && n.status.is_none())
    );
    assert!(
        result
            .nodes
            .iter()
            .any(|n| n.id == dep.id && n.status == Some(Status::Backlog))
    );
    assert_eq!(result.nodes[0].parent.as_deref(), Some(dep.id.as_str()));
    assert_eq!(ctx.store.event_tail().unwrap(), before);
    assert!(inspect(&ctx, &task.id, "R2").is_err());
    ctx.store
        .log_event(
            Some(&task.id),
            "panel_history",
            serde_json::json!({"run":"R2"}),
        )
        .unwrap();
    ctx.store
        .log_event(
            Some(&task.id),
            "panel_history",
            serde_json::json!({"run":"R1", "large":"x".repeat(9000)}),
        )
        .unwrap();
    let gap = inspect(&ctx, &task.id, "R1").unwrap();
    assert!(gap.event_gap);
    assert!(
        gap.events
            .iter()
            .all(|e| e.event.run.as_deref() != Some("R2"))
    );
    assert!(
        gap.events
            .iter()
            .all(|e| e.event.detail.get("large").is_none())
    );
    ctx.store
        .update(&task.id, |t| {
            t.depends_on = (100..160).map(|n| format!("T-{n}")).collect();
            Ok(())
        })
        .unwrap();
    let capped = inspect(&ctx, &task.id, "R1").unwrap();
    assert!(capped.graph_gap);
    assert!(capped.nodes.len() <= MAX_NODES);
    assert!(capped.nodes[0].depends_on.len() <= MAX_NODES);
}
