use super::*;
use corral_core::{AgentCmd, AutoStage, HerdrRef, Status};

#[test]
fn stale_review_rework_and_peer_landing_through_cli() {
    // Given a real git candidate and the hermetic herdr executable, never the user's engine.
    let ws = init_ws();
    let (mut store, worktree, _) = review_task(&ws, None);
    store.config.agents.insert(
        "rev".into(),
        AgentCmd {
            command: "true".into(),
        },
    );
    store.config.review.gate = Some(vec!["true".into()]);
    store.config.review.reviewer = Some("rev".into());
    store.save_config().unwrap();
    store
        .update("T-001", |task| {
            task.status = Status::Running;
            task.current_run_mut().unwrap().herdr = Some(HerdrRef {
                workspace_id: "w1".into(),
                pane_id: "w1:p1".into(),
            });
            Ok(())
        })
        .unwrap();
    let bin = ws.root.join("herdr");
    std::os::unix::fs::symlink(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../corral-ops/tests/autopilot_ops/herdr.sh"),
        &bin,
    )
    .unwrap();
    let invoke = |args: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
        for (name, _) in std::env::vars_os() {
            let text = name.to_string_lossy();
            if text.starts_with("CORRAL_") || text.starts_with("HERDR_") {
                command.env_remove(name);
            }
        }
        let out = command
            .env("HERDR_BIN_PATH", &bin)
            .env("CORRAL_HERDR_SESSION", "corral-test")
            .arg("--root")
            .arg(&ws.root)
            .args(args)
            .current_dir(&ws.root)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        println!(
            "CLI {args:?}: exit={}; {}{}",
            out.status,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        out
    };
    assert!(
        invoke(&["done", "T-001", "--summary", "First candidate"])
            .status
            .success()
    );
    assert!(invoke(&["sync"]).status.success());
    let first = store.get("T-001").unwrap();
    let old = first.current_run().unwrap().review.as_ref().unwrap();
    std::fs::write(worktree.join("feature.txt"), "moved during review\n").unwrap();
    git(&worktree, &["add", "-A"]);
    git(&worktree, &["commit", "-q", "-m", "Move candidate"]);
    let refused = invoke(&["verdict", "T-001", "approve", "--notes", "Reviewed"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("stale review"));
    std::fs::write(ws.root.join("gone"), "w1:p2\n").unwrap();

    // When the owner requests changes and the worker submits another committed candidate.
    assert!(invoke(&["changes", "T-001", "Rework it"]).status.success());
    let sent_back = store.get("T-001").unwrap();
    assert_eq!(sent_back.status, Status::Running);
    assert!(sent_back.current_run().unwrap().review.is_none());
    std::fs::write(worktree.join("feature.txt"), "reworked\n").unwrap();
    assert!(
        invoke(&["done", "T-001", "--summary", "Reworked candidate"])
            .status
            .success()
    );
    std::fs::remove_file(ws.root.join("gone")).unwrap();
    assert!(invoke(&["sync"]).status.success());

    // Then the CLI starts the next gate and review without Needs me.
    let task = store.get("T-001").unwrap();
    assert_eq!(task.status, Status::Review);
    let run = task.current_run().unwrap();
    assert_eq!(run.autopilot.as_ref().unwrap().stage, AutoStage::Reviewing);
    let review = run.review.as_ref().unwrap();
    assert_ne!(review.sha, old.sha);
    assert_eq!(review.identity, run.gate.as_ref().unwrap().identity);
    assert_eq!(run.review_runs.len(), 2);
    assert!(
        store
            .events()
            .unwrap()
            .iter()
            .all(|e| e.kind != "review_escalated")
    );

    // A legacy Running peer retains an old pending review; it must not block this landing.
    let peer = store
        .create("Reworking peer", Some("app"), Status::Running, "")
        .unwrap();
    store
        .update(&peer.id, |task| {
            let mut legacy = run.clone();
            legacy.review = Some(old.clone());
            legacy.outcome = None;
            task.runs.push(legacy);
            Ok(())
        })
        .unwrap();
    assert!(
        invoke(&["verdict", "T-001", "approve", "--notes", "Rework reviewed"])
            .status
            .success()
    );
    assert!(
        invoke(&["approve", "T-001", "--keep-worktree", "--keep-branch"])
            .status
            .success()
    );
    assert_eq!(store.get("T-001").unwrap().status, Status::Done);
    assert_eq!(store.get(&peer.id).unwrap().status, Status::Running);
}
