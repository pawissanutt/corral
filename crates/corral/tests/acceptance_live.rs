//! Real CLI and real herdr transport; no model claims or timing-based waits.
use std::path::Path;
use std::process::{Command, Output, Stdio};

use corral_core::{Accept, AgentCmd, AutoStage, Status, Store};
use corral_herdr::Herdr;
use serde_json::{Value, json};

#[path = "acceptance_live/verdict_carry.rs"]
mod verdict_carry;

fn cli(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_corral"))
        .env_remove("CORRAL_ROOT")
        .env_remove("HERDR_BIN_PATH")
        .env("CORRAL_HERDR_SESSION", "corral-test")
        .arg("--root")
        .arg(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn ok(root: &Path, args: &[&str]) -> Output {
    let out = cli(root, args);
    assert!(
        out.status.success(),
        "{args:?}: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn checkpoint(root: &Path, id: &str) {
    let out = Command::new("bash")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/checkpoint-fixture.sh"
        ))
        .arg(env!("CARGO_BIN_EXE_corral"))
        .arg(id)
        .env("CORRAL_ROOT", root)
        .env_remove("HERDR_BIN_PATH")
        .env("CORRAL_HERDR_SESSION", "corral-test")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

struct Panes {
    herdr: Herdr,
    ids: Vec<String>,
}

impl Drop for Panes {
    fn drop(&mut self) {
        for pane in &self.ids {
            if let Err(error) = self.herdr.pane_close(pane) {
                eprintln!("isolated pane cleanup {pane}: {error:#}");
            }
        }
    }
}

fn live_fixture(accept: Accept) -> Option<(tempfile::TempDir, Store, Panes, String)> {
    let herdr = Herdr::with_bin("herdr".into(), Some("corral-test".into()));
    if !herdr.available() {
        println!("SKIP: herdr session corral-test unavailable");
        return None;
    }
    let tmp = tempfile::tempdir().unwrap();
    let app = tmp.path().join("app");
    std::fs::create_dir(&app).unwrap();
    git(&app, &["init", "-q", "-b", "main"]);
    git(&app, &["config", "user.name", "corral acceptance fixture"]);
    git(&app, &["config", "user.email", "corral@test.invalid"]);
    git(&app, &["config", "commit.gpgsign", "false"]);
    std::fs::write(app.join("initial.txt"), "initial\n").unwrap();
    git(&app, &["add", "-A"]);
    git(&app, &["commit", "-q", "-m", "initial"]);
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.herdr.session = Some("corral-test".into());
    for profile in ["generic-worker", "generic-reviewer"] {
        store.config.agents.insert(
            profile.into(),
            AgentCmd {
                command: "true".into(),
            },
        );
    }
    store.config.review.gate = Some(vec![
        "test \"$(git status --porcelain)\" = ''".into(),
        "test -f widget.txt".into(),
    ]);
    store.config.review.reviewer = Some("generic-reviewer".into());
    store.config.review.accept = Some(accept);
    store.config.review.remove_worktree = Some(false);
    store.save_config().unwrap();
    let id = String::from_utf8(
        ok(
            tmp.path(),
            &[
                "new",
                "T052 live fixture",
                "-p",
                "app",
                "--ready",
                "--body",
                "The widget exists.",
            ],
        )
        .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    ok(
        tmp.path(),
        &["dispatch", &id, "--profile", "generic-worker"],
    );
    let task = store.get(&id).unwrap();
    let run = task.current_run().unwrap();
    let panes = Panes {
        herdr,
        ids: vec![run.herdr.as_ref().unwrap().pane_id.clone()],
    };
    std::fs::write(run.worktree.join("widget.txt"), "observed\n").unwrap();
    git(&run.worktree, &["add", "-A"]);
    git(&run.worktree, &["commit", "-q", "-m", "widget"]);
    Some((tmp, store, panes, id))
}

const SUMMARY: &str =
    "Added widget.\nVerified: live file check and corral gate passed.\nNot verified: none";

fn manifest(root: &Path, id: &str) {
    let listed: Value =
        serde_json::from_slice(&ok(root, &["criteria", "list", id]).stdout).unwrap();
    let file = root.join("criteria.json");
    std::fs::write(&file, serde_json::to_vec_pretty(&json!({
        "version":1, "spec_digest":listed["spec_digest"], "reviewed_by":"fixture owner",
        "audited":true, "require_gate":true, "criteria":[
            {"id":"C1", "required":true, "live":true, "scenario":"inspect actual widget", "expected":"widget exists"}
        ]
    })).unwrap()).unwrap();
    ok(
        root,
        &["criteria", "register", id, "--file", file.to_str().unwrap()],
    );
}

#[test]
fn isolated_gate_review_land_preserves_exact_receipt_identities() {
    let Some((tmp, store, mut panes, id)) = live_fixture(Accept::Auto) else {
        return;
    };
    let root = tmp.path();
    manifest(root, &id);
    ok(root, &["gate", &id, "--wait"]);
    let task = store.get(&id).unwrap();
    let run = task.current_run().unwrap();
    let gate = run.gate.as_ref().unwrap();
    panes.ids.push(gate.pane.clone().unwrap());
    let identity = gate.identity.clone().unwrap();
    let log = root.join("widget-live.log");
    std::fs::write(
        &log,
        format!(
            "HEAD={}\nobserved={}",
            identity.head,
            std::fs::read_to_string(run.worktree.join("widget.txt")).unwrap()
        ),
    )
    .unwrap();
    ok(
        root,
        &[
            "artifact",
            "add",
            &id,
            log.to_str().unwrap(),
            "--kind",
            "evidence",
            "--title",
            "Live widget",
        ],
    );
    let task = store.get(&id).unwrap();
    let artifact = task
        .artifacts
        .iter()
        .find(|a| a.title == "Live widget")
        .unwrap();
    let file = root.join("observation.json");
    std::fs::write(&file, serde_json::to_vec_pretty(&json!({
        "version":1, "key":"live-widget", "criterion":"C1", "run":identity.run,
        "attempt":identity.attempt, "status":"pass", "action":"read actual widget.txt",
        "environment":"corral-test", "expected":"widget exists", "observed":"widget exists",
        "artifacts":[artifact.path], "captured":jiff::Timestamp::now(), "producer":"generic-fixture",
        "live":true, "cleanup":"fixture owns and closes isolated panes"
    })).unwrap()).unwrap();
    ok(
        root,
        &["evidence", "record", &id, "--file", file.to_str().unwrap()],
    );
    let evidence: Value =
        serde_json::from_slice(&ok(root, &["evidence", "list", &id, "--json"]).stdout).unwrap();
    let audit: Value = serde_json::from_slice(
        &ok(
            root,
            &["done", &id, "--check", "--json", "--summary", SUMMARY],
        )
        .stdout,
    )
    .unwrap();
    assert_eq!(audit["identity"], serde_json::to_value(&identity).unwrap());
    assert_eq!(evidence[0]["identity"], audit["identity"]);
    checkpoint(root, &id);
    ok(root, &["done", &id, "--summary", SUMMARY]);
    assert_eq!(store.get(&id).unwrap().status, Status::Review);
    ok(root, &["sync"]);
    let reviewed = store.get(&id).unwrap();
    let review = reviewed.current_run().unwrap().review.as_ref().unwrap();
    panes.ids.push(review.pane.clone().unwrap());
    assert_eq!(review.identity.as_ref(), Some(&identity));
    // Submit from a different process environment than the gate-starting CLI.
    let mut reviewer_paths = vec![root.join("reviewer-only-bin")];
    reviewer_paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let reviewer_path = std::env::join_paths(reviewer_paths).unwrap();
    let verdict = Command::new(env!("CARGO_BIN_EXE_corral"))
        .env_remove("CORRAL_ROOT")
        .env_remove("HERDR_BIN_PATH")
        .env("CORRAL_HERDR_SESSION", "corral-test")
        .env("PATH", &reviewer_path)
        .env("RUSTFLAGS", "--cfg reviewer_only")
        .arg("--root")
        .arg(root)
        .args([
            "verdict",
            &id,
            "approve",
            "--attempt",
            &identity.attempt,
            "--notes",
            "Observed criterion C1 in the fixture.",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        verdict.status.success(),
        "{}",
        String::from_utf8_lossy(&verdict.stderr)
    );
    println!(
        "LIVE_CROSS_ENV_VERDICT_ACCEPTED {}",
        json!({"attempt": identity.attempt, "changed": ["PATH", "RUSTFLAGS"],
            "stdout": String::from_utf8_lossy(&verdict.stdout)})
    );
    ok(root, &["sync"]);
    let landed = store.get(&id).unwrap();
    assert_eq!(landed.status, Status::Done);
    assert_eq!(
        git(&root.join("app"), &["rev-parse", "main"]),
        identity.head
    );
    let receipt = landed
        .artifacts
        .iter()
        .find(|a| a.title == "Acceptance record")
        .unwrap();
    println!(
        "LIVE_RECEIPTS {}",
        json!({"identity":identity, "audit":audit, "evidence":evidence,
        "gate":landed.current_run().unwrap().gate, "review":landed.current_run().unwrap().review,
        "acceptance":std::fs::read_to_string(store.dir().join(&receipt.path)).unwrap(),
        "events":store.events().unwrap()})
    );
}

#[test]
fn isolated_missing_live_evidence_refuses_done_and_escalates_review() {
    let Some((tmp, store, mut panes, id)) = live_fixture(Accept::Auto) else {
        return;
    };
    let root = tmp.path();
    // Legacy intake remains usable; enabling the manifest for this run requires evidence.
    checkpoint(root, &id);
    ok(root, &["done", &id, "--summary", SUMMARY]);
    manifest(root, &id);
    ok(root, &["gate", &id, "--wait"]);
    panes.ids.push(
        store
            .get(&id)
            .unwrap()
            .current_run()
            .unwrap()
            .gate
            .as_ref()
            .unwrap()
            .pane
            .clone()
            .unwrap(),
    );
    let out = cli(
        root,
        &["done", &id, "--check", "--json", "--summary", SUMMARY],
    );
    assert!(!out.status.success());
    let audit: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        audit["failures"]
            .as_array()
            .unwrap()
            .contains(&json!("evidence_missing:C1"))
    );
    ok(root, &["sync"]);
    let task = store.get(&id).unwrap();
    assert_eq!(task.status, Status::NeedsMe);
    assert_eq!(
        task.current_run()
            .unwrap()
            .autopilot
            .as_ref()
            .unwrap()
            .stage,
        AutoStage::Escalated
    );
    assert!(!root.join("app/widget.txt").exists());
    println!(
        "LIVE_MISSING_EVIDENCE {}",
        json!({"audit":audit, "task":task})
    );
}

#[test]
fn isolated_docs_failure_retains_its_own_command_receipt() {
    let Some((tmp, mut store, mut panes, id)) = live_fixture(Accept::Owner) else {
        return;
    };
    let task = store.get(&id).unwrap();
    let worktree = &task.current_run().unwrap().worktree;
    std::fs::create_dir_all(worktree.join("docs/guide")).unwrap();
    std::fs::create_dir(worktree.join("scripts")).unwrap();
    std::fs::write(
        worktree.join("scripts/check-docs.sh"),
        include_str!("../../../scripts/check-docs.sh"),
    )
    .unwrap();
    std::fs::write(worktree.join("README.md"), "`corral nonexistent-command`\n").unwrap();
    std::fs::write(worktree.join("docs/guide/fixture.md"), "Fixture.\n").unwrap();
    git(worktree, &["add", "-A"]);
    git(
        worktree,
        &["commit", "-q", "-m", "invalid documented command"],
    );
    let docs = format!(
        "bash scripts/check-docs.sh '{}'",
        env!("CARGO_BIN_EXE_corral")
    );
    store.config.review.gate = Some(vec!["true".into(), docs.clone()]);
    store.save_config().unwrap();
    let out = cli(tmp.path(), &["gate", &id, "--wait"]);
    assert!(!out.status.success());
    let task = store.get(&id).unwrap();
    let gate = task.current_run().unwrap().gate.as_ref().unwrap();
    panes.ids.push(gate.pane.clone().unwrap());
    assert_eq!(gate.failed_cmd.as_ref(), Some(&docs));
    assert_eq!(gate.commands[0].exit_code, 0);
    assert_ne!(gate.commands[1].exit_code, 0);
    println!(
        "LIVE_DOCS_REJECTION {}",
        serde_json::to_string(gate).unwrap()
    );
}
