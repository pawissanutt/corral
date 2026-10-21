use corral_core::{Run, Status, Store, Usage, UsageEntry};
use corral_ops::throughput::{ModeThroughput, mode_label, throughput_section_html};
use corral_ops::{StatsBy, collect_analytics};
use jiff::Timestamp;
use serde_json::json;

fn run(id: &str, effort: &str) -> Run {
    serde_json::from_value(json!({
        "id":id, "agent":"implementer", "harness":"custom", "model":"model-a",
        "effort":effort, "worktree":"/unused", "branch":"fixture",
        "base":"main", "base_sha":"abc",
        "started":"2026-01-01T00:00:00Z", "ended":"2026-01-01T01:00:00Z"
    }))
    .unwrap()
}

fn entry(model: &str, output: u64, latency_ms: Option<u64>) -> UsageEntry {
    UsageEntry {
        model: Some(model.into()),
        output,
        latency_ms,
        ..Default::default()
    }
}

fn collect(store: &Store) -> corral_ops::Analytics {
    collect_analytics(
        &store.list().unwrap(),
        &store.events().unwrap(),
        None,
        "2026-02-01T00:00:00Z".parse::<Timestamp>().unwrap(),
    )
    .unwrap()
}

fn find<'a>(rows: &'a [ModeThroughput], label: &str) -> &'a ModeThroughput {
    rows.iter()
        .find(|row| mode_label(&row.group) == label)
        .unwrap_or_else(|| panic!("no mode {label}"))
}

#[test]
fn switched_models_are_rated_under_their_own_mode() {
    // Given: a model-a dispatch whose usage switches to model-b mid-run, a legacy
    // aggregate run, another effort, and a timed reviewer on a third model.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store
        .create("Rates", Some("app"), Status::Done, "")
        .unwrap();
    let mut first = run("R1", "high");
    first.usage = Some(Usage::from_entries(vec![
        entry("model-a", 100, Some(2000)),
        entry("model-a", 300, Some(2000)),
        entry("model-b", 1000, Some(1000)),
        entry("model-a", 500, None),
    ]));
    let mut reviewer = run("review", "high");
    reviewer.model = Some("model-c".into());
    reviewer.usage = Some(Usage::from_entries(vec![entry(
        "model-c",
        9000,
        Some(1000),
    )]));
    first.review_runs.push(reviewer);
    let mut second = run("R2", "high");
    second.usage = Some(Usage {
        output: 5,
        ..Default::default()
    });
    let mut third = run("R3", "low");
    third.usage = Some(Usage::from_entries(vec![entry("model-a", 60, Some(1000))]));
    task.runs = vec![first, second, third];
    store.save(&task).unwrap();
    // When: the pure collector rates every mode.
    let report = collect(&store);
    let rows = &report.throughput_by_mode;
    // Then: the switched response leaves model-a's rate and the reviewer is not rated.
    assert_eq!(rows.len(), 3);
    let a = &find(rows, "model-a / high / custom").throughput;
    assert_eq!((a.samples, a.excluded), (2, 2));
    assert_eq!(a.p50_tokens_per_second, Some(100.0));
    assert_eq!(a.p90_tokens_per_second, Some(140.0));
    let b = &find(rows, "model-b / high / custom").throughput;
    assert_eq!((b.samples, b.excluded), (1, 0));
    assert_eq!(b.p50_tokens_per_second, Some(1000.0));
    let low = &find(rows, "model-a / low / custom").throughput;
    assert_eq!((low.samples, low.excluded), (1, 0));
    assert_eq!(low.p90_tokens_per_second, Some(60.0));
    // And: run-outcome mode cohorts keep the dispatch's key, one row per distinct dispatch mode.
    assert_eq!(report.groups[&StatsBy::Mode].len(), 2);
}

#[test]
fn html_section_lists_one_row_per_mode_with_exclusion_counts() {
    // Given: a store with two modes and two excluded entries.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store
        .create("Rates", Some("app"), Status::Done, "")
        .unwrap();
    let mut first = run("R1", "high");
    first.usage = Some(Usage::from_entries(vec![
        entry("model-a", 100, Some(2000)),
        entry("model-a", 300, Some(2000)),
        entry("model-a", 500, None),
        entry("model-b", 40, None),
    ]));
    task.runs = vec![first];
    store.save(&task).unwrap();
    // When: the section is rendered.
    let report = collect(&store);
    let html = throughput_section_html(&report.throughput_by_mode);
    // Then: it has an anchor, one table row per mode with numeric cells, and no network references.
    assert!(html.contains("<section id=\"throughput\">"));
    assert_eq!(html.matches("<tr><td>").count(), 2);
    assert!(html.contains(
        "<tr><td>model-a / high / custom</td><td>2</td><td>1</td><td>100.0</td><td>140.0</td><td>-</td></tr>"
    ));
    assert!(html.contains(
        "<tr><td>model-b / high / custom</td><td>0</td><td>1</td><td>-</td><td>-</td><td>-</td></tr>"
    ));
    assert!(html.contains("data-note=\"rate-definition\""));
    assert!(html.contains("data-note=\"excluded\" data-count=\"2\""));
    assert!(!html.contains("http"));
    assert_eq!(html, throughput_section_html(&report.throughput_by_mode));
}

#[test]
fn report_page_is_a_standalone_document_around_the_section() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store
        .create("Rates", Some("app"), Status::Done, "")
        .unwrap();
    let mut first = run("R1", "high");
    first.usage = Some(Usage::from_entries(vec![entry("model-a", 100, Some(2000))]));
    task.runs = vec![first];
    store.save(&task).unwrap();
    let rows = collect(&store).throughput_by_mode;
    let page = corral_ops::throughput::throughput_report_html(&rows);
    assert!(page.starts_with("<!doctype html>"));
    assert!(page.contains("<meta charset=\"utf-8\">"));
    assert_eq!(page.matches("<section id=\"throughput\">").count(), 1);
    assert!(page.contains("<tr><td>model-a / high / custom</td><td>1</td><td>0</td><td>50.0</td>"));
    assert!(!page.contains("http"));
    assert!(!page.contains("<script"));
}
