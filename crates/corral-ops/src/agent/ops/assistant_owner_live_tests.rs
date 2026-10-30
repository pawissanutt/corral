//! Setup for the private-server owner-authority host scenario (T-544 C3 to C6): two tasks in
//! Review whose branches carry one real commit, gated and approved; a Ready task to dispatch; a
//! Backlog task to drop; and assistant A-1 opened in the test's private pane.
use super::*;
use corral_core::{
    Accept, AgentCmd, CheckpointUpdate, GateRecord, GateResult, ReviewRecord, Status, Store,
};
use serde_json::{Value, json};
use std::path::Path;

fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// A task dispatched with the fake agent, one commit on its branch, gated, approved and done.
fn reviewable(ctx: &Ctx, title: &str) -> String {
    let task = ctx
        .store
        .create(title, Some("corral"), Status::Ready, "")
        .unwrap();
    let opts = crate::DispatchOpts {
        profile: Some("owner-fake".into()),
        ..crate::DispatchOpts::default()
    };
    let crate::DispatchOutcome::Started { task, .. } =
        crate::dispatch(ctx, &task.id, &opts).unwrap()
    else {
        panic!("{title}: expected a dispatched run");
    };
    let worktree = task.current_run().unwrap().worktree.clone();
    std::fs::write(worktree.join(format!("{}.txt", task.id)), "owner landing\n").unwrap();
    git(&worktree, &["add", "."]);
    git(&worktree, &["commit", "-q", "-m", title]);
    let head = git(&worktree, &["rev-parse", "HEAD"]);
    ctx.store
        .update(&task.id, |t| {
            let r = t.current_run_mut().unwrap();
            r.gate = Some(GateRecord {
                socket: None,
                identity: None,
                commands: Vec::new(),
                log_sha256: None,
                result: GateResult::Pass,
                process_lost: false,
                failed_cmd: None,
                at: r.started,
                sha: head.clone(),
                pane: None,
                tier: None,
            });
            r.review = Some(ReviewRecord {
                route: None,
                identity: None,
                evidence_digest: None,
                verdict: Some(corral_core::Verdict::Approve),
                process_lost: false,
                reviewer: "owner-fake".into(),
                notes: None,
                sha: head.clone(),
                at: r.started,
                pane: None,
            });
            Ok(())
        })
        .unwrap();
    let state = crate::progress_readback(ctx, &task.id).unwrap();
    crate::save_checkpoint(
        ctx,
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
            next_action: "Await the owner's approval".into(),
            decisions: Vec::new(),
            waits: Vec::new(),
            resources: Vec::new(),
        },
    )
    .unwrap();
    crate::done(ctx, &task.id, "owner-authority live fixture").unwrap();
    assert_eq!(ctx.store.get(&task.id).unwrap().status, Status::Review);
    task.id
}

#[test]
#[ignore = "driven by the private-server owner-authority live test"]
fn owner_authority_live_setup() {
    let Ok(input) = std::env::var("CORRAL_OWNER_LIVE") else {
        return;
    };
    let input: Value = serde_json::from_str(&input).unwrap();
    assert_eq!(
        std::env::var("CORRAL_HERDR_SESSION").unwrap(),
        "corral-test"
    );
    let root = Path::new(input["root"].as_str().unwrap());
    let mut store = Store::init(root).unwrap();
    store.config.agent.assets_dir = Some(input["assets"].as_str().unwrap().into());
    store.config.assistant.agent = Some("owner".into());
    store.config.profiles.insert(
        "owner".into(),
        serde_json::from_value(
            json!({"harness":"pi-native", "model":"gpt-6.1-sol", "vendor":"openai"}),
        )
        .unwrap(),
    );
    store.config.agents.insert(
        "owner-fake".into(),
        AgentCmd {
            command: "true".into(),
        },
    );
    store.config.review.accept = Some(Accept::Owner);
    store.config.review.gate = Some(vec!["true".into()]);
    store.config.resources.max_load_per_core = 1.0e6;
    store.config.projects.insert(
        "corral".into(),
        serde_json::from_value(json!({"path":"app"})).unwrap(),
    );
    store.save_config().unwrap();
    let ctx = Ctx::from_store(Store::discover(root).unwrap());
    let land = reviewable(&ctx, "Land the owner's change");
    let held = reviewable(&ctx, "Approval while autopilot holds it");
    let ready = ctx
        .store
        .create(
            "Dispatch on the owner's do it",
            Some("corral"),
            Status::Ready,
            "",
        )
        .unwrap();
    let drop = ctx
        .store
        .create(
            "Drop on the owner's confirmation",
            Some("corral"),
            Status::Backlog,
            "",
        )
        .unwrap();
    let (session, prepared, _) = open(
        &ctx,
        input["pane"].as_str().unwrap(),
        &Overrides::default(),
        false,
    )
    .unwrap();
    let bootstrap: crate::agent::launch::Bootstrap =
        serde_json::from_slice(&std::fs::read(&prepared.bootstrap).unwrap()).unwrap();
    let ready_launch = crate::agent::launch::check(
        &bootstrap,
        prepared.bootstrap.parent().unwrap(),
        prepared.assets.as_deref(),
    )
    .unwrap();
    let secrets = crate::agent::launch::Secrets {
        host_cap: "e".repeat(64),
        ctl_cap: "d".repeat(64),
    };
    let launch = root.join("owner-launch.json");
    std::fs::write(
        &launch,
        crate::agent::launch::launch_message(&bootstrap, &ready_launch, &secrets).unwrap(),
    )
    .unwrap();
    println!(
        "OWNER_LIVE {}",
        json!({"land": land, "held": held, "dispatch": ready.id, "drop": drop.id,
            "session": session.id, "bootstrap": prepared.bootstrap, "launch": launch})
    );
}
