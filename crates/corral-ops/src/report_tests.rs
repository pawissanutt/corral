use corral_core::pins::PinTarget;
use corral_core::{Event, Task};
use serde_json::json;

use super::*;

fn at(secs: i64) -> Timestamp {
    Timestamp::from_second(1_800_000_000 + secs).unwrap()
}

fn run(id: &str, started: i64, model: &str, usage: Option<serde_json::Value>) -> serde_json::Value {
    let mut run = json!({
        "id": id, "agent": "fake", "harness": "claude", "model": model,
        "worktree": "/nonexistent", "branch": "corral/x", "base": "main", "base_sha": "0",
        "started": at(started), "ended": at(started + 600), "outcome": "done",
    });
    if let Some(usage) = usage {
        run["usage"] = usage;
    } else {
        run["usage_note"] = json!("unsupported harness opus");
    }
    run
}

fn entry(model: &str, offset: i64, tokens: u64) -> serde_json::Value {
    json!({"at": at(offset), "model": model, "input": tokens, "output": tokens / 2,
           "cache_read": tokens * 3, "cache_write": 0, "reported_usd": null,
           "estimated_usd": null, "unpriced_tokens": 0})
}

fn usage(entries: Vec<serde_json::Value>) -> serde_json::Value {
    json!({"input": 0, "output": 0, "cache_read": 0, "cache_write": 0, "entries": entries})
}

fn tasks() -> Vec<Task> {
    let mut priced = run(
        "R1",
        100,
        "claude-sonnet-4-6",
        Some(usage(vec![entry("claude-sonnet-4-6", 200, 1000)])),
    );
    priced["review_runs"] = json!([run(
        "R1-review",
        300,
        "claude-sonnet-4-6",
        Some(usage(vec![entry("claude-sonnet-4-6", 400, 500)]))
    )]);
    let task = |id: &str, project: &str, runs: serde_json::Value| -> Task {
        serde_json::from_value(json!({
            "id": id, "title": format!("Task {id}"), "status": "done", "project": project,
            "created": at(0), "updated": at(0), "runs": runs,
        }))
        .unwrap()
    };
    vec![
        task("T-001", "app", json!([priced])),
        task(
            "T-002",
            "lib",
            json!([
                run(
                    "R1",
                    150,
                    "mystery-model",
                    Some(usage(vec![entry("mystery-model", 250, 800)]))
                ),
                run("R2", 160, "claude-sonnet-4-6", None),
            ]),
        ),
    ]
}

fn events() -> Vec<Event> {
    let event = |ts: i64, task: &str, kind: &str, detail: serde_json::Value| Event {
        seq: 0,
        ts: at(ts),
        task: Some(task.into()),
        kind: kind.into(),
        actor: None,
        via: None,
        detail,
    };
    vec![
        event(110, "T-001", "gate_started", json!({"run": "R1"})),
        event(170, "T-001", "gate", json!({"run": "R1", "result": "fail"})),
        event(
            180,
            "T-001",
            "autopilot",
            json!({"run": "R1", "to": "changes", "reason": "the gate failed at abc: `cargo test`"}),
        ),
        event(500, "T-001", "approved", json!({"run": "R1"})),
    ]
}

fn page(tasks: &[Task]) -> String {
    let config = corral_core::Config::default_for(std::path::Path::new("/nonexistent"));
    let model = collect_report(&config, tasks, &events(), at(0), at(1000), None).unwrap();
    html::render(&model).unwrap()
}

fn data(page: &str) -> &str {
    let start = page.find("id=\"report-data\">").unwrap();
    let rest = &page[start..];
    &rest[..rest.find("</script>").unwrap()]
}

#[test]
fn same_evidence_and_snapshot_render_identical_bytes_in_any_task_order() {
    let forward = tasks();
    let mut reversed = tasks();
    reversed.reverse();
    let (a, b) = (page(&forward), page(&reversed));
    assert_eq!(data(&a), data(&b));
    assert_eq!(a, b);
}

#[test]
fn page_references_no_network_resource() {
    let page = page(&tasks());
    for needle in [
        "http://",
        "https://",
        "src=",
        "<link",
        "@import",
        "url(",
        "fetch(",
        "XMLHttpRequest",
    ] {
        assert!(!page.contains(needle), "page contains {needle}");
    }
    assert!(page.contains("<svg"));
}

#[test]
fn unpriced_tokens_are_their_own_category_never_zero_dollars() {
    let config = corral_core::Config::default_for(std::path::Path::new("/nonexistent"));
    let model = collect_report(&config, &tasks(), &events(), at(0), at(1000), None).unwrap();
    let unpriced = model
        .spend
        .categories
        .iter()
        .find(|c| c.category == "unpriced")
        .unwrap();
    assert_eq!(unpriced.tokens, 800 + 400 + 2400);
    assert_eq!(unpriced.estimated_usd, None);
    let lib = model.tasks.iter().find(|t| t.id == "T-002").unwrap();
    assert_eq!(lib.total.estimated_usd, None);
    assert_eq!(model.quality.send_backs.get("gate"), Some(&1));
    assert_eq!(
        model.data_quality.unsupported_harnesses.get("claude"),
        Some(&1)
    );
    assert!(model.summary.spend.estimated_usd.is_some_and(|v| v > 0.0));
}

#[test]
fn reports_keep_one_follow_latest_pin_that_opens_the_newest_page_live() {
    let tmp = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(tmp.path()).unwrap();
    store
        .create("Anchor", None, Status::Done, "report anchor")
        .unwrap();
    let ctx = Ctx::from_store(store);
    let now = corral_core::clock::now();
    let first = report_at(&ctx, "30d", None, now).unwrap();
    let second = report_at(&ctx, "30d", None, now).unwrap();
    assert_eq!(first.html, second.html);
    let pins = ctx.store.pins().unwrap();
    assert_eq!(pins.len(), 1);
    // The board's Enter resolves pins through the head API (`shelf::open`).
    let resolved = ctx.store.resolve_pin_head(&pins[0]).unwrap();
    let PinTarget::Artifact { artifact, .. } = resolved else {
        panic!("pin did not resolve to an artifact: {resolved:?}");
    };
    assert_eq!(Some(&artifact), second.artifact.as_ref());
    assert!(crate::viewers::served_live(
        &ctx.store.config.viewers,
        &artifact
    ));
}
