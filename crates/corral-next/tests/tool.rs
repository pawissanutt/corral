#![cfg(unix)]

use serde_json::Value;
use std::{fs, process::Command};

#[test]
fn tool_lists_and_exchanges_request_in_callers_directory() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join(".corral-next");
    fs::create_dir(&state).unwrap();
    fs::write(root.path().join("fixture.sh"), "#!/bin/sh\ncat\n").unwrap();
    fs::write(
        state.join("corral.toml"),
        r#"
[projects.site]
path = "."
[extensions.fixture.tools]
context = { run = "sh fixture.sh", about = "Find context" }
"#,
    )
    .unwrap();
    let call = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_corral-next"))
            .arg("--root")
            .arg(root.path())
            .args(args)
            .current_dir(root.path())
            .env_remove("CORRAL_TASK")
            .env_remove("CORRAL_RUN")
            .env_remove("CORRAL_ACTOR")
            .env_remove("CORRAL_PROJECT")
            .output()
            .unwrap()
    };
    let list = call(&["tool"]);
    assert!(list.status.success(), "{list:?}");
    let text = String::from_utf8(list.stdout).unwrap();
    assert!(text.contains("context") && text.contains("Find context"));
    let output = call(&["tool", "context", "--example", "value"]);
    assert!(output.status.success(), "{output:?}");
    let request: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(request["v"], 1);
    assert_eq!(request["args"], serde_json::json!(["--example", "value"]));

    // A session targeting another workspace must not import its run identity.
    let other = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_corral-next"))
        .arg("--root")
        .arg(root.path())
        .args(["tool", "context"])
        .current_dir(root.path())
        .env("CORRAL_NEXT_ROOT", other.path())
        .env("CORRAL_TASK", "T-999")
        .env("CORRAL_RUN", "R1")
        .env("CORRAL_PROJECT", "other")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let request: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(request["task"].is_null());
    assert!(request["run"].is_null());
}
