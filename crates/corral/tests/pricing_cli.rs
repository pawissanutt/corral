use std::path::Path;
use std::process::{Command, Output, Stdio};

use corral_core::{Run, Status, Store};
use serde_json::{Value, json};

fn output(root: &Path, args: &[&str]) -> Output {
    let output = Command::new(env!("CARGO_BIN_EXE_corral"))
        .env("CORRAL_ROOT", root)
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("HERDR_BIN_PATH", "/nonexistent/herdr")
        .env_remove("CORRAL_GATE_ATTEMPT")
        .env_remove("CORRAL_CANDIDATE_HEAD")
        .env_remove("CORRAL_INTEGRATION_BASE")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_WORKSPACE_ID")
        .env_remove("HERDR_TAB_ID")
        .arg("--root")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    println!(
        "corral {}\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout)
    );
    output
}

fn invoke(root: &Path, args: &[&str]) -> Value {
    let output = output(root, args);
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn run(model: Option<&str>, usage: Value) -> Run {
    serde_json::from_value(json!({
        "id":"R1", "agent":"test", "model":model,
        "worktree":"/unavailable", "branch":"test", "base":"main", "base_sha":"abc",
        "started":"2026-01-01T00:00:00Z", "ended":"2026-01-01T00:01:00Z",
        "usage":usage
    }))
    .unwrap()
}

fn usage(model: Option<&str>, input: u64, read: u64, write: u64) -> Value {
    json!({
        "input":input, "output":0, "cache_read":read, "cache_write":write,
        "entries":[{
            "model":model, "input":input, "output":0, "cache_read":read, "cache_write":write,
            "reported_usd":null, "estimated_usd":null, "unpriced_tokens":input+read+write
        }]
    })
}

#[test]
fn pricing_check_json_covers_unknown_models_cache_fallbacks_and_shadows() {
    // Given: legacy pricing, response-level model switches and nested reviewer usage.
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.pricing.clear();
    for (model, price) in [
        (
            "gpt-6-astra",
            json!({"input":7.0,"output":11.0,"cache_write":0.0}),
        ),
        (
            "alias-owner",
            json!({"input":8.0,"output":12.0,"aliases":["claude-sonnet-5-5"]}),
        ),
        ("cacheless", json!({"input":3.0,"output":9.0})),
    ] {
        store
            .config
            .pricing
            .insert(model.into(), serde_json::from_value(price).unwrap());
    }
    // A matching catalog override must not be reported as a differing shadow.
    store.config.pricing.insert(
        "gpt-6-sol".into(),
        corral_core::pricing::catalog_prices()["gpt-6-sol"].clone(),
    );
    store.save_config().unwrap();
    let mut task = store.create("Coverage", None, Status::Done, "").unwrap();
    let mut parent = run(
        Some("gpt-6-astra"),
        usage(Some("gpt-6-astra"), 1_000_000, 20, 30),
    );
    parent.review_runs = vec![
        run(None, usage(Some("unlisted"), 99, 3, 4)),
        run(None, usage(None, 7, 0, 0)),
        run(None, usage(Some("cacheless"), 0, 40, 50)),
        run(None, usage(Some("cacheless"), 1, 0, 0)),
        run(None, usage(Some("gpt-6-luna"), 1, 2, 3)),
        run(None, usage(Some("unknown"), 0, 0, 0)),
        run(None, Value::Null),
    ];
    task.runs.push(parent);
    store.save(&task).unwrap();
    // When: the real CLI refreshes evidence and produces its check JSON.
    let value = invoke(tmp.path(), &["pricing", "check", "--json"]);
    let text = output(tmp.path(), &["pricing", "check"]);
    // Then: only affected cache categories count, explicit zero never falls back.
    assert_eq!(value["catalog_date"], "2026-10-15");
    assert!(text.status.success(), "{text:?}");
    assert_eq!(value["unpriced_tokens"], 113);
    assert_eq!(
        value["unpriced_models"],
        json!([
            {"model":null,"entries":1,"tokens":7},
            {"model":"unknown","entries":1,"tokens":0},
            {"model":"unlisted","entries":1,"tokens":106}
        ])
    );
    assert_eq!(
        value["cache_rate_fallbacks"],
        json!([
            {"model":"cacheless","price_source":"config","entries":1,"cache_read_tokens":40,"cache_write_tokens":50},
            {"model":"gpt-6-astra","price_source":"config","entries":1,"cache_read_tokens":20,"cache_write_tokens":0}
        ])
    );
    let shadows = value["shadows"].as_array().unwrap();
    assert_eq!(shadows.len(), 2);
    assert_eq!(shadows[0]["model"], "claude-sonnet-5-5");
    assert_eq!(shadows[0]["config_model"], "alias-owner");
    assert_eq!(shadows[1]["model"], "gpt-6-astra");
    assert_eq!(shadows[1]["config_price"]["input"], 7.0);
    assert_eq!(shadows[1]["catalog_price"]["input"], 10.0);
    let saved = store.get(&task.id).unwrap();
    let priced = &saved.runs[0].usage.as_ref().unwrap().entries[0];
    assert!((priced.estimated_usd.unwrap() - 7.00014).abs() < 1e-10);
    assert_eq!(priced.price_source.as_deref(), Some("config"));
}

#[test]
fn doctor_cli_includes_soft_pricing_warning_for_recent_unknown_tokens() {
    // Given: recent usage with no price, in an isolated workspace.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store
        .create("Doctor coverage", None, Status::Done, "")
        .unwrap();
    let mut recent = run(Some("unlisted"), usage(Some("unlisted"), 23, 0, 0));
    recent.started = jiff::Timestamp::now()
        .checked_sub(jiff::SignedDuration::from_secs(86400))
        .unwrap();
    recent.ended = Some(recent.started);
    task.runs.push(recent);
    store.save(&task).unwrap();
    // When: the real doctor command produces its structured report.
    let output = output(tmp.path(), &["doctor", "--json"]);
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    // Then: unrelated setup failures do not hide the soft pricing warning.
    assert_eq!(output.status.code(), Some(1));
    let pricing = value["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "pricing_coverage")
        .unwrap();
    assert_eq!(pricing["ok"], false);
    assert_eq!(pricing["hard"], false);
    assert!(pricing["fix"].is_null());
}

#[test]
fn stats_cli_reprices_old_history_from_catalog_without_changing_config() {
    // Given: stored unpriced response evidence and an existing empty pricing table.
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.pricing.clear();
    store.save_config().unwrap();
    let original = std::fs::read(store.config_path()).unwrap();
    let mut task = store
        .create("Catalog history", None, Status::Done, "")
        .unwrap();
    task.runs.push(run(
        Some("gpt-6-astra"),
        usage(Some("gpt-6-astra"), 1_000_000, 0, 0),
    ));
    store.save(&task).unwrap();
    assert_eq!(
        store.get(&task.id).unwrap().runs[0]
            .usage
            .as_ref()
            .unwrap()
            .estimated_usd,
        None
    );
    // When: stats uses the built-in catalog added after the old evidence was saved.
    let rows = invoke(tmp.path(), &["stats", "--by", "task", "--json"]);
    // Then: both the report and stored evidence are catalog-priced, without config seeding.
    assert_eq!(rows[0]["total_estimated_usd"], 10.0);
    assert_eq!(rows[0]["unpriced_tokens"], 0);
    let saved = store.get(&task.id).unwrap();
    let entry = &saved.runs[0].usage.as_ref().unwrap().entries[0];
    assert_eq!(entry.price_source.as_deref(), Some("catalog@2026-10-15"));
    assert!((entry.estimated_usd.unwrap() - 10.0).abs() < 1e-10);
    assert_eq!(std::fs::read(store.config_path()).unwrap(), original);
}
