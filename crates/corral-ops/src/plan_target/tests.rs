use super::*;
use corral_core::{Origin, RunOutcome, Status};
use serde_json::json;

fn git(repo: &Path, args: &[&str]) -> String {
    let output = crate::test_support::command("git", repo)
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap().trim().into()
}

const APPROVAL_STARTED: &str = "2026-10-19T01:00:00Z";

struct Fixture {
    _dir: tempfile::TempDir,
    ctx: Ctx,
    repo: PathBuf,
    task: Task,
    target: Target,
    main: String,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("app");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-qb", "main"]);
        git(&repo, &["config", "user.name", "Fixture"]);
        git(&repo, &["config", "user.email", "fixture@example.invalid"]);
        git(&repo, &["config", "commit.gpgsign", "false"]);
        fs::write(repo.join("README"), "main bytes\n").unwrap();
        fs::write(repo.join(".gitignore"), ".worktrees/\n").unwrap();
        corral_git::commit_all(&repo, "initial").unwrap();
        let main = corral_git::rev_sha(&repo, "HEAD").unwrap();
        let ctx = crate::resources::FixedResourceProbe::ctx(Store::init(dir.path()).unwrap());
        let source = dir.path().join("plan.md");
        fs::write(&source, "---\ntitle: Plan\nproject: app\napproved_scope:\n  goal: Combined change\n  acceptance: [{scenario: together, expected: pass}]\n  delivery_target: {kind: integration, branch: feature/plan}\ntasks:\n  - {key: first, title: First}\n  - {key: consumer, title: Consumer, depends: [first]}\n---\n").unwrap();
        let imported = crate::import_plan(&ctx, &source, false, Origin::Agent).unwrap();
        let task = imported.tasks[0].clone();
        let _lock = corral_git::landing_lock(&repo).unwrap();
        let (target, binding) = prepare_dispatch(&ctx, &task).unwrap().unwrap();
        let worktree = repo.join(".worktrees").join(&task.id);
        let branch = format!("corral/{}", task.id);
        corral_git::worktree_add(&repo, &worktree, &branch, &target.head).unwrap();
        fs::write(worktree.join("first.txt"), "accepted work\n").unwrap();
        corral_git::commit_all(&worktree, "first").unwrap();
        let run: Run = serde_json::from_value(json!({
            "id":"R1", "agent":"fixture", "worktree":worktree, "branch":branch,
            "base":"feature/plan", "base_sha":target.head, "plan_target":binding,
            "started":"2026-10-19T00:00:00Z", "outcome":"done",
        }))
        .unwrap();
        let task = ctx
            .store
            .update(&task.id, |task| {
                task.status = Status::Review;
                task.runs.push(run);
                Ok(())
            })
            .unwrap();
        Self {
            _dir: dir,
            ctx,
            repo,
            task,
            target,
            main,
        }
    }

    fn prepare(&mut self) {
        let run = self.task.current_run().unwrap();
        let head = corral_git::rev_sha(&run.worktree, "HEAD").unwrap();
        self.target
            .prepare_landing(
                &self.task,
                run,
                &head,
                "fast_forward",
                None,
                LandingOptions {
                    remove_worktree: false,
                    keep_branch: false,
                    started: APPROVAL_STARTED.parse().unwrap(),
                },
            )
            .unwrap();
    }

    fn move_git(&self) {
        let run = self.task.current_run().unwrap();
        let head = corral_git::rev_sha(&run.worktree, "HEAD").unwrap();
        corral_git::land_checked(
            &self.target.checkout,
            &run.worktree,
            &run.branch,
            &run.base,
            &head,
            &self.target.head,
        )
        .unwrap();
    }

    fn reopen(&self) -> Ctx {
        Ctx {
            store: Store::open_unobserved(self.ctx.store.root()).unwrap(),
            ..self.ctx.clone()
        }
    }

    fn untouched_main(&self) {
        assert_eq!(corral_git::current_branch(&self.repo).unwrap(), "main");
        assert_eq!(corral_git::rev_sha(&self.repo, "HEAD").unwrap(), self.main);
        assert_eq!(
            corral_git::rev_sha(&self.repo, "refs/heads/main").unwrap(),
            self.main
        );
        assert_eq!(fs::read(self.repo.join("README")).unwrap(), b"main bytes\n");
        assert!(!self.repo.join("first.txt").exists());
    }

    fn approvals(&self) -> usize {
        self.ctx
            .store
            .events()
            .unwrap()
            .iter()
            .filter(|event| {
                event.kind == "approved" && event.task.as_deref() == Some(&self.task.id)
            })
            .count()
    }
}

#[test]
fn crash_after_git_before_receipt_recovers_exactly_once_without_an_engine() {
    // Given durable preparation and an advanced Git target, but no task/event receipt.
    let mut f = Fixture::new();
    {
        let _lock = corral_git::landing_lock(&f.repo).unwrap();
        f.prepare();
        f.move_git();
    }
    assert_eq!(f.approvals(), 0);
    assert_eq!(f.ctx.store.get(&f.task.id).unwrap().status, Status::Review);
    // When a fresh controller reconciles the same repository twice.
    let reopened = f.reopen();
    reconcile_all(&reopened).unwrap();
    reconcile_all(&reopened).unwrap();
    // Then exactly one target-qualified acceptance is published, with no repeated merge.
    assert_eq!(f.approvals(), 1);
    let target = Target::load(&f.repo, "feature/plan").unwrap().unwrap();
    assert!(target.pending.is_none());
    assert_eq!(target.receipts.len(), 1);
    assert!(target.receipts[0].is_published());
    let task = f.ctx.store.get(&f.task.id).unwrap();
    assert_eq!(task.status, Status::Done);
    assert_eq!(task.current_run().unwrap().outcome, Some(RunOutcome::Done));
    assert_eq!(
        target.head,
        corral_git::rev_sha(&f.repo, "feature/plan").unwrap()
    );
    let consumer = f
        .ctx
        .store
        .get(&f.ctx.store.get(&target.plan).unwrap().children[1])
        .unwrap();
    assert!(
        crate::tracker::target_blocker(&f.ctx.store, &consumer)
            .unwrap()
            .is_none()
    );
    assert!(
        crate::tracker::check_dispatch_base(&f.ctx.store, &consumer, "feature/plan", &f.main)
            .is_err()
    );
    assert!(
        crate::tracker::check_dispatch_base(&f.ctx.store, &consumer, "feature/plan", &target.head)
            .is_ok()
    );
    f.untouched_main();
}

#[test]
fn crash_after_receipt_before_task_publication_recovers_once() {
    // Given Git and the integrated receipt persisted before the task/event publication.
    let mut f = Fixture::new();
    {
        let _lock = corral_git::landing_lock(&f.repo).unwrap();
        f.prepare();
        f.move_git();
        let receipt = f.target.pending.take().unwrap();
        f.target.head = receipt.head.clone();
        f.target.receipts.push(receipt);
        f.target.save().unwrap();
    }
    // When recovery resumes the publication phase.
    reconcile_all(&f.reopen()).unwrap();
    reconcile_all(&f.reopen()).unwrap();
    // Then it finishes the existing transaction instead of preparing a second one.
    assert_eq!(f.approvals(), 1);
    assert_eq!(f.ctx.store.get(&f.task.id).unwrap().status, Status::Done);
    f.untouched_main();
}

#[test]
fn prepared_but_unperformed_landing_never_marks_task_accepted() {
    // Given a crash before any Git mutation.
    let mut f = Fixture::new();
    f.prepare();
    // When restart inspects the unchanged exact previous head.
    reconcile_all(&f.reopen()).unwrap();
    // Then preparation is released, not promoted to acceptance.
    let target = Target::load(&f.repo, "feature/plan").unwrap().unwrap();
    assert!(target.pending.is_none());
    assert!(target.receipts.is_empty());
    assert_eq!(f.approvals(), 0);
    assert_eq!(f.ctx.store.get(&f.task.id).unwrap().status, Status::Review);
    f.untouched_main();
}

#[test]
fn unrecorded_descendant_ref_movement_cannot_reconcile_a_prepared_landing() {
    // Given the intended candidate followed by an unrecorded external commit.
    let mut f = Fixture::new();
    f.prepare();
    f.move_git();
    git(
        &f.target.checkout,
        &["commit", "--allow-empty", "-qm", "external"],
    );
    let external = corral_git::rev_sha(&f.repo, "feature/plan").unwrap();
    // When the controller attempts recovery.
    assert!(reconcile_all(&f.reopen()).is_err());
    // Then ancestry alone cannot turn an external movement into our receipt.
    assert_eq!(f.approvals(), 0);
    assert_eq!(f.ctx.store.get(&f.task.id).unwrap().status, Status::Review);
    assert_eq!(
        corral_git::rev_sha(&f.repo, "feature/plan").unwrap(),
        external
    );
    assert!(
        Target::load(&f.repo, "feature/plan")
            .unwrap()
            .unwrap()
            .pending
            .is_some()
    );
    f.untouched_main();
}

#[test]
fn dirty_integration_checkout_retains_bytes_and_unpublished_intent() {
    // Given a crash after Git movement and an untracked file in the integration checkout.
    let mut f = Fixture::new();
    f.prepare();
    f.move_git();
    fs::write(f.target.checkout.join("untracked"), "retain me").unwrap();
    // When recovery validates ownership and cleanliness.
    assert!(reconcile_all(&f.reopen()).is_err());
    // Then it refuses without deleting work or publishing acceptance.
    assert_eq!(f.approvals(), 0);
    assert_eq!(
        fs::read(f.target.checkout.join("untracked")).unwrap(),
        b"retain me"
    );
    assert_eq!(f.ctx.store.get(&f.task.id).unwrap().status, Status::Review);
    f.untouched_main();
}

#[test]
fn wrong_store_owner_and_changed_plan_contract_refuse_target_use() {
    // Given a target whose durable ownership no longer matches the selected store.
    let mut f = Fixture::new();
    let owner = f.target.owner.clone();
    f.target.owner = f.repo.clone();
    f.target.save().unwrap();
    // When dispatch or verification attempts to use it.
    assert!(prepare_dispatch(&f.ctx, &f.task).is_err());
    assert!(validate_run(&f.ctx.store, &f.task, f.task.current_run().unwrap()).is_err());
    // Then no new ref or acceptance was produced.
    assert_eq!(f.approvals(), 0);
    f.untouched_main();
    f.target.owner = owner;
    f.target.save().unwrap();
    let plan = f.ctx.store.get(&f.target.plan).unwrap();
    let mut scope = plan.approved_scope.clone().unwrap();
    scope.goal = "Changed scope".into();
    f.ctx
        .store
        .amend_plan_scope(&plan.id, plan.approved_scope_revision, scope, |_, _, _| {
            Ok(())
        })
        .unwrap();
    assert!(validate_run(&f.ctx.store, &f.task, f.task.current_run().unwrap()).is_err());
}

#[test]
fn missing_owned_checkout_reconstructs_but_missing_initialized_ref_does_not() {
    // Given an owned target whose checkout was safely removed.
    let f = Fixture::new();
    corral_git::worktree_remove(&f.repo, &f.target.checkout, false).unwrap();
    // When the next dispatch resolves the target.
    prepare_dispatch(&f.ctx, &f.task).unwrap();
    // Then it reconstructs from the owned ref without moving main.
    assert!(f.target.checkout.is_dir());
    f.untouched_main();
    corral_git::worktree_remove(&f.repo, &f.target.checkout, false).unwrap();
    git(&f.repo, &["branch", "-D", "feature/plan"]);
    assert!(prepare_dispatch(&f.ctx, &f.task).is_err());
    assert!(corral_git::rev_sha(&f.repo, "refs/heads/feature/plan").is_err());
}

#[test]
fn task_cleanup_cannot_remove_or_discard_the_controller_target() {
    // Given a forged run that points at the controller's path and branch.
    let f = Fixture::new();
    let mut run = f.task.current_run().unwrap().clone();
    run.worktree = f.target.checkout.clone();
    run.branch = "feature/plan".into();
    // When normal and explicit-discard cleanup inspect it.
    assert!(crate::cleanup_worktree::remove(&f.ctx, &f.task, &f.repo, &run, false).is_err());
    assert!(crate::cleanup_worktree::remove(&f.ctx, &f.task, &f.repo, &run, true).is_err());
    assert!(crate::cleanup_worktree::discard_branch(&f.repo, &run).is_err());
    // Then persistent integration data and main remain intact.
    assert!(f.target.checkout.is_dir());
    assert_eq!(
        corral_git::rev_sha(&f.repo, "feature/plan").unwrap(),
        f.main
    );
    f.untouched_main();
}

#[test]
fn event_publication_before_journal_acknowledgement_is_not_duplicated() {
    // Given a successful landing whose last journal acknowledgement was lost.
    let mut f = Fixture::new();
    f.prepare();
    f.move_git();
    reconcile_all(&f.reopen()).unwrap();
    let target = Target::load(&f.repo, "feature/plan").unwrap().unwrap();
    let mut value = serde_json::to_value(target).unwrap();
    value["receipts"][0]["published"] = json!(false);
    let target: Target = serde_json::from_value(value).unwrap();
    target.save().unwrap();
    // When restart sees the already durable acceptance event.
    reconcile_all(&f.reopen()).unwrap();
    // Then it acknowledges that event, rather than emitting another one.
    assert_eq!(f.approvals(), 1);
    assert!(
        Target::load(&f.repo, "feature/plan")
            .unwrap()
            .unwrap()
            .receipts[0]
            .is_published()
    );
    f.untouched_main();
}

#[test]
fn plan_accepted_code_cannot_unlock_an_unscoped_main_consumer() {
    // Given accepted Plan work which is deliberately absent from main.
    let mut f = Fixture::new();
    f.prepare();
    f.move_git();
    reconcile_all(&f.reopen()).unwrap();
    let consumer = f
        .ctx
        .store
        .create("Outside consumer", Some("app"), Status::Ready, "")
        .unwrap();
    let consumer = f
        .ctx
        .store
        .update(&consumer.id, |task| {
            task.depends_on.push(f.task.id.clone());
            Ok(())
        })
        .unwrap();
    // When a non-Plan consumer prepares main as its exact dispatch base.
    let result = crate::tracker::check_dispatch_base(&f.ctx.store, &consumer, "main", &f.main);
    // Then Done and a Plan receipt still cannot substitute for actual inclusion.
    assert!(result.is_err());
    assert!(consumer.runs.is_empty());
    f.untouched_main();
}

#[test]
fn dirty_project_checkout_is_preserved_while_owned_plan_target_lands() {
    // Given owner edits in the project's main checkout, not the integration checkout.
    let mut f = Fixture::new();
    fs::write(f.repo.join("README"), "owner bytes\n").unwrap();
    // When the controller integrates and recovers its Plan landing.
    f.prepare();
    f.move_git();
    reconcile_all(&f.reopen()).unwrap();
    // Then it never treats the project checkout as a writable landing target.
    assert_eq!(f.ctx.store.get(&f.task.id).unwrap().status, Status::Done);
    assert_eq!(
        corral_git::rev_sha(&f.repo, "refs/heads/main").unwrap(),
        f.main
    );
    assert_eq!(corral_git::current_branch(&f.repo).unwrap(), "main");
    assert_eq!(fs::read(f.repo.join("README")).unwrap(), b"owner bytes\n");
}

#[test]
fn interrupted_initial_claim_reaches_its_checked_dispatch_recovery() {
    // Given an initial claim interrupted before its final initialization receipt.
    let mut f = Fixture::new();
    f.target.initialized = false;
    f.target.save().unwrap();
    // When admission resumes the same owned claim.
    check_dispatch_target(&f.ctx.store, &f.task).unwrap();
    let (target, _) = prepare_dispatch(&f.ctx, &f.task).unwrap().unwrap();
    // Then full validation precedes admission and no second target is created.
    target.check().unwrap();
    assert_eq!(target.head, f.main);
    assert_eq!(f.approvals(), 0);
    f.untouched_main();
}

#[test]
fn explicit_target_reconciliation_invalidates_old_gate_identity() {
    // Given a candidate checked under the original approved Plan contract.
    let f = Fixture::new();
    let run = f.task.current_run().unwrap();
    let old = crate::verification::capture_identity(&f.ctx, &f.task, run, "attempt").unwrap();
    let plan = f.ctx.store.get(&f.target.plan).unwrap();
    let mut scope = plan.approved_scope.clone().unwrap();
    scope.goal = "Reconciled scope".into();
    let plan = f
        .ctx
        .store
        .amend_plan_scope(&plan.id, plan.approved_scope_revision, scope, |_, _, _| {
            Ok(())
        })
        .unwrap();
    let task = f
        .ctx
        .store
        .update(&f.task.id, |task| {
            task.current_run_mut().unwrap().plan_target = Some(f.target.binding(&plan)?);
            Ok(())
        })
        .unwrap();
    // When an explicit reconciliation preserves Git SHAs but binds the new scope.
    let current =
        crate::verification::identity(&f.ctx, &task, task.current_run().unwrap(), &old).unwrap();
    // Then the old verification identity cannot authorize the new Plan contract.
    assert_eq!(old.head, current.head);
    assert_eq!(old.integration_base, current.integration_base);
    assert_ne!(old.policy_digest, current.policy_digest);
    f.untouched_main();
}

#[test]
fn first_owner_plan_target_approval_times_landing_from_its_recorded_start() {
    // Given a first owner approval prepared at APPROVAL_STARTED and recovered after Git moved.
    let mut f = Fixture::new();
    {
        let _lock = corral_git::landing_lock(&f.repo).unwrap();
        f.prepare();
        f.move_git();
    }
    reconcile_all(&f.reopen()).unwrap();
    // When the published events are paired into phases.
    let events = f.ctx.store.events().unwrap();
    let approved = events.iter().find(|e| e.kind == "approved").unwrap();
    assert_eq!(approved.detail["started"], APPROVAL_STARTED);
    let task = f.ctx.store.get(&f.task.id).unwrap();
    let spans = crate::phases::task_phases(&task, &events.iter().collect::<Vec<_>>());
    // Then landing runs from the recorded start to the publication, not unknown.
    let landing = spans
        .iter()
        .find(|s| s.phase == crate::phases::Phase::Landing)
        .unwrap();
    assert_eq!(
        landing.start,
        APPROVAL_STARTED.parse::<jiff::Timestamp>().unwrap()
    );
    assert!(landing.complete, "{landing:?}");
    assert_eq!(landing.outcome, "landed");
}
