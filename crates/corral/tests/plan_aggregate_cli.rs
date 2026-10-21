//! The real CLI aggregate acceptance workflow against a fake herdr whose `pane run` executes the
//! reviewer command synchronously. No live herdr session is touched.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use corral_core::{AgentCmd, Origin, Status, Store};
use serde_json::{Value, json};

/// Extracts the exact verdict command from its prompt, writes notes and approves.
const REVIEWER: &str = r#"#!/bin/sh
set -eu
cmd=$(grep 'plan aggregate-verdict' "$1" | tail -n 1 | sed 's/approve|changes|escalate/approve/')
notes=$(printf '%s\n' "$cmd" | sed "s/.*--notes-file '\(.*\)'$/\1/")
printf 'Combined candidate satisfies C1 and C2 together.\n' > "$notes"
eval "$cmd" > "$(dirname "$0")/verdict.out" 2>&1
"#;

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap().trim().into()
}

fn commit(repo: &Path, message: &str) -> String {
    git(repo, &["add", "-A"]);
    git(repo, &["commit", "-qm", message]);
    git(repo, &["rev-parse", "HEAD"])
}

fn cli(root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
    for (name, _) in std::env::vars_os() {
        let name = name.to_string_lossy().into_owned();
        if name.starts_with("CORRAL_") || name.starts_with("HERDR_") {
            command.env_remove(name);
        }
    }
    command
        .env("CORRAL_ROOT", root)
        .env("CORRAL_HERDR_SESSION", "corral-test")
        .env("HERDR_BIN_PATH", root.join("herdr"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .arg("--root")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn json_ok(root: &Path, args: &[&str]) -> Value {
    let output = cli(root, args);
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// A direct-to-main Plan whose two members are accepted on main.
fn fixture(root: &Path) -> (Store, String) {
    let repo = root.join("app");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-qb", "main"]);
    git(&repo, &["config", "user.name", "Fixture"]);
    git(&repo, &["config", "user.email", "fixture@example.invalid"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    std::fs::write(repo.join(".gitignore"), ".worktrees/\n").unwrap();
    let mut base = commit(&repo, "initial");
    std::os::unix::fs::symlink(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../corral-ops/tests/gate_ops/herdr.sh"),
        root.join("herdr"),
    )
    .unwrap();
    let reviewer = root.join("reviewer.sh");
    std::fs::write(&reviewer, REVIEWER).unwrap();
    let mut store = Store::init(root).unwrap();
    store.config.review.gate = Some(vec![
        "test -f first.txt".into(),
        "test -f second.txt".into(),
    ]);
    store.config.review.reviewer = Some("rev".into());
    store.config.agents.insert(
        "rev".into(),
        AgentCmd {
            command: format!("sh {} {{prompt_file}}", reviewer.display()),
        },
    );
    store.save_config().unwrap();
    let source = root.join("plan.md");
    std::fs::write(
        &source,
        "---\ntitle: Plan\nproject: app\napproved_scope:\n  goal: Combined change\n  acceptance:\n    - {criterion: C1, scenario: first works, expected: pass}\n    - {criterion: C2, scenario: both work together, expected: pass together}\ntasks:\n  - {key: first, title: First}\n  - {key: second, title: Second}\n---\n",
    )
    .unwrap();
    let ctx = corral_ops::Ctx::from_store(store.clone());
    let imported = corral_ops::import_plan(&ctx, &source, false, Origin::Agent).unwrap();
    for (member, file) in imported.tasks.iter().zip(["first.txt", "second.txt"]) {
        std::fs::write(repo.join(file), "accepted work\n").unwrap();
        let sha = commit(&repo, &member.id);
        store
            .update(&member.id, |task| {
                task.status = Status::Done;
                task.runs = vec![
                    serde_json::from_value(json!({
                        "id":"R1", "agent":"fixture", "worktree":"/unused",
                        "branch":format!("corral/{}", member.id), "base":"main", "base_sha":base,
                        "started":"2026-10-19T00:00:00Z", "outcome":"done",
                    }))
                    .unwrap(),
                ];
                Ok(())
            })
            .unwrap();
        store
            .log_event(
                Some(&member.id),
                "approved",
                json!({"run":"R1", "sha":sha, "strategy":"fast_forward", "base":"main"}),
            )
            .unwrap();
        base = sha;
    }
    (store, imported.tracker.id)
}

fn observe(root: &Path, plan: &str, attempt: &str, criterion: &str, expected: &str) {
    let file: PathBuf = root.join(format!("{criterion}.json"));
    std::fs::write(
        &file,
        serde_json::to_vec(&json!({
            "version":1, "key":format!("{attempt}-{criterion}"), "criterion":criterion,
            "run":"aggregate", "attempt":attempt, "status":"pass",
            "action":"run the combined scenario", "environment":"fixture",
            "expected":expected, "observed":"observed", "artifacts":[],
            "captured":jiff::Timestamp::now(), "producer":"test", "live":false,
        }))
        .unwrap(),
    )
    .unwrap();
    json_ok(
        root,
        &[
            "plan",
            "evidence",
            plan,
            "--file",
            file.to_str().unwrap(),
            "--json",
        ],
    );
}

#[test]
fn cli_owner_acceptance_requires_a_launched_aggregate_review() {
    // Given a verified Plan with passing aggregate scenarios and a configured reviewer.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (store, plan) = fixture(&root);
    let attempt = json_ok(&root, &["plan", "verify", &plan, "--json"]);
    assert_eq!(attempt["gate"]["result"], "pass", "{attempt}");
    let attempt = attempt["id"].as_str().unwrap().to_owned();
    observe(&root, &plan, &attempt, "C1", "pass");
    observe(&root, &plan, &attempt, "C2", "pass together");
    let head = git(&root.join("app"), &["rev-parse", "HEAD"]);

    // When the owner accepts before any aggregate review, the CLI refuses.
    let refused = cli(
        &root,
        &["plan", "accept", &plan, "--attempt", &attempt, "--json"],
    );
    assert!(!refused.status.success(), "{refused:?}");
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("aggregate review missing or pending"),
        "{refused:?}"
    );
    assert_ne!(store.get(&plan).unwrap().status, Status::Done);

    // And a callback for a review that was never launched is refused.
    let notes = root.join("notes.md");
    std::fs::write(&notes, "approve\n").unwrap();
    let unlaunched = cli(
        &root,
        &[
            "plan",
            "aggregate-verdict",
            &plan,
            "approve",
            "--attempt",
            &attempt,
            "--review",
            "A1-R1",
            "--notes-file",
            notes.to_str().unwrap(),
        ],
    );
    assert!(!unlaunched.status.success(), "{unlaunched:?}");

    // When the configured reviewer is launched and answers through its exact callback.
    let review = json_ok(
        &root,
        &[
            "plan",
            "aggregate-review",
            &plan,
            "--attempt",
            &attempt,
            "--json",
        ],
    );
    let callback = std::fs::read_to_string(root.join("verdict.out")).unwrap_or_default();
    assert_eq!(review["id"], format!("{attempt}-R1"), "{review}");
    assert_eq!(review["reviewer"], "rev", "{review}");
    assert_eq!(review["verdict"], "approve", "{review} {callback}");
    assert!(review["herdr"]["pane_id"].is_string(), "{review}");

    // Then owner acceptance lands once without changing main.
    let acceptance = json_ok(
        &root,
        &["plan", "accept", &plan, "--attempt", &attempt, "--json"],
    );
    assert_eq!(acceptance["by"], "owner", "{acceptance}");
    assert_eq!(acceptance["automatic"], false, "{acceptance}");
    assert_eq!(acceptance["state"], "landed", "{acceptance}");
    assert_eq!(store.get(&plan).unwrap().status, Status::Done);
    assert_eq!(git(&root.join("app"), &["rev-parse", "HEAD"]), head);
}
