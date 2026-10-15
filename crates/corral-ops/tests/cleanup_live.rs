//! Real terminal lifecycle smoke check, confined to the isolated corral-test engine.

use std::path::Path;
use std::process::Command;

use corral_core::{Accept, AgentCmd, GateRecord, GateResult, ReviewRecord, Status, Store};
use corral_herdr::Herdr;
use corral_ops::{ApproveStrategy, Ctx, DispatchOpts, DispatchOutcome};

fn git(path: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn isolated_terminal_cleanup_lifecycle() {
    // Given: only disposable resources created by this test in the isolated engine.
    let herdr = Herdr::new(Some("corral-test".into()));
    if !herdr.available() {
        println!("SKIP: herdr session corral-test unavailable");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let app = tmp.path().join("app");
    std::fs::create_dir(&app).unwrap();
    git(&app, &["init", "-q", "-b", "main"]);
    git(&app, &["config", "user.name", "corral test"]);
    git(&app, &["config", "user.email", "corral@test.invalid"]);
    git(&app, &["commit", "-q", "--allow-empty", "-m", "init"]);
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.agents.insert(
        "cleanup-fake".into(),
        AgentCmd {
            command: "true".into(),
        },
    );
    store.config.review.accept = Some(Accept::Owner);
    store.save_config().unwrap();
    let ctx = Ctx { store, herdr };
    for action in ["approve", "discard", "sync"] {
        let task = ctx
            .store
            .create("T-066 isolated cleanup", Some("app"), Status::Ready, "")
            .unwrap();
        let DispatchOutcome::Started { task, .. } = corral_ops::dispatch(
            &ctx,
            &task.id,
            &DispatchOpts {
                profile: Some("cleanup-fake".into()),
                ..DispatchOpts::default()
            },
        )
        .unwrap() else {
            panic!("expected a dispatched run");
        };
        let run = task.current_run().unwrap();
        let h = run.herdr.as_ref().unwrap();
        let gate = ctx
            .herdr
            .tab_create(&h.workspace_id, &run.worktree, "gate", false)
            .unwrap();
        let review = ctx
            .herdr
            .tab_create(&h.workspace_id, &run.worktree, "review", false)
            .unwrap();
        ctx.store
            .update(&task.id, |t| {
                let r = t.current_run_mut().unwrap();
                r.gate = Some(GateRecord {
                    identity: None,
                    commands: Vec::new(),
                    log_sha256: None,
                    result: GateResult::Pass,
                    failed_cmd: None,
                    at: r.started,
                    sha: r.base_sha.clone(),
                    pane: Some(gate.pane_id.clone()),
                });
                r.review = Some(ReviewRecord {
                    identity: None,
                    evidence_digest: None,
                    verdict: Some(corral_core::Verdict::Approve),
                    reviewer: "cleanup-fake".into(),
                    notes: None,
                    sha: r.base_sha.clone(),
                    at: r.started,
                    pane: Some(review.pane_id.clone()),
                });
                Ok(())
            })
            .unwrap();
        corral_ops::done(&ctx, &task.id, "isolated lifecycle smoke").unwrap();

        // When: the real operations reach a terminal status.
        match action {
            "approve" => {
                corral_ops::approve(&ctx, &task.id, ApproveStrategy::FastForward, true, false)
                    .unwrap();
            }
            "discard" => {
                corral_ops::discard(&ctx, &task.id).unwrap();
            }
            "sync" => {
                ctx.store
                    .update(&task.id, |t| {
                        t.status = Status::Done;
                        Ok(())
                    })
                    .unwrap();
                corral_ops::sync(&ctx).unwrap();
            }
            _ => unreachable!(),
        }

        // Then: both tool tabs, the agent, and their workspace are gone.
        for pane in [&h.pane_id, &gate.pane_id, &review.pane_id] {
            assert!(
                ctx.herdr.pane_get(pane).unwrap().is_none(),
                "{action}: {pane}"
            );
        }
        assert!(
            !ctx.herdr
                .workspace_list()
                .unwrap()
                .iter()
                .any(|(id, _)| id == &h.workspace_id),
            "{action}: workspace {}",
            h.workspace_id
        );
        println!(
            "PASS {action}: closed agent {}, gate {}, review {}, workspace {} (corral-test)",
            h.pane_id, gate.pane_id, review.pane_id, h.workspace_id
        );
    }
}
