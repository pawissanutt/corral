use std::{path::Path, process::Command};

use serde_json::{Value, json};

fn cli(root: &Path, args: &[&str]) -> Vec<u8> {
    let output = Command::new(env!("CARGO_BIN_EXE_corral-next"))
        .arg("--root")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn json_cli(root: &Path, args: &[&str]) -> Value {
    let mut options = vec!["--json"];
    options.extend_from_slice(args);
    serde_json::from_slice(&cli(root, &options)).unwrap()
}

#[test]
fn task_metadata_survives_cli_reopen_and_supports_replacement_and_clearing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    cli(root, &["init"]);
    let plain = json_cli(root, &["new", "No estimates"]);
    let tagged = json_cli(
        root,
        &[
            "new",
            "Small but hard",
            "--size",
            "xs",
            "--difficulty",
            "hard",
            "--tag",
            "bug",
            "--tag",
            "Payments, EU",
            "--tag",
            "bug",
        ],
    );
    let plain_id = plain["task"]["id"].as_str().unwrap();
    let tagged_id = tagged["task"]["id"].as_str().unwrap();
    let list = json_cli(root, &["list"]);
    let tasks = list.as_array().unwrap();
    assert_eq!(tasks[0]["id"], plain_id);
    assert_eq!(tasks[0]["size"], Value::Null);
    assert_eq!(tasks[0]["difficulty"], Value::Null);
    assert_eq!(tasks[0]["tags"], json!([]));
    assert_eq!(tasks[1]["id"], tagged_id);
    assert_eq!(tasks[1]["size"], "xs");
    assert_eq!(tasks[1]["difficulty"], "hard");
    assert_eq!(tasks[1]["tags"], json!(["bug", "Payments, EU"]));
    let text = String::from_utf8(cli(root, &["show", plain_id])).unwrap();
    assert!(text.contains("Size: unset"));
    assert!(text.contains("Difficulty: unset"));
    let text = String::from_utf8(cli(root, &["list"])).unwrap();
    assert!(text.lines().any(|line| line.starts_with(tagged_id)
        && line.contains("xs")
        && line.contains("hard")
        && line.contains("Payments, EU")));

    cli(
        root,
        &["edit", tagged_id, "--size", "l", "--tag", "Payments, EU"],
    );
    let edited = json_cli(root, &["show", tagged_id]);
    assert_eq!(edited["task"]["size"], "l");
    assert_eq!(edited["task"]["difficulty"], "hard");
    assert_eq!(edited["task"]["tags"], json!(["Payments, EU"]));

    cli(root, &["edit", tagged_id, "--difficulty", "standard"]);
    assert_eq!(
        json_cli(root, &["show", tagged_id])["task"]["difficulty"],
        "standard"
    );
    cli(
        root,
        &[
            "edit",
            tagged_id,
            "--size=",
            "--difficulty=",
            "--clear-tags",
        ],
    );
    let cleared = json_cli(root, &["show", tagged_id]);
    assert_eq!(cleared["task"]["size"], Value::Null);
    assert_eq!(cleared["task"]["difficulty"], Value::Null);
    assert_eq!(cleared["task"]["tags"], json!([]));
}
