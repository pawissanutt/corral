use std::path::Path;
use std::process::{Command, Output, Stdio};

use corral_core::{PinRef, RetentionAction, Status, Store};
use serde_json::Value;

fn invoke(root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("CORRAL_") || name.starts_with("HERDR_") {
            command.env_remove(key);
        }
    }
    command
        .env("CORRAL_ROOT", root)
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("HERDR_BIN_PATH", "/nonexistent/herdr")
        .env("CORRAL_HERDR_SESSION", "corral-test")
        .arg("--root")
        .arg(root)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn success(root: &Path, args: &[&str]) -> String {
    let output = invoke(root, args);
    assert!(output.status.success(), "{args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn actual_cli_searches_compressed_artifact_filters_snippets_and_source_slice() {
    // Given a real CLI-managed artifact and compression policy, without any external server.
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.artifacts.retention.action = RetentionAction::Compress;
    store.config.artifacts.retention.larger_than = Some("1B".into());
    store.save_config().unwrap();
    let task = store
        .create("CLI investigation", Some("app"), Status::Done, "specmarker")
        .unwrap();
    let source = tmp.path().join("startup.log");
    let text = format!(
        "{}before\nerror: stale review startup_cli\nafter\n",
        "padding\n".repeat(100)
    );
    std::fs::write(&source, &text).unwrap();
    success(
        tmp.path(),
        &[
            "artifact",
            "add",
            &task.id,
            source.to_str().unwrap(),
            "--kind",
            "log",
        ],
    );
    success(tmp.path(), &["artifact", "gc", "--yes"]);
    let artifact = store.get(&task.id).unwrap().artifacts.remove(0);
    assert!(artifact.compressed_bytes.is_some());
    let reference = PinRef::of_artifact(&task.id, &artifact).to_string();

    // When driving JSON/plain search and exact line slicing through the binary.
    let args = [
        "search",
        "error:\"stale review\"",
        "--in",
        "artifacts",
        "--task",
        &task.id,
        "--kind",
        "log",
        "--tier",
        "transient",
        "--since",
        "7d",
        "--project",
        "app",
        "--limit",
        "1",
        "--context",
        "1",
    ];
    let mut json_args = args.to_vec();
    json_args.push("--json");
    let output = success(tmp.path(), &json_args);
    let hits: Vec<Value> = serde_json::from_str(&output).unwrap();
    let plain = success(tmp.path(), &args);
    let slice = success(
        tmp.path(),
        &["artifact", "show", &reference, "--lines", "101-103"],
    );
    let rebuilt = success(
        tmp.path(),
        &[
            "search",
            "error:\"stale review\"",
            "--in",
            "artifacts",
            "--reindex",
            "--json",
        ],
    );
    let rebuilt: Vec<Value> = serde_json::from_str(&rebuilt).unwrap();

    // Then hit metadata, line numbers, context and decoded source bytes agree.
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["artifact_ref"], reference);
    assert_eq!(hits[0]["task"], task.id);
    assert_eq!(hits[0]["run"], "R0");
    assert_eq!(hits[0]["kind"], "log");
    assert_eq!(hits[0]["tier"], "transient");
    assert_eq!(hits[0]["project"], "app");
    assert_eq!(hits[0]["line_start"], 101);
    assert_eq!(hits[0]["line_end"], 103);
    assert_eq!(
        hits[0]["snippet"],
        "before\nerror: stale review startup_cli\nafter"
    );
    assert!(plain.contains(&reference));
    assert_eq!(slice, "before\nerror: stale review startup_cli\nafter\n");
    assert_eq!(rebuilt[0]["artifact_ref"], reference);
    println!("Live compressed CLI hit: {output}Source slice:\n{slice}");
}

#[test]
fn actual_cli_caps_hits_and_unicode_snippets_and_rejects_invalid_options() {
    // Given several matching artifacts with long UTF-8 lines.
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let task = store.create("Budget", None, Status::Backlog, "").unwrap();
    for i in 0..3 {
        let source = tmp.path().join(format!("{i}.log"));
        std::fs::write(
            &source,
            format!("error: needle {i} {}\n", "界".repeat(3000)),
        )
        .unwrap();
        success(
            tmp.path(),
            &["artifact", "add", &task.id, source.to_str().unwrap()],
        );
    }

    // When searching at a small hit budget, rebuilding without a query, and sending invalid options.
    let output = success(
        tmp.path(),
        &["search", "needle", "--in", "all", "--limit", "2", "--json"],
    );
    let hits: Vec<Value> = serde_json::from_str(&output).unwrap();
    success(tmp.path(), &["search", "--reindex", "--json"]);
    let errors = [
        vec!["search", "needle", "--in", "unknown"],
        vec!["search", "needle", "--context", "21"],
        vec!["search", "needle", "--limit", "101"],
        vec!["search", "unknown:value", "--in", "all"],
    ]
    .map(|args| invoke(tmp.path(), &args));

    // Then the actual serialized output is capped and malformed requests fail.
    assert_eq!(hits.len(), 2);
    assert!(
        hits.iter()
            .all(|h| h["snippet"].as_str().unwrap().chars().count() <= 1200)
    );
    assert!(errors.iter().all(|out| !out.status.success()));
    assert!(store.state_dir().join("content-search.json").is_file());
}
