//! Integration preflight through the real CLI and isolated herdr transport.
use std::path::Path;
use std::process::{Command, Output, Stdio};

use corral_core::{AgentCmd, AutoStage, Status, Store};
use corral_herdr::Herdr;
use serde_json::{Value, json};

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn cli(root: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_corral"));
    for (name, _) in std::env::vars_os() {
        let text = name.to_string_lossy();
        if text.starts_with("CORRAL_") || text.starts_with("HERDR_") {
            cmd.env_remove(name);
        }
    }
    let out = cmd
        .env("CORRAL_HERDR_SESSION", "corral-test")
        .arg("--root")
        .arg(root)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {out:?}");
    out
}

struct Workspace {
    herdr: Herdr,
    id: String,
}

impl Drop for Workspace {
    fn drop(&mut self) {
        if let Err(error) = self.herdr.workspace_close(&self.id) {
            eprintln!("isolated preflight cleanup: {error:#}");
        }
    }
}

#[test]
fn isolated_cli_conflict_escalates_before_gate_or_reviewer_and_keeps_exact_packet() {
    // Given a real isolated CLI run and a conflicting main with two independent paths.
    let herdr = Herdr::with_bin("herdr".into(), Some("corral-test".into()));
    if !herdr.available() {
        println!("SKIP: herdr session corral-test unavailable");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let app = tmp.path().join("app");
    std::fs::create_dir(&app).unwrap();
    git(&app, &["init", "-q", "-b", "main"]);
    git(&app, &["config", "user.name", "corral preflight fixture"]);
    git(&app, &["config", "user.email", "corral@test.invalid"]);
    git(&app, &["config", "commit.gpgsign", "false"]);
    for path in ["first.rs", "second.rs"] {
        std::fs::write(app.join(path), "fn original() {}\n").unwrap();
    }
    git(&app, &["add", "-A"]);
    git(&app, &["commit", "-q", "-m", "initial"]);
    let dispatch = git(&app, &["rev-parse", "HEAD"]);
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.herdr.session = Some("corral-test".into());
    for name in ["preflight-worker", "preflight-reviewer"] {
        store.config.agents.insert(
            name.into(),
            AgentCmd {
                command: "true".into(),
            },
        );
    }
    store.config.review.gate = Some(vec!["false".into()]);
    store.config.review.reviewer = Some("preflight-reviewer".into());
    store.config.review.max_rounds = Some(0);
    store.save_config().unwrap();
    let id = String::from_utf8(
        cli(
            tmp.path(),
            &["new", "T054 preflight", "-p", "app", "--ready"],
        )
        .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    cli(
        tmp.path(),
        &["dispatch", &id, "--profile", "preflight-worker"],
    );
    let task = store.get(&id).unwrap();
    let run = task.current_run().unwrap().clone();
    let _workspace = Workspace {
        herdr,
        id: run.herdr.as_ref().unwrap().workspace_id.clone(),
    };
    for path in ["first.rs", "second.rs"] {
        std::fs::write(run.worktree.join(path), "fn candidate() {}\n").unwrap();
    }
    git(&run.worktree, &["add", "-A"]);
    git(&run.worktree, &["commit", "-q", "-m", "candidate"]);
    let head = git(&run.worktree, &["rev-parse", "HEAD"]);
    let index = git(&run.worktree, &["write-tree"]);
    for path in ["first.rs", "second.rs"] {
        std::fs::write(app.join(path), "fn landed() {}\n").unwrap();
    }
    git(&app, &["add", "--", "first.rs", "second.rs"]);
    git(
        &app,
        &["commit", "-q", "-m", "T-999 misleading unapproved subject"],
    );
    let base = git(&app, &["rev-parse", "HEAD"]);
    let checkpoint = Command::new("bash")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../scripts/checkpoint-fixture.sh"
        ))
        .arg(env!("CARGO_BIN_EXE_corral"))
        .arg(&id)
        .env("CORRAL_ROOT", tmp.path())
        .env_remove("HERDR_BIN_PATH")
        .env("CORRAL_HERDR_SESSION", "corral-test")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        checkpoint.status.success(),
        "{}",
        String::from_utf8_lossy(&checkpoint.stderr)
    );
    cli(
        tmp.path(),
        &[
            "done",
            &id,
            "--summary",
            "Verified: fixture commits\nNot verified: none",
        ],
    );

    // When the real CLI sync observes a conflict with no authorized repair rounds.
    cli(tmp.path(), &["sync"]);
    cli(tmp.path(), &["sync"]);

    // Then the owner gets exact context without gate/reviewer spend or checkout mutation.
    let task = store.get(&id).unwrap();
    let run = task.current_run().unwrap();
    assert_eq!(task.status, Status::NeedsMe);
    assert_eq!(run.autopilot.as_ref().unwrap().stage, AutoStage::Escalated);
    assert!(run.gate.is_none());
    assert!(run.review.is_none());
    assert_eq!(git(&run.worktree, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&run.worktree, &["write-tree"]), index);
    assert_eq!(git(&run.worktree, &["status", "--porcelain"]), "");
    let artifacts: Vec<_> = task
        .artifacts
        .iter()
        .filter(|a| a.title == "Integration repair packet")
        .collect();
    assert_eq!(artifacts.len(), 1, "{:?}", run.autopilot);
    let packet: Value =
        serde_json::from_slice(&std::fs::read(store.dir().join(&artifacts[0].path)).unwrap())
            .unwrap();
    assert_eq!(
        packet["key"],
        json!({
            "task": id, "run": run.id, "head": head, "base": base, "reason": "merge_conflict"
        })
    );
    assert_eq!(packet["dispatch_base"], dispatch);
    assert_eq!(packet["merge_base"], dispatch);
    assert_eq!(packet["conflict_paths"], json!(["first.rs", "second.rs"]));
    assert_eq!(packet["missing_from_candidate"][0]["sha"], base);
    assert_eq!(packet["missing_from_candidate"][0]["tasks"], json!([]));
    assert_eq!(packet["gate_commands"], json!(["false"]));
    let events = store.events().unwrap();
    for kind in ["gate_started", "review_started", "changes_requested"] {
        assert!(!events.iter().any(|e| e.kind == kind));
    }
    println!("T054 isolated CLI: new, dispatch, done, sync twice, conflict escalation PASS");
    println!(
        "T054 exact packet: {}",
        serde_json::to_string(&packet).unwrap()
    );
    println!("T054 branch, index, worktree unchanged; gate/reviewer not launched PASS");
}
