//! Real managed receipt attribution on isolated disposable Git worktrees, no live server.
use super::*;
use corral_ops::gate_triage::{self, Baseline, FailureClass, GateDiagnosis};

#[test]
fn red_base_is_proven_without_repair_charge_and_candidate_is_gated_again_after_base_repair() {
    let mut fx = Fx::new(Some("rev"));
    fx.ctx.store.config.review.gate = Some(vec![
        "test -f base-fixed || { printf 'known-base-error\n'; exit 1; }".into(),
    ]);
    fx.ctx.store.save_config().unwrap();
    let ctx = fx.ctx.clone();
    let repo = fx.app.clone();
    let base = git(&repo, &["rev-parse", "HEAD"]);
    // The baseline gate uses its own clean checkout and the same command/environment contract.
    let baseline_tree = repo.join(".worktrees/baseline");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-qb",
            "baseline",
            baseline_tree.to_str().unwrap(),
            "main",
        ],
    );
    let baseline = ctx
        .store
        .create("baseline diagnosis", Some("app"), Status::Review, "")
        .unwrap();
    let run: Run = serde_json::from_value(json!({"id":"R1","agent":"fake","worktree":baseline_tree,
        "branch":"baseline","base":"main","base_sha":base,"started":baseline.created,"outcome":"done"})).unwrap();
    ctx.store
        .update(&baseline.id, |task| {
            task.kind = Some("backend".into());
            task.size = Some(corral_core::TaskSize::M);
            task.runs.push(run);
            Ok(())
        })
        .unwrap();
    corral_ops::gate_start(&ctx, &baseline.id).unwrap();
    let red_base = corral_ops::gate_wait(&ctx, &baseline.id).unwrap();
    assert_eq!(red_base.result, GateResult::Fail);
    let scratch = scratch::create(&ctx, "A-1", "app", Some("candidate"), None).unwrap();
    std::fs::write(scratch.path.join("candidate"), "candidate work\n").unwrap();
    git(&scratch.path, &["add", "."]);
    git(&scratch.path, &["commit", "-qm", "candidate"]);
    let submitted = scratch::finish(
        &ctx,
        "A-1",
        "candidate",
        &Finish {
            title: Some("candidate"),
            ..Default::default()
        },
    )
    .unwrap();
    corral_ops::sync(&ctx).unwrap();
    let task = ctx.store.get(&submitted.task).unwrap();
    assert_eq!((task.status, task.rounds), (Status::NeedsMe, 0));
    let failed = task.current_run().unwrap().gate.clone().unwrap();
    assert_eq!(failed.result, GateResult::Fail);
    let proof = ctx.store.root().join("diagnosis.md");
    std::fs::write(
        &proof,
        "Exact baseline and candidate failed command output retained in managed receipts.",
    )
    .unwrap();
    let artifact = corral_ops::add_artifact(
        &ctx,
        &task.id,
        &proof,
        Some(corral_core::ArtifactKind::Report),
        Some("baseline diagnosis"),
    )
    .unwrap();
    let report = GateDiagnosis {
        attempt: failed.identity.as_ref().unwrap().attempt.clone(),
        class: FailureClass::RedBase,
        explanation: "Same relevant failure on isolated clean base".into(),
        evidence: vec![artifact.path],
        baseline: Some(Baseline {
            task: baseline.id,
            run: "R1".into(),
            attempt: red_base.identity.unwrap().attempt,
        }),
        retry: false,
    };
    gate_triage::diagnose(&ctx, &task.id, report.clone(), true).unwrap();
    corral_ops::sync(&ctx).unwrap();
    assert_eq!(ctx.store.get(&task.id).unwrap().rounds, 0);
    std::fs::write(repo.join("base-fixed"), "base repaired\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "repair base"]);
    gate_triage::diagnose(
        &ctx,
        &task.id,
        GateDiagnosis {
            retry: true,
            ..report
        },
        true,
    )
    .unwrap();
    corral_ops::sync(&ctx).unwrap();
    let checked = ctx.store.get(&task.id).unwrap();
    let gate = checked.current_run().unwrap().gate.as_ref().unwrap();
    assert_eq!((checked.rounds, gate.result), (0, GateResult::Pass));
    assert_ne!(
        gate.identity.as_ref().unwrap().attempt,
        failed.identity.as_ref().unwrap().attempt
    );
    assert_eq!(
        gate_triage::status(&ctx, &task.id).unwrap().failures[0].gate,
        failed
    );
    corral_ops::verdict(
        &ctx,
        &task.id,
        Verdict::Approve,
        "independent review of repaired-base candidate",
    )
    .unwrap();
    corral_ops::sync(&ctx).unwrap();
    assert_eq!(ctx.store.get(&task.id).unwrap().status, Status::Done);
    assert!(repo.join("candidate").is_file());
}
