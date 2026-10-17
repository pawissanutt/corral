//! Real terminal lifecycle smoke check, confined to the isolated corral-test engine.
#![cfg(unix)] // The isolated live-client wrapper is an executable POSIX script.

use std::path::Path;
#[path = "support/live.rs"]
mod live;
mod support;

use corral_core::{
    Accept, AgentCmd, CheckpointUpdate, GateRecord, GateResult, ReviewRecord, Status, Store,
};
use corral_ops::{ApproveStrategy, Ctx, DispatchOpts, DispatchOutcome};

fn git(path: &Path, args: &[&str]) {
    let output = support::command("git", path)
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
    let tmp = tempfile::tempdir().unwrap();
    let herdr = live::herdr(tmp.path());
    if !herdr.available() {
        println!("SKIP: herdr session corral-test unavailable");
        return;
    }
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
    store.config.review.gate = Some(vec!["true".into()]);
    store.save_config().unwrap();
    let ctx = Ctx {
        resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
        store,
        herdr,
    };
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
                    socket: Some(ctx.herdr.socket_path().unwrap()),
                    identity: None,
                    commands: Vec::new(),
                    log_sha256: None,
                    result: GateResult::Pass,
                    process_lost: false,
                    failed_cmd: None,
                    at: r.started,
                    sha: r.base_sha.clone(),
                    pane: Some(gate.pane_id.clone()),
                });
                r.review = Some(ReviewRecord {
                    identity: None,
                    evidence_digest: None,
                    verdict: Some(corral_core::Verdict::Approve),
                    process_lost: false,
                    reviewer: "cleanup-fake".into(),
                    notes: None,
                    sha: r.base_sha.clone(),
                    at: r.started,
                    pane: Some(review.pane_id.clone()),
                });
                let mut child = r.clone();
                child.id = format!("{}-review1", r.id);
                child.herdr = None;
                child.gate = None;
                child.ended = Some(r.started);
                child.outcome = Some(corral_core::RunOutcome::Done);
                child.agent_state = Some("done".into());
                child.review_runs.clear();
                r.review_runs.push(child);
                Ok(())
            })
            .unwrap();
        let state = corral_ops::progress_readback(&ctx, &task.id).unwrap();
        corral_ops::save_checkpoint(
            &ctx,
            &task.id,
            &CheckpointUpdate {
                task: task.id.clone(),
                run: state.run,
                spec_revision: state.spec_revision,
                expected_revision: 0,
                progress_revision: 0,
                head: state.head,
                base: state.base,
                tree: state.tree,
                criterion_ids: Vec::new(),
                evidence_refs: Vec::new(),
                next_action: "Complete the isolated cleanup lifecycle".into(),
                decisions: Vec::new(),
                waits: Vec::new(),
                resources: Vec::new(),
            },
        )
        .unwrap();
        corral_ops::done(&ctx, &task.id, "isolated lifecycle smoke").unwrap();
        if action == "approve" {
            ctx.store
                .update(&task.id, |t| {
                    let r = t.current_run_mut().unwrap();
                    r.ended = None;
                    r.outcome = None;
                    r.agent_state = Some("working".into());
                    Ok(())
                })
                .unwrap();
        }
        let (tx, messages) = std::sync::mpsc::channel();
        let _capture = corral_ops::capture_messages(tx);

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
        if action == "approve" {
            let landed = ctx.store.get(&task.id).unwrap();
            let finished = landed.current_run().unwrap();
            assert_eq!(landed.status, Status::Done);
            assert!(finished.ended.is_some());
            assert_eq!(finished.outcome, Some(corral_core::RunOutcome::Done));
            assert_eq!(finished.agent_state.as_deref(), Some("done"));
            println!("PASS approve: Done persisted with ended/outcome=done/agent_state=done");
        }
        let sidecar = ctx
            .store
            .state_dir()
            .join("review-watch")
            .join(&task.id)
            .join(format!("{}-review1.json", run.id));
        let captured = std::fs::read(&sidecar).unwrap();
        corral_ops::sync(&ctx).unwrap();
        corral_ops::sync(&ctx).unwrap();
        assert_eq!(std::fs::read(&sidecar).unwrap(), captured);
        assert!(
            !messages
                .try_iter()
                .any(|m| m.text.contains("pane_not_found"))
        );
        println!("PASS {action}: repeated sync retained final capture without pane_not_found");

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
