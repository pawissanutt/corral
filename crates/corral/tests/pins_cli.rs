use std::path::Path;
use std::process::{Command, Output};

fn corral(root: &Path, opener_log: &Path, args: &[&str]) -> Output {
    let script = opener_log.with_extension("sh");
    std::fs::write(
        &script,
        format!("printf '%s\\n' \"$1\" >> '{}'\n", opener_log.display()),
    )
    .expect("write opener stub");
    Command::new(env!("CARGO_BIN_EXE_corral"))
        .env("CORRAL_ROOT", root)
        .env_remove("CORRAL_GATE_ATTEMPT")
        .env_remove("CORRAL_CANDIDATE_HEAD")
        .env_remove("CORRAL_INTEGRATION_BASE")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_WORKSPACE_ID")
        .env_remove("HERDR_TAB_ID")
        .env_remove("CORRAL_HERDR_SESSION")
        .env("HERDR_BIN_PATH", "/nonexistent/herdr")
        .env("CORRAL_OPENER", format!("sh {}", script.display()))
        .arg("--root")
        .arg(root)
        .args(args)
        .current_dir(root)
        .output()
        .expect("corral runs")
}

fn ok(root: &Path, log: &Path, args: &[&str]) -> String {
    let out = corral(root, log, args);
    assert!(
        out.status.success(),
        "corral {args:?} failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn pins_json(root: &Path, log: &Path) -> Vec<serde_json::Value> {
    let v: serde_json::Value =
        serde_json::from_str(&ok(root, log, &["pins", "--json"])).expect("json");
    v.as_array().expect("array").clone()
}

#[test]
fn pin_unpin_pins_json_and_open() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().canonicalize().expect("canonical");
    let log = root.join("opened.log");
    ok(&root, &log, &["init"]);
    ok(&root, &log, &["new", "Dashboards"]);
    let report = root.join("dash.html");
    std::fs::write(&report, "v1\n").expect("write");
    let report_s = report.to_str().expect("utf8");
    ok(&root, &log, &["artifact", "add", "T-001", report_s]);

    let out = ok(
        &root,
        &log,
        &[
            "pin",
            "T-001:R0:dash.html",
            "--label",
            "Dash",
            "--follow-latest",
        ],
    );
    assert!(out.starts_with("pinned #1 T-001:R0:"), "{out}");
    ok(&root, &log, &["pin", "T-001"]);

    let pins = pins_json(&root, &log);
    assert_eq!(pins.len(), 2);
    assert_eq!(pins[0]["index"], 1);
    assert_eq!(pins[0]["label"], "Dash");
    assert_eq!(pins[0]["follow"], "latest");
    assert_eq!(pins[0]["state"], "ok");
    assert_eq!(pins[0]["kind"], "artifact");
    assert_eq!(pins[1]["kind"], "task");
    assert_eq!(pins[1]["title"], "Dashboards");
    let first_path = pins[0]["path"].as_str().expect("path").to_string();
    assert!(first_path.ends_with("/2d27fbdf-dash.html"));

    // Identical bytes deliberately dedupe; follow-latest stays on that snapshot.
    ok(&root, &log, &["artifact", "add", "T-001", report_s]);
    let pins = pins_json(&root, &log);
    assert_eq!(pins[0]["path"], first_path);
    assert_eq!(pins[0]["followed"], false);

    std::fs::write(&report, "v2\n").expect("rewrite");
    ok(&root, &log, &["artifact", "add", "T-001", report_s]);
    let pins = pins_json(&root, &log);
    let latest = pins[0]["path"].as_str().expect("path").to_string();
    assert_ne!(
        latest, first_path,
        "follow-latest moves to the new snapshot"
    );
    assert!(latest.ends_with("/81db67b6-dash.html"));
    assert_eq!(pins[0]["followed"], true);
    assert_eq!(std::fs::read_to_string(&latest).expect("snapshot"), "v2\n");

    ok(&root, &log, &["open", "1"]);
    ok(&root, &log, &["open", "T-001"]);
    let opened = std::fs::read_to_string(&log).expect("opener ran");
    let lines: Vec<&str> = opened.lines().collect();
    assert_eq!(lines.len(), 2, "{opened}");
    assert_eq!(lines[0], latest);
    assert!(
        lines[1].ends_with(".md") && lines[1].contains("T-001"),
        "{opened}"
    );
    assert!(!corral(&root, &log, &["open", "7"]).status.success());

    ok(&root, &log, &["unpin", "T-001"]);
    let pins = pins_json(&root, &log);
    assert_eq!(pins.len(), 1);

    std::fs::remove_file(&latest).expect("remove snapshot");
    let pins = pins_json(&root, &log);
    assert_eq!(pins[0]["state"], "broken");
    assert!(ok(&root, &log, &["pins"]).contains("BROKEN"));
    assert!(!corral(&root, &log, &["open", "1"]).status.success());
    ok(&root, &log, &["unpin", "1"]);
    assert!(pins_json(&root, &log).is_empty());
}
