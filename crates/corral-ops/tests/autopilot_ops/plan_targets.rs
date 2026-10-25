use super::*;
use serde_json::Value;

const TARGET: &str = "feature/plan";

pub(super) fn plan() -> (Fx, Task, [String; 3]) {
    let mut fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.reviewer = Some("rev".into());
        r.accept = Some(Accept::Owner);
        r.remove_worktree = Some(true);
    });
    // Release Fx's initial executor without syncing or admitting unrelated work.
    fx.done();
    let source = fx.ctx.store.root().join("plan.md");
    std::fs::write(
        &source,
        r#"---
title: Integration Plan
project: app
approved_scope:
  goal: Ship A and B, then C
  acceptance: [{scenario: Combine changes, expected: All included}]
  delivery_target: {kind: integration, branch: feature/plan}
tasks:
  - {key: a, title: A, profile: fake}
  - {key: b, title: B, profile: fake}
  - {key: c, title: C, profile: fake, depends: [a]}
---
"#,
    )
    .unwrap();
    let imported =
        corral_ops::import_plan(&fx.ctx, &source, false, corral_core::Origin::Owner).unwrap();
    let ids = std::array::from_fn(|i| imported.tasks[i].id.clone());
    fx.grant(std::slice::from_ref(&imported.tracker.id));
    assert_eq!(imported.tasks[2].depends_on, [ids[0].clone()]);
    assert_eq!(
        imported
            .tracker
            .approved_scope
            .as_ref()
            .unwrap()
            .delivery_target,
        corral_core::PlanDeliveryTarget::Integration {
            branch: TARGET.into()
        }
    );
    (fx, imported.tracker, ids)
}

pub(super) fn dispatch(fx: &mut Fx, id: &str) -> Run {
    fx.id = id.into();
    assert!(matches!(
        corral_ops::dispatch(&fx.ctx, id, &DispatchOpts::default()).unwrap(),
        DispatchOutcome::Started { .. }
    ));
    let run = fx.run();
    assert_eq!(run.harness.as_deref(), Some("fake"));
    run
}

pub(super) fn reviewed(fx: &mut Fx, id: &str, file: &str, text: &str) -> Run {
    fx.id = id.into();
    fx.change(file, text);
    fx.done();
    corral_ops::gate_start(&fx.ctx, id).unwrap();
    assert_eq!(
        corral_ops::gate_wait(&fx.ctx, id).unwrap().result,
        GateResult::Pass
    );
    corral_ops::review_run(&fx.ctx, id, None).unwrap();
    fx.verdict(Verdict::Approve, "Candidate approved.");
    let run = fx.run();
    assert_eq!(
        run.gate.as_ref().unwrap().identity,
        run.review.as_ref().unwrap().identity
    );
    run
}

pub(super) fn events(fx: &Fx, id: &str, kind: &str) -> Vec<Value> {
    fx.ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.task.as_deref() == Some(id) && e.kind == kind)
        .map(|e| e.detail)
        .collect()
}

fn prerequisite(fx: &Fx, plan: &Task, child: &str) -> corral_ops::PlanPrerequisite {
    corral_ops::plan_progress(&fx.ctx, &plan.id)
        .unwrap()
        .work
        .into_iter()
        .find(|work| work.task == child)
        .unwrap()
        .prerequisites
        .into_iter()
        .next()
        .unwrap()
}

fn assert_main(fx: &Fx, sha: &str) {
    assert_eq!(git(&fx.app, &["branch", "--show-current"]), "main");
    assert_eq!(git(&fx.app, &["rev-parse", "HEAD"]), sha);
    assert_eq!(git(&fx.app, &["rev-parse", "refs/heads/main"]), sha);
    assert_eq!(
        std::fs::read_to_string(fx.app.join("README.md")).unwrap(),
        "hello\n"
    );
    assert_eq!(git(&fx.app, &["diff", "--name-only", "HEAD"]), "");
    for file in ["a.txt", "b.txt", "c.txt", "shared.txt"] {
        assert!(!fx.app.join(file).exists());
    }
}

fn assert_cleaned(fx: &Fx, run: &Run, target_head: &str) {
    let target = run.plan_target.as_ref().unwrap();
    assert!(!run.worktree.exists());
    assert_eq!(git(&fx.app, &["branch", "--list", &run.branch]), "");
    assert!(target.checkout.is_dir());
    assert_eq!(git(&target.checkout, &["branch", "--show-current"]), TARGET);
    assert_eq!(git(&target.checkout, &["rev-parse", "HEAD"]), target_head);
    assert_eq!(
        git(&fx.app, &["rev-parse", "refs/heads/feature/plan"]),
        target_head
    );
    assert_eq!(git(&target.checkout, &["status", "--porcelain"]), "");
}

#[test]
fn independent_candidates_rebase_into_owned_plan_before_dependent_dispatch() {
    // Given an approved integration Plan, A and B start before either lands.
    let (mut fx, plan, [a, b, c]) = plan();
    let main = git(&fx.app, &["rev-parse", "main"]);
    let a_run = dispatch(&mut fx, &a);
    let b_run = dispatch(&mut fx, &b);
    assert_eq!(a_run.base, TARGET);
    assert_eq!(b_run.base, TARGET);
    assert_eq!(a_run.base_sha, main);
    assert_eq!(a_run.base_sha, b_run.base_sha);
    assert_ne!(a_run.worktree, b_run.worktree);
    assert_ne!(a_run.branch, b_run.branch);
    let binding = a_run.plan_target.as_ref().unwrap();
    assert_eq!(Some(binding), b_run.plan_target.as_ref());
    assert_eq!(binding.plan, plan.id);
    assert_eq!(binding.scope_revision, plan.approved_scope_revision);
    assert_eq!(
        binding.contract_revision,
        corral_core::operating_contract_revision(&plan).unwrap()
    );
    assert_eq!(binding.branch, TARGET);
    assert_ne!(binding.checkout, a_run.worktree);
    assert_ne!(binding.checkout, b_run.worktree);
    assert_main(&fx, &main);
    reviewed(&mut fx, &a, "a.txt", "A\n");
    let old_b = reviewed(&mut fx, &b, "b.txt", "B\n");
    let old_gate = old_b.gate.unwrap().identity.unwrap();
    let old_review = old_b.review.unwrap();

    // When A lands and one Rebase approval integrates B against A's new target.
    corral_ops::approve(&fx.ctx, &a, ApproveStrategy::FastForward, true, false).unwrap();
    let a_head = git(&fx.app, &["rev-parse", TARGET]);
    assert_cleaned(&fx, &a_run, &a_head);
    assert_eq!(events(&fx, &a, "gate_started").len(), 1);
    corral_ops::approve(&fx.ctx, &b, ApproveStrategy::Rebase, true, false).unwrap();

    // Then B has a new gate identity and only its equal patch carries review.
    fx.id = b.clone();
    let landed = fx.run();
    let gate = landed.gate.unwrap();
    let identity = gate.identity.unwrap();
    let review = landed.review.unwrap();
    let head = git(&fx.app, &["rev-parse", TARGET]);
    assert_eq!(fx.task().status, Status::Done);
    assert_eq!(gate.result, GateResult::Pass);
    assert_eq!(identity.head, head);
    assert_eq!(identity.integration_base, a_head);
    assert_ne!(identity.head, old_gate.head);
    assert_ne!(identity.attempt, old_gate.attempt);
    assert_eq!(review.identity.as_ref(), Some(&identity));
    assert_eq!(review.verdict, Some(Verdict::Approve));
    assert_eq!(review.reviewer, old_review.reviewer);
    assert_eq!(review.notes, old_review.notes);
    assert_eq!(events(&fx, &b, "gate_started").len(), 2);
    assert_eq!(events(&fx, &b, "review_started").len(), 1);
    let carries = events(&fx, &b, "verdict_carried");
    assert_eq!(carries.len(), 1);
    assert_eq!(carries[0]["old_sha"], old_gate.head);
    assert_eq!(carries[0]["new_sha"], head);
    assert_eq!(
        carries[0]["evidence"]["old_patch_sha256"],
        carries[0]["evidence"]["new_patch_sha256"]
    );
    assert_cleaned(&fx, &b_run, &head);
    for (file, content) in [("a.txt", "A\n"), ("b.txt", "B\n")] {
        assert_eq!(
            std::fs::read_to_string(binding.checkout.join(file)).unwrap(),
            content
        );
    }
    let receipt = events(&fx, &a, "approved");
    assert_eq!(receipt.len(), 1);
    assert_eq!(receipt[0]["sha"], a_head);
    assert_eq!(receipt[0]["target_plan"]["plan"], plan.id);
    assert!(
        receipt[0]["landing_receipt"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    let dependency = prerequisite(&fx, &plan, &c);
    assert!(dependency.accepted && dependency.satisfied);
    assert_eq!(dependency.receipt_sha.as_deref(), Some(a_head.as_str()));
    assert_eq!(dependency.target_head.as_deref(), Some(head.as_str()));
    let c_run = dispatch(&mut fx, &c);
    assert_eq!(c_run.base_sha, head);
    assert!(corral_git::is_ancestor(&c_run.worktree, &a_head, &c_run.base_sha).unwrap());
    assert_eq!(git(&c_run.worktree, &["rev-parse", "HEAD"]), c_run.base_sha);
    assert_eq!(
        std::fs::read_to_string(c_run.worktree.join("a.txt")).unwrap(),
        "A\n"
    );
    reviewed(&mut fx, &c, "c.txt", "C\n");
    corral_ops::approve(&fx.ctx, &c, ApproveStrategy::FastForward, true, false).unwrap();
    assert_cleaned(&fx, &c_run, &git(&fx.app, &["rev-parse", TARGET]));
    assert_main(&fx, &main);
}

#[test]
fn done_and_keep_branch_acceptance_do_not_qualify_a_plan_prerequisite() {
    // Given A's completed candidate but no published target receipt.
    let (mut fx, plan, [a, _, c]) = plan();
    let main = git(&fx.app, &["rev-parse", "main"]);
    let run = dispatch(&mut fx, &a);
    reviewed(&mut fx, &a, "a.txt", "A\n");
    fx.ctx
        .store
        .update(&a, |task| {
            task.status = Status::Done;
            Ok(())
        })
        .unwrap();

    // When C is dispatched with a Done-only prerequisite, then no run is reserved.
    let dependency = prerequisite(&fx, &plan, &c);
    assert!(dependency.accepted && !dependency.satisfied);
    assert!(dependency.receipt_sha.is_none());
    assert!(corral_ops::dispatch(&fx.ctx, &c, &DispatchOpts::default()).is_err());
    assert!(fx.ctx.store.get(&c).unwrap().runs.is_empty());
    fx.ctx
        .store
        .update(&a, |task| {
            task.status = Status::Review;
            Ok(())
        })
        .unwrap();
    corral_ops::approve(&fx.ctx, &a, ApproveStrategy::KeepBranch, false, true).unwrap();
    let dependency = prerequisite(&fx, &plan, &c);
    assert!(dependency.accepted && !dependency.satisfied);
    assert!(dependency.receipt_sha.is_none());
    assert_eq!(events(&fx, &a, "approved")[0]["strategy"], "keep_branch");
    assert!(corral_ops::dispatch(&fx.ctx, &c, &DispatchOpts::default()).is_err());
    let child = fx.ctx.store.get(&c).unwrap();
    assert!(child.runs.is_empty() && child.launch.is_none());
    assert_eq!(git(&fx.app, &["rev-parse", TARGET]), main);
    assert!(run.worktree.is_dir());
    assert_main(&fx, &main);
}

#[test]
fn changed_patch_after_plan_rebase_cannot_carry_the_old_review() {
    // Given B reviewed two commits, one of which A independently integrates.
    let (mut fx, _, [a, b, _]) = plan();
    let main = git(&fx.app, &["rev-parse", "main"]);
    dispatch(&mut fx, &a);
    dispatch(&mut fx, &b);
    reviewed(&mut fx, &a, "shared.txt", "shared\n");
    fx.id = b.clone();
    fx.change("shared.txt", "shared\n");
    corral_git::commit_all(&fx.run().worktree, "shared change").unwrap();
    let old = reviewed(&mut fx, &b, "b.txt", "B\n");
    corral_ops::approve(&fx.ctx, &a, ApproveStrategy::FastForward, true, false).unwrap();
    let target = git(&fx.app, &["rev-parse", TARGET]);

    // When B's rebase drops the already integrated commit, approval is refused.
    assert!(corral_ops::approve(&fx.ctx, &b, ApproveStrategy::Rebase, true, false).is_err());

    // Then the new gate cannot authorize the changed patch with the old review.
    let run = fx.run();
    let gate = run.gate.unwrap();
    assert_eq!(fx.task().status, Status::Review);
    assert_eq!(gate.result, GateResult::Pass);
    assert_eq!(gate.identity.as_ref().unwrap().integration_base, target);
    assert_ne!(gate.sha, old.gate.unwrap().sha);
    assert_eq!(run.review.unwrap().sha, old.review.unwrap().sha);
    assert_eq!(events(&fx, &b, "gate_started").len(), 2);
    assert!(events(&fx, &b, "verdict_carried").is_empty());
    assert!(events(&fx, &b, "approved").is_empty());
    assert_eq!(git(&fx.app, &["rev-parse", TARGET]), target);
    assert!(fx.run().worktree.exists());
    assert_main(&fx, &main);
}

#[test]
fn dirty_integration_checkout_refuses_approval_without_mutating_candidates() {
    // Given a reviewed candidate and an owner edit in the integration checkout.
    let (mut fx, _, [a, _, _]) = plan();
    let main = git(&fx.app, &["rev-parse", "main"]);
    dispatch(&mut fx, &a);
    let run = reviewed(&mut fx, &a, "a.txt", "A\n");
    let checkout = &run.plan_target.as_ref().unwrap().checkout;
    let head = git(&run.worktree, &["rev-parse", "HEAD"]);
    let target = git(&fx.app, &["rev-parse", TARGET]);
    let dirty = b"owner's uncommitted edit\n";
    std::fs::write(checkout.join("README.md"), dirty).unwrap();

    // When approval reaches the owned-target boundary, then it fails safely.
    assert!(corral_ops::approve(&fx.ctx, &a, ApproveStrategy::Rebase, true, false).is_err());
    assert_eq!(fx.task().status, Status::Review);
    assert_eq!(git(&run.worktree, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(checkout, &["rev-parse", "HEAD"]), target);
    assert_eq!(git(&fx.app, &["rev-parse", TARGET]), target);
    assert_eq!(std::fs::read(checkout.join("README.md")).unwrap(), dirty);
    assert!(run.worktree.is_dir() && checkout.is_dir());
    assert_eq!(events(&fx, &a, "gate_started").len(), 1);
    assert!(events(&fx, &a, "verdict_carried").is_empty());
    assert!(events(&fx, &a, "approved").is_empty());
    assert_main(&fx, &main);
}

#[test]
fn ordinary_autopilot_lands_into_the_owned_plan_checkout() {
    // Given an independently reviewed Plan candidate with scoped acceptance authority.
    let (mut fx, plan, [a, _, _]) = plan();
    fx.ctx.store.config.review.accept = Some(Accept::Auto);
    fx.ctx.store.save_config().unwrap();
    fx.grant(std::slice::from_ref(&plan.id));
    let main = git(&fx.app, &["rev-parse", "main"]);
    let run = dispatch(&mut fx, &a);
    reviewed(&mut fx, &a, "a.txt", "A\n");
    // When the existing review autopilot consumes the fresh receipts.
    corral_ops::sync_at(&fx.ctx, now()).unwrap();
    // Then it lands through the controller checkout, without touching main.
    fx.id = a.clone();
    assert_eq!(
        fx.task().status,
        Status::Done,
        "reason={}; blocker={:?}",
        fx.reason(),
        corral_ops::automation_blocker(&fx.ctx, &fx.task())
    );
    let head = git(&fx.app, &["rev-parse", TARGET]);
    assert_cleaned(&fx, &run, &head);
    assert_eq!(events(&fx, &a, "approved").len(), 1);
    assert_main(&fx, &main);
}

#[test]
fn transient_target_index_lock_waits_for_a_later_pass_instead_of_escalating() {
    // Given a reviewed Plan candidate while another process briefly holds the target's index.lock.
    let (mut fx, plan, [a, _, _]) = plan();
    fx.ctx.store.config.review.accept = Some(Accept::Auto);
    fx.ctx.store.save_config().unwrap();
    fx.grant(std::slice::from_ref(&plan.id));
    let run = dispatch(&mut fx, &a);
    reviewed(&mut fx, &a, "a.txt", "A\n");
    let checkout = run.plan_target.as_ref().unwrap().checkout.clone();
    let admin = git(&checkout, &["rev-parse", "--absolute-git-dir"]);
    let lock = std::path::Path::new(&admin).join("index.lock");
    std::fs::write(&lock, "").unwrap();

    // When the autopilot pass runs, then the task waits: no escalation, no landing.
    corral_ops::sync_at(&fx.ctx, now()).unwrap();
    fx.id = a.clone();
    let stage = fx.run().autopilot.map(|state| state.stage);
    assert_ne!(stage, Some(AutoStage::Escalated), "reason={}", fx.reason());
    assert_eq!(fx.task().status, Status::Review);
    let wait = fx.task().infra_wait.unwrap();
    assert_eq!(wait.class, corral_core::InfraWaitClass::Contention);
    assert_eq!(wait.marker, "index.lock");
    assert_eq!(wait.path, lock);
    assert_eq!(wait.first_seen, now());
    assert!(wait.held_at.is_none());
    assert!(events(&fx, &a, "approved").is_empty());
    assert!(lock.exists());

    // And once the lock is released, a later pass proceeds and lands the task.
    std::fs::remove_file(&lock).unwrap();
    corral_ops::sync_at(&fx.ctx, now()).unwrap();
    assert_eq!(
        fx.task().status,
        Status::Done,
        "reason={}; blocker={:?}",
        fx.reason(),
        corral_ops::automation_blocker(&fx.ctx, &fx.task())
    );
    assert_eq!(events(&fx, &a, "approved").len(), 1);
}
