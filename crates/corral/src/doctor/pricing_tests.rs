use corral_core::{Run, Status, Store};
use serde_json::json;

use super::check;

#[test]
fn coverage_warning_obeys_inclusive_cutoff_and_reviewer_start_times() {
    // Given: old parent usage and recent reviewer evidence; a fixed clock fixes the cutoff.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store.create("Coverage", None, Status::Done, "").unwrap();
    let parent: Run = serde_json::from_value(json!({
        "id":"R1","agent":"test","worktree":"/unused","branch":"test","base":"main","base_sha":"abc",
        "started":"2026-09-17T23:59:59Z",
        "usage":{"input":100,"output":0,"cache_read":0,"cache_write":0},
        "review_runs":[{
            "id":"review","agent":"test","worktree":"/unused","branch":"test","base":"main","base_sha":"abc",
            "started":"2026-09-18T00:00:00Z","model":"unlisted",
            "usage":{"input":3,"output":0,"cache_read":0,"cache_write":0}
        }]
    })).unwrap();
    task.runs.push(parent);
    store.save(&task).unwrap();
    let original = std::fs::read(&task.path).unwrap();
    let now = "2026-10-18T00:00:00Z".parse().unwrap();
    // When: doctor checks stored evidence without recollecting or writing it.
    let warning = serde_json::to_value(check(&store, now).unwrap()).unwrap();
    // Then: only a soft machine-readable warning is emitted, and task bytes survive.
    assert_eq!(warning["id"], "pricing_coverage");
    assert_eq!(warning["ok"], false);
    assert_eq!(warning["hard"], false);
    assert!(warning["fix"].is_null());
    assert_eq!(std::fs::read(&task.path).unwrap(), original);
}

#[test]
fn catalog_priced_tokens_old_usage_and_zero_tokens_do_not_warn() {
    for (started, model, tokens) in [
        ("2026-10-01T00:00:00Z", "gpt-6-astra", 100),
        ("2026-09-17T23:59:59Z", "unlisted", 100),
        ("2026-10-01T00:00:00Z", "unlisted", 0),
    ] {
        // Given: catalog-priced, old, or zero legacy counters, plus a run with no usage.
        let tmp = tempfile::tempdir().unwrap();
        let mut store = Store::init(tmp.path()).unwrap();
        store.config.pricing.clear();
        let mut task = store.create("Priced", None, Status::Done, "").unwrap();
        let run: Run = serde_json::from_value(json!({
        "id":"R1","agent":"test","worktree":"/unused","branch":"test","base":"main","base_sha":"abc",
        "started":started,"model":model,
        "usage":{"input":tokens,"output":0,"cache_read":0,"cache_write":0}
    })).unwrap();
        task.runs = vec![run.clone(), Run { usage: None, ..run }];
        store.save(&task).unwrap();
        // When / Then: read-only repricing uses catalog coverage, not the old estimate's absence.
        assert!(
            check(&store, "2026-10-18T00:00:00Z".parse().unwrap())
                .unwrap()
                .ok
        );
    }
}
