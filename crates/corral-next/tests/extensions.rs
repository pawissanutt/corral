use std::{fs, path::Path, process::Command};

#[cfg(unix)]
use serde_json::{Value, json};

fn cli(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_corral-next"))
        .arg("--root")
        .arg(root)
        .args(args)
        .env_remove("CORRAL_PROJECT")
        .output()
        .unwrap()
}

#[test]
fn config_check_names_unknown_extension_keys_and_both_facts_providers() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join(".corral-next");
    fs::create_dir(&dir).unwrap();
    let file = dir.join("corral.toml");
    let valid = r#"
[projects.site]
path = "."

[extensions.first]
facts = "true"

[extensions.second]
projects = ["site"]
enabled = false
facts = "true"
on = ["landed"]
run = "true"

[extensions.second.tools]
context = { run = "true", about = "Find context", record = true }
"#;
    fs::write(&file, valid).unwrap();
    let output = cli(root.path(), &["config", "check"]);
    assert!(output.status.success(), "{output:?}");

    fs::write(&file, valid.replace("record = true", "recrod = true")).unwrap();
    let output = cli(root.path(), &["config", "check"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("recrod"));

    fs::write(&file, valid.replace("enabled = false", "enabled = true")).unwrap();
    let output = cli(root.path(), &["config", "check"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).lines().any(|line| {
        ["facts", "site", "first", "second"]
            .iter()
            .all(|term| line.contains(term))
    }));
}

#[cfg(unix)]
#[test]
fn ext_check_runs_every_line_once_even_after_a_failure() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join(".corral-next");
    fs::create_dir(&dir).unwrap();
    fs::create_dir(root.path().join("site")).unwrap();
    fs::write(
        dir.join("corral.toml"),
        r#"
[projects.site]
path = "site"
env = { SAMPLE_VAR = "from-project", CORRAL_ACTOR = "overridden" }

[extensions.example]
projects = ["site"]
enabled = false
facts = 'printf "facts\n" >> calls; cat'
on = ["blocked"]
run = 'printf "run\n" >> calls; printf "broken\n" >&2; exit 9'

[extensions.example.tools]
info = { run = 'printf "tool\n" >> calls; printf "%s\n" "$SAMPLE_VAR/$CORRAL_PROJECT/$CORRAL_EXTENSION/$CORRAL_ACTOR" "$CORRAL_NEXT_ROOT"; cat' }
"#,
    )
    .unwrap();
    let output = cli(root.path(), &["ext", "check", "example", "--json"]);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let report: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report.len(), 3);
    assert_eq!(report[0]["line"], "facts");
    let request: Value = serde_json::from_str(report[0]["stdout"].as_str().unwrap()).unwrap();
    assert_eq!(request["v"], 1);
    assert_eq!(request["query"], "snapshot");
    assert_eq!(report[1]["line"], "run");
    assert!(report[1]["error"].as_str().unwrap().contains("broken"));
    assert_eq!(report[2]["line"], "tools.info");
    let mut stdout = report[2]["stdout"].as_str().unwrap().lines();
    assert_eq!(stdout.next(), Some("from-project/site/example/extension"));
    let canonical_root = root.path().canonicalize().unwrap();
    assert_eq!(stdout.next(), canonical_root.to_str());
    assert_eq!(
        serde_json::from_str::<Value>(stdout.next().unwrap()).unwrap(),
        json!({"args": [], "sample": true, "v": 1})
    );
    assert_eq!(stdout.next(), None);
    assert_eq!(
        fs::read_to_string(root.path().join("site/calls")).unwrap(),
        "facts\nrun\ntool\n"
    );
}
