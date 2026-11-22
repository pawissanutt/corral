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
fn ext_check_with_empty_project_scope_runs_at_workspace_root() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join(".corral-next");
    fs::create_dir(&dir).unwrap();
    fs::create_dir(root.path().join("site")).unwrap();
    fs::write(
        dir.join("corral.toml"),
        r#"
[projects.site]
path = "site"

[extensions.example]
projects = []
facts = 'test -d .corral-next && test -z "$CORRAL_PROJECT" && printf checked'
"#,
    )
    .unwrap();
    let output = cli(root.path(), &["config", "check"]);
    assert!(output.status.success(), "{output:?}");

    let output = Command::new(env!("CARGO_BIN_EXE_corral-next"))
        .arg("--root")
        .arg(root.path())
        .args(["ext", "check", "example", "--json"])
        .env("CORRAL_PROJECT", "site")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report, json!([{"line": "facts", "stdout": "checked"}]));
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

#[cfg(unix)]
#[test]
fn ext_deliver_resumes_after_a_crash_in_order_and_skips_after_three_failures() {
    use std::os::unix::process::ExitStatusExt;

    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    assert!(cli(root, &["init"]).status.success());
    let config = root.join(".corral-next/corral.toml");
    let mut text = fs::read_to_string(&config).unwrap();
    // The first delivery kills `ext deliver` (its supervisor's parent) mid-delivery and
    // exits, leaving a child in its process group that holds stdout open.
    text.push_str(
        r#"
[extensions.fail]
on = ["commented"]
run = "exit 3"

[extensions.log]
on = ["created"]
run = '''
if [ ! -e crashed ]; then
  touch crashed; echo old-start >> trace
  sleep 90 & echo $! > child
  kill -9 $(ps -o ppid= -p $PPID); exit 0
fi
kill -0 $(cat child) 2>/dev/null && echo overlap >> trace
echo deliver >> trace; cat >> delivered; echo >> delivered
'''
"#,
    );
    fs::write(&config, text).unwrap();
    let deliver = || cli(root, &["--json", "ext", "deliver"]);
    let id = |output: std::process::Output| -> String {
        assert!(output.status.success(), "{output:?}");
        let task: Value = serde_json::from_slice(&output.stdout).unwrap();
        task["task"]["id"].as_str().unwrap().to_owned()
    };

    id(cli(root, &["--json", "new", "History"]));
    assert!(deliver().status.success());
    let first = id(cli(root, &["--json", "new", "First"]));
    let second = id(cli(root, &["--json", "new", "Second"]));
    assert_eq!(deliver().status.signal(), Some(9));
    // The interrupted attempt owns delivery for `log` until it has been stopped.
    let log_busy = || {
        let output = deliver();
        assert!(output.status.success(), "{output:?}");
        let reports: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(reports[1]["extension"], "log");
        reports[1]["busy"].as_bool().unwrap()
    };
    wait_for(|| !log_busy());
    assert_eq!(
        fs::read_to_string(root.join("trace")).unwrap(),
        "old-start\ndeliver\ndeliver\n"
    );
    let delivered: Vec<Value> = fs::read_to_string(root.join("delivered"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let tasks: Vec<_> = delivered
        .iter()
        .map(|event| event["task"].clone())
        .collect();
    assert_eq!(tasks, [json!(first), json!(second)]);
    assert!(
        delivered
            .iter()
            .all(|event| event["kind"] == "created" && event["v"] == 1)
    );
    assert!(delivered[0]["seq"].as_u64() < delivered[1]["seq"].as_u64());

    assert!(cli(root, &["comment", &first, "note"]).status.success());
    for _ in 0..2 {
        assert_eq!(deliver().status.code(), Some(1));
    }
    let output = deliver();
    assert!(output.status.success(), "{output:?}");
    let reports: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(reports[0]["extension"], "fail");
    assert_eq!(reports[0]["skipped"], 1);
    let cursor: Value =
        serde_json::from_slice(&fs::read(root.join(".corral-next/extensions/fail.json")).unwrap())
            .unwrap();
    assert_eq!(cursor["skipped"], 1);
    assert!(cursor["last_error"].as_str().unwrap().contains("status 3"));
}

#[cfg(unix)]
fn wait_for(mut done: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !done() {
        assert!(
            std::time::Instant::now() < deadline,
            "condition not reached"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[cfg(unix)]
#[test]
fn ext_deliver_starts_a_new_extension_at_the_head_seen_with_its_configuration() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    assert!(cli(root, &["init"]).status.success());
    let config = root.join(".corral-next/corral.toml");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str(
        r#"
[extensions.a]
on = ["created"]
run = 'touch a-started; i=0; while [ ! -e a-release ] && [ $i -lt 1000 ]; do sleep 0.02; i=$((i+1)); done'
"#,
    );
    fs::write(&config, &text).unwrap();
    let new = |title: &str| {
        let output = cli(root, &["--json", "new", title]);
        assert!(output.status.success(), "{output:?}");
        let task: Value = serde_json::from_slice(&output.stdout).unwrap();
        task["task"]["id"].as_str().unwrap().to_owned()
    };
    assert!(cli(root, &["ext", "deliver"]).status.success());
    new("A");
    text.push_str("\n[extensions.b]\non = [\"created\"]\nrun = 'cat >> b; echo >> b'\n");
    fs::write(&config, &text).unwrap();
    // This pass observes `b`, then blocks delivering A to `a` while B is created.
    let pass = Command::new(env!("CARGO_BIN_EXE_corral-next"))
        .arg("--root")
        .arg(root)
        .args(["ext", "deliver"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    wait_for(|| root.join("a-started").exists());
    let b = new("B");
    fs::write(root.join("a-release"), "").unwrap();
    assert!(pass.wait_with_output().unwrap().status.success());
    let got: Vec<Value> = fs::read_to_string(root.join("b"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0]["task"], json!(b));
}

#[cfg(unix)]
fn deliver_report(root: &Path) -> Value {
    let output = cli(root, &["--json", "ext", "deliver"]);
    assert!(output.status.success(), "{output:?}");
    let reports: Value = serde_json::from_slice(&output.stdout).unwrap();
    reports[0].clone()
}

#[cfg(unix)]
fn react_workspace(run: &str) -> (tempfile::TempDir, impl Fn(&str) -> u64) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_owned();
    assert!(cli(&root, &["init"]).status.success());
    let config = root.join(".corral-next/corral.toml");
    let mut text = fs::read_to_string(&config).unwrap();
    text.push_str(&format!(
        "\n[extensions.log]\non = [\"created\"]\nrun = '{run}'\n"
    ));
    fs::write(&config, text).unwrap();
    assert!(cli(&root, &["ext", "deliver"]).status.success());
    let new = move |title: &str| {
        let output = cli(&root, &["--json", "new", title]);
        assert!(output.status.success(), "{output:?}");
        let applied: Value = serde_json::from_slice(&output.stdout).unwrap();
        applied["seq"].as_u64().unwrap()
    };
    (dir, new)
}

#[cfg(unix)]
#[test]
fn ext_deliver_stops_a_hung_line_when_its_pass_dies() {
    use std::os::unix::process::ExitStatusExt;

    let (dir, new) = react_workspace(
        "if [ ! -e crashed ]; then touch crashed; kill -9 $(ps -o ppid= -p $PPID); \
         exec sleep 90; fi; cat >> delivered; echo >> delivered",
    );
    let root = dir.path();
    let a = new("A");
    let output = cli(root, &["--json", "ext", "deliver"]);
    assert_eq!(output.status.signal(), Some(9), "{output:?}");
    // Well before the 60-second deadline.
    wait_for(|| deliver_report(root)["busy"] == false && root.join("delivered").exists());
    let delivered: Value =
        serde_json::from_str(&fs::read_to_string(root.join("delivered")).unwrap()).unwrap();
    assert_eq!(delivered["seq"], a);
}

#[cfg(unix)]
#[test]
fn ext_deliver_ends_an_attempt_with_its_line() {
    let (dir, new) = react_workspace(
        "sleep 90 >/dev/null 2>&1 & echo $! >> bg; cat >> delivered; echo >> delivered",
    );
    let root = dir.path();
    new("A");
    new("B");
    let report = deliver_report(root);
    assert_eq!(
        (report["busy"].clone(), report["delivered"].clone()),
        (json!(false), json!(2))
    );
    for pid in fs::read_to_string(root.join("bg")).unwrap().lines() {
        wait_for(|| {
            !Command::new("kill")
                .args(["-0", pid])
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success()
        });
    }
    new("C");
    let report = deliver_report(root);
    assert_eq!(
        (report["busy"].clone(), report["delivered"].clone()),
        (json!(false), json!(1))
    );
}

#[cfg(target_os = "linux")]
#[test]
fn ext_deliver_is_not_held_up_by_a_process_that_left_its_group() {
    // Every attempt leaves a process in its own session; the first one also fails.
    let (dir, new) = react_workspace(
        "setsid sleep 90 </dev/null >/dev/null 2>&1 & echo $! >> escaped; \
         if [ ! -e failed ]; then touch failed; exit 3; fi; cat >> delivered; echo >> delivered",
    );
    let root = dir.path();
    new("A");
    new("B");
    let output = cli(root, &["--json", "ext", "deliver"]);
    let failed: Value = serde_json::from_slice(&output.stdout).unwrap();
    let retried = deliver_report(root);
    new("C");
    let later = deliver_report(root);
    for pid in fs::read_to_string(root.join("escaped")).unwrap().lines() {
        let _ = Command::new("kill").args(["-9", pid]).status();
    }
    assert_eq!(failed[0]["failed"], true, "{failed}");
    for report in [&failed[0], &retried, &later] {
        assert_eq!(report["busy"], false, "{report}");
    }
    assert_eq!(
        (retried["delivered"].clone(), later["delivered"].clone()),
        (json!(2), json!(1))
    );
}
