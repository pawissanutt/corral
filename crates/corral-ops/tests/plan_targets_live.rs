#![cfg(unix)]
//! Real terminal dispatch and gates, always inside scripts/test-herdr.sh's private environment.
use std::path::Path;
use std::process::Stdio;

use corral_core::{Accept, AgentCmd, GateResult, Origin, Run, Status, Store};
use corral_ops::{ApproveStrategy, Ctx, DispatchOpts, DispatchOutcome};

#[path = "support/live.rs"]
mod live;
mod support;

fn git(repo: &Path, args: &[&str]) -> String {
    let output = support::command("git", repo)
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap().trim().into()
}

fn run(ctx: &Ctx, id: &str) -> Run {
    ctx.store.get(id).unwrap().current_run().unwrap().clone()
}

fn dispatch(ctx: &Ctx, id: &str) -> Run {
    assert!(matches!(
        corral_ops::dispatch(ctx, id, &DispatchOpts::default()).unwrap(),
        DispatchOutcome::Started { .. }
    ));
    let run = run(ctx, id);
    assert_eq!(run.execution.as_ref().unwrap().resolved.profile, run.agent);
    assert!(run.plan_target.is_some());
    run
}

fn submit(ctx: &Ctx, id: &str, file: &str) {
    let candidate = run(ctx, id);
    std::fs::write(candidate.worktree.join(file), format!("{id}\n")).unwrap();
    corral_git::commit_all(&candidate.worktree, id).unwrap();
    let identity = corral_ops::progress_readback(ctx, id).unwrap();
    corral_ops::save_checkpoint(
        ctx,
        id,
        &corral_core::CheckpointUpdate {
            task: identity.task,
            run: identity.run,
            spec_revision: identity.spec_revision,
            expected_revision: 0,
            progress_revision: 0,
            head: identity.head,
            base: identity.base,
            tree: identity.tree,
            criterion_ids: vec![],
            evidence_refs: vec![],
            next_action: "Review integrated candidate".into(),
            decisions: vec![],
            waits: vec![],
            resources: vec![],
        },
    )
    .unwrap();
    corral_ops::done(ctx, id, "Live Plan-target candidate").unwrap();
    corral_ops::gate_start(ctx, id).unwrap();
    assert_eq!(
        corral_ops::gate_wait(ctx, id).unwrap().result,
        GateResult::Pass
    );
}

#[test]
fn isolated_real_herdr_dispatches_gates_and_lands_serial_plan_candidates() {
    // Given a fresh plugin-free server supplied by scripts/test-herdr.sh.
    let private = std::env::var("XDG_CONFIG_HOME").is_ok_and(|path| {
        Path::new(&path)
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name.to_string_lossy().starts_with("corral-herdr-"))
    });
    if !private {
        eprintln!("SKIP: live Plan target check requires scripts/test-herdr.sh");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let herdr = live::herdr(dir.path());
    if herdr.workspace_list().is_err() {
        eprintln!("SKIP: isolated corral-test server unavailable");
        return;
    }
    let repo = dir.path().join("app");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-qb", "main"]);
    git(&repo, &["config", "user.name", "Fixture"]);
    git(&repo, &["config", "user.email", "fixture@example.invalid"]);
    git(&repo, &["config", "commit.gpgsign", "false"]);
    std::fs::write(repo.join("README"), "main bytes\n").unwrap();
    std::fs::write(repo.join(".gitignore"), ".worktrees/\n").unwrap();
    corral_git::commit_all(&repo, "initial").unwrap();
    let main = corral_git::rev_sha(&repo, "HEAD").unwrap();
    let mut store = Store::init(dir.path()).unwrap();
    store.config.herdr.session = Some("corral-test".into());
    store.config.agents.insert(
        "fixture".into(),
        AgentCmd {
            command: "true".into(),
        },
    );
    store.config.review.gate = Some(vec!["test -f README".into()]);
    store.config.review.reviewer = Some("none".into());
    store.config.review.accept = Some(Accept::Owner);
    store.config.review.remove_worktree = Some(true);
    store.save_config().unwrap();
    let ctx = Ctx {
        store,
        herdr,
        resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
    };
    let source = dir.path().join("plan.md");
    std::fs::write(&source, "---\ntitle: Live Plan\nproject: app\napproved_scope:\n  goal: Combine changes\n  acceptance: [{scenario: together, expected: pass}]\n  delivery_target: {kind: integration, branch: feature/live-plan}\ntasks:\n  - {key: a, title: A, profile: fixture}\n  - {key: b, title: B, profile: fixture}\n  - {key: c, title: C, profile: fixture, depends: [a]}\n---\n").unwrap();
    let plan = corral_ops::import_plan(&ctx, &source, false, Origin::Owner).unwrap();
    let a = &plan.tasks[0].id;
    let b = &plan.tasks[1].id;
    let c = &plan.tasks[2].id;
    let first = dispatch(&ctx, a);
    let second = dispatch(&ctx, b);
    assert_ne!(first.worktree, second.worktree);
    assert_eq!(first.base_sha, second.base_sha);
    assert_eq!(first.base_sha, main);
    assert!(corral_ops::dispatch(&ctx, c, &DispatchOpts::default()).is_err());
    assert!(ctx.store.get(c).unwrap().runs.is_empty());
    println!("LIVE: isolated tasks {a}/{b}, base={main}, dependency {c} refused before inclusion");
    submit(&ctx, a, "a.txt");
    submit(&ctx, b, "b.txt");
    let old_gate = run(&ctx, b).gate.unwrap();

    // When the first exact candidate lands, the sibling must re-gate on its new base.
    corral_ops::approve(&ctx, a, ApproveStrategy::FastForward, true, false).unwrap();
    let first_head = corral_git::rev_sha(&repo, "feature/live-plan").unwrap();
    corral_ops::approve(&ctx, b, ApproveStrategy::Rebase, true, false).unwrap();
    let new_gate = run(&ctx, b).gate.unwrap();
    assert_ne!(old_gate.sha, new_gate.sha);
    assert_eq!(
        new_gate.identity.as_ref().unwrap().integration_base,
        first_head
    );
    let integrated = corral_git::rev_sha(&repo, "feature/live-plan").unwrap();
    assert_eq!(new_gate.sha, integrated);
    let consumer = dispatch(&ctx, c);
    assert_eq!(consumer.base_sha, integrated);
    assert!(corral_git::is_ancestor(&repo, &first_head, &consumer.base_sha).unwrap());
    println!(
        "LIVE: serial landings {first_head} -> {integrated}; B gate {} -> {}; C base={}",
        old_gate.sha, new_gate.sha, consumer.base_sha
    );
    submit(&ctx, c, "c.txt");
    corral_ops::approve(&ctx, c, ApproveStrategy::FastForward, true, false).unwrap();

    // Then all task cleanup preserves the owned target and the project's checkout/ref/bytes.
    let checkout = &first.plan_target.as_ref().unwrap().checkout;
    assert!(checkout.is_dir());
    assert!(
        checkout.join("a.txt").is_file()
            && checkout.join("b.txt").is_file()
            && checkout.join("c.txt").is_file()
    );
    for id in [a, b, c] {
        assert_eq!(ctx.store.get(id).unwrap().status, Status::Done);
        assert!(!run(&ctx, id).worktree.exists());
        assert_eq!(git(&repo, &["branch", "--list", &run(&ctx, id).branch]), "");
    }
    assert_eq!(corral_git::current_branch(&repo).unwrap(), "main");
    assert_eq!(corral_git::rev_sha(&repo, "HEAD").unwrap(), main);
    assert_eq!(corral_git::rev_sha(&repo, "refs/heads/main").unwrap(), main);
    assert_eq!(std::fs::read(repo.join("README")).unwrap(), b"main bytes\n");
    assert!(
        !repo.join("a.txt").exists()
            && !repo.join("b.txt").exists()
            && !repo.join("c.txt").exists()
    );
    let approvals = ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|event| event.kind == "approved")
        .count();
    assert_eq!(approvals, 3);
    println!(
        "LIVE: 3 exact approval receipts; owned target retained; task worktrees/branches removed; main unchanged at {main}"
    );
}
