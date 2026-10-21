use super::*;
use corral_core::{
    AgentCmd, AggregateReview, AggregateStrategy, OperatingAuthority, OperatingSelection, Origin,
    Run, Store, Verdict,
};
use std::path::{Path, PathBuf};

mod integrity;
mod landing;
mod policy;
mod review;

pub(super) fn git(repo: &Path, args: &[&str]) -> String {
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

pub(super) struct Fixture {
    pub(super) _dir: tempfile::TempDir,
    pub(super) ctx: Ctx,
    pub(super) repo: PathBuf,
    pub(super) plan: String,
    pub(super) members: Vec<String>,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Delivery {
    Integration,
    Main,
    NoCode,
}

impl Fixture {
    /// Two required members accepted into the Plan target, each passing locally.
    pub(super) fn new(delivery: Delivery, gate: &[&str]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("app");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-qb", "main"]);
        git(&repo, &["config", "user.name", "Fixture"]);
        git(&repo, &["config", "user.email", "fixture@example.invalid"]);
        git(&repo, &["config", "commit.gpgsign", "false"]);
        std::fs::write(repo.join("README"), "main bytes\n").unwrap();
        std::fs::write(repo.join(".gitignore"), ".worktrees/\n").unwrap();
        corral_git::commit_all(&repo, "initial").unwrap();
        let repo = repo.canonicalize().unwrap();
        let mut store = Store::init(dir.path()).unwrap();
        store.config.review.gate = Some(gate.iter().map(|c| (*c).into()).collect());
        store.config.review.reviewer = Some("rev".into());
        store.config.agents.insert(
            "rev".into(),
            AgentCmd {
                command: "true".into(),
            },
        );
        store.save_config().unwrap();
        let mut ctx = crate::resources::FixedResourceProbe::ctx(store);
        // `pane run` executes the reviewer command synchronously; never a live session.
        let herdr = dir.path().join("herdr");
        std::os::unix::fs::symlink(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gate_ops/herdr.sh"),
            &herdr,
        )
        .unwrap();
        ctx.herdr = corral_herdr::Herdr::with_bin(herdr, Some("corral-test".into()));
        let target = match delivery {
            Delivery::Integration => {
                "\n  delivery_target: {kind: integration, branch: feature/plan}"
            }
            Delivery::Main | Delivery::NoCode => "",
        };
        let source = dir.path().join("plan.md");
        std::fs::write(
            &source,
            format!("---\ntitle: Plan\nproject: app\napproved_scope:\n  goal: Combined change\n  acceptance:\n    - {{criterion: C1, scenario: first works, expected: pass}}\n    - {{criterion: C2, scenario: first and second work together, expected: pass together}}{target}\ntasks:\n  - {{key: first, title: First}}\n  - {{key: second, title: Second}}\n---\n"),
        )
        .unwrap();
        let imported = crate::import_plan(&ctx, &source, false, Origin::Agent).unwrap();
        let fixture = Self {
            _dir: dir,
            ctx,
            repo,
            plan: imported.tracker.id.clone(),
            members: imported.tasks.iter().map(|task| task.id.clone()).collect(),
        };
        for (index, member) in fixture.members.iter().enumerate() {
            let file = ["first.txt", "second.txt"][index];
            match delivery {
                Delivery::Integration => fixture.land_integration(member, file),
                Delivery::Main => fixture.land_main(member, Some(file)),
                Delivery::NoCode => fixture.land_main(member, None),
            }
        }
        fixture
    }

    fn land_integration(&self, id: &str, file: &str) {
        let task = self.ctx.store.get(id).unwrap();
        {
            let _lock = corral_git::landing_lock(&self.repo).unwrap();
            let (mut target, binding) = crate::plan_target::prepare_dispatch(&self.ctx, &task)
                .unwrap()
                .unwrap();
            let worktree = self.repo.join(".worktrees").join(id);
            let branch = format!("corral/{id}");
            corral_git::worktree_add(&self.repo, &worktree, &branch, &target.head).unwrap();
            std::fs::write(worktree.join(file), "accepted work\n").unwrap();
            corral_git::commit_all(&worktree, id).unwrap();
            let head = corral_git::rev_sha(&worktree, "HEAD").unwrap();
            let run: Run = serde_json::from_value(json!({
                "id":"R1", "agent":"fixture", "worktree":worktree, "branch":branch,
                "base":"feature/plan", "base_sha":target.head, "plan_target":binding,
                "started":"2026-10-19T00:00:00Z", "outcome":"done",
            }))
            .unwrap();
            let task = self
                .ctx
                .store
                .update(id, |task| {
                    task.status = Status::Review;
                    task.runs.push(run);
                    Ok(())
                })
                .unwrap();
            let run = task.current_run().unwrap();
            target
                .prepare_landing(
                    &task,
                    run,
                    &head,
                    "fast_forward",
                    None,
                    crate::plan_target::LandingOptions {
                        remove_worktree: false,
                        keep_branch: false,
                        started: Timestamp::now(),
                    },
                )
                .unwrap();
            corral_git::land_checked(
                &target.checkout,
                &worktree,
                &branch,
                "feature/plan",
                &head,
                &target.head,
            )
            .unwrap();
            // Reconcile under the held lock, as approval does. `reconcile_all` skips a busy
            // lock, and a parallel test's forked child can briefly hold an inherited copy.
            target.reconcile(&self.ctx).unwrap();
        }
        assert_eq!(self.ctx.store.get(id).unwrap().status, Status::Done);
    }

    /// Accepted directly on main; `None` accepts a run that added no commits.
    pub(super) fn land_main(&self, id: &str, file: Option<&str>) {
        let base = self.main();
        if let Some(file) = file {
            std::fs::write(self.repo.join(file), "accepted work\n").unwrap();
            corral_git::commit_all(&self.repo, id).unwrap();
        }
        self.accept_receipt(id, &base, &self.main(), "fast_forward");
    }

    pub(super) fn accept_receipt(&self, id: &str, base: &str, sha: &str, strategy: &str) {
        self.ctx
            .store
            .update(id, |task| {
                task.status = Status::Done;
                task.runs = vec![
                    serde_json::from_value(json!({
                        "id":"R1", "agent":"fixture", "worktree":"/unused",
                        "branch":format!("corral/{id}"), "base":"main", "base_sha":base,
                        "started":"2026-10-19T00:00:00Z", "outcome":"done",
                    }))
                    .unwrap(),
                ];
                Ok(())
            })
            .unwrap();
        self.ctx
            .store
            .log_event(
                Some(id),
                "approved",
                json!({"run":"R1", "sha":sha, "strategy":strategy, "base":"main"}),
            )
            .unwrap();
    }

    pub(super) fn main(&self) -> String {
        corral_git::rev_sha(&self.repo, "refs/heads/main").unwrap()
    }

    pub(super) fn commits(&self) -> String {
        git(&self.repo, &["rev-list", "--all", "--count"])
    }

    pub(super) fn plan(&self) -> Task {
        self.ctx.store.get(&self.plan).unwrap()
    }

    pub(super) fn observe(
        &self,
        attempt: &str,
        criterion: &str,
        status: ObservationStatus,
    ) -> Result<EvidenceRecord> {
        let scope = self.plan().approved_scope.unwrap();
        let index = scope
            .criterion_keys()
            .iter()
            .position(|key| key == criterion)
            .unwrap();
        let observation: EvidenceObservation = serde_json::from_value(json!({
            "version":1, "key":format!("{attempt}-{criterion}-{status:?}"),
            "criterion":criterion, "run":"aggregate", "attempt":attempt, "status":status,
            "action":"run the combined scenario", "environment":"fixture",
            "expected":scope.acceptance[index].expected, "observed":"observed",
            "artifacts":[], "captured":Timestamp::now(), "producer":"test", "live":false,
        }))
        .unwrap();
        plan_evidence(&self.ctx, &self.plan, &observation)
    }

    /// A verified attempt with both aggregate scenarios observed passing.
    pub(super) fn passing(&self) -> AggregateAttempt {
        let attempt = plan_verify(&self.ctx, &self.plan).unwrap();
        for criterion in ["C1", "C2"] {
            self.observe(&attempt.id, criterion, ObservationStatus::Pass)
                .unwrap();
        }
        attempt
    }

    /// Launch the configured reviewer; the fixture reviewer never calls back by itself.
    pub(super) fn launch(&self, attempt: &str) -> Result<AggregateReview> {
        plan_aggregate_review(&self.ctx, &self.plan, attempt)
    }

    pub(super) fn verdict(
        &self,
        attempt: &str,
        review: &str,
        verdict: Verdict,
        notes: &str,
    ) -> Result<AggregateReview> {
        let input = AggregateVerdict {
            attempt,
            review,
            verdict,
            notes,
        };
        plan_aggregate_verdict(&self.ctx, &self.plan, &input)
    }

    /// A launched review answered through its own callback.
    pub(super) fn review(&self, attempt: &str, verdict: Verdict) -> Result<AggregateReview> {
        let review = self.launch(attempt)?;
        self.verdict(attempt, &review.id, verdict, "aggregate notes")
    }

    /// `passing`, plus an approving review by the configured reviewer.
    pub(super) fn reviewed(&self) -> AggregateAttempt {
        let attempt = self.passing();
        self.review(&attempt.id, Verdict::Approve).unwrap();
        attempt
    }

    pub(super) fn grant(&self, authority: OperatingAuthority) {
        let preview = self
            .ctx
            .store
            .preview_operating_policy(&OperatingSelection {
                tasks: [self.plan.clone()].into(),
                authority,
                ..Default::default()
            })
            .unwrap();
        self.ctx
            .store
            .confirm_operating_policy(
                &preview,
                "fixture owner",
                "2026-10-19T00:00:00Z".parse().unwrap(),
            )
            .unwrap();
    }

    pub(super) fn accepted_events(&self) -> usize {
        self.ctx
            .store
            .events()
            .unwrap()
            .iter()
            .filter(|event| event.kind == "plan_accepted")
            .count()
    }

    pub(super) fn assert_not_completed(&self, main: &str) {
        assert_eq!(self.main(), main);
        assert_ne!(self.plan().status, Status::Done);
        assert_eq!(self.accepted_events(), 0);
    }
}

pub(super) fn message<T: std::fmt::Debug>(result: Result<T>) -> String {
    format!("{:#}", result.unwrap_err())
}

#[test]
fn local_task_pass_cannot_hide_a_failing_aggregate_scenario() {
    // Given both members accepted locally and a passing combined gate.
    let f = Fixture::new(
        Delivery::Integration,
        &["test -f first.txt", "test -f second.txt"],
    );
    let main = f.main();
    let attempt = plan_verify(&f.ctx, &f.plan).unwrap();
    assert_eq!(attempt.gate.as_ref().unwrap().result, GateResult::Pass);
    // When only C1 is observed, the owner cannot accept a not-run aggregate scenario.
    f.observe(&attempt.id, "C1", ObservationStatus::Pass)
        .unwrap();
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, false));
    assert!(
        error.contains("criterion C2 has no aggregate observation"),
        "{error}"
    );
    // When the cross-task scenario fails, acceptance names it and nothing lands.
    f.observe(&attempt.id, "C2", ObservationStatus::Fail)
        .unwrap();
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, false));
    assert!(error.contains("criterion C2 is fail"), "{error}");
    f.assert_not_completed(&main);
}

#[test]
fn failing_combined_gate_blocks_acceptance_despite_passing_members() {
    // Given members that each passed, but a combined candidate the gate rejects.
    let f = Fixture::new(Delivery::Integration, &["test -f third.txt"]);
    let main = f.main();
    let attempt = plan_verify(&f.ctx, &f.plan).unwrap();
    // Then the aggregate gate receipt is Fail and scenario passes cannot override it.
    let gate = attempt.gate.as_ref().unwrap();
    assert_eq!(gate.result, GateResult::Fail);
    assert_eq!(gate.failed_cmd.as_deref(), Some("test -f third.txt"));
    for criterion in ["C1", "C2"] {
        f.observe(&attempt.id, criterion, ObservationStatus::Pass)
            .unwrap();
    }
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, false));
    assert!(error.contains("aggregate gate has not passed"), "{error}");
    f.assert_not_completed(&main);
}

#[test]
fn moved_main_head_spec_or_policy_invalidates_prior_plan_proof() {
    let gate = ["test -f first.txt", "test -f second.txt"];
    // Moved main: the Plan candidate no longer contains the final target base.
    let f = Fixture::new(Delivery::Integration, &gate);
    let attempt = f.passing();
    std::fs::write(f.repo.join("other.txt"), "unrelated\n").unwrap();
    corral_git::commit_all(&f.repo, "unrelated main work").unwrap();
    let main = f.main();
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, false));
    assert!(error.contains("does not include current main"), "{error}");
    assert!(
        f.observe(&attempt.id, "C1", ObservationStatus::Pass)
            .is_err()
    );
    f.assert_not_completed(&main);

    // Moved head: a direct-to-main candidate changed after verification.
    let f = Fixture::new(Delivery::Main, &gate);
    let attempt = f.passing();
    std::fs::write(f.repo.join("later.txt"), "later\n").unwrap();
    corral_git::commit_all(&f.repo, "later main work").unwrap();
    let main = f.main();
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, false));
    assert!(error.contains("stale Plan proof"), "{error}");
    assert!(error.contains("head"), "{error}");
    f.assert_not_completed(&main);

    // Changed spec: an approved scope amendment changes the Plan contract.
    let f = Fixture::new(Delivery::Integration, &gate);
    let attempt = f.passing();
    let plan = f.plan();
    let mut scope = plan.approved_scope.clone().unwrap();
    scope.goal = "Changed goal".into();
    f.ctx
        .store
        .amend_plan_scope(&plan.id, plan.approved_scope_revision, scope, |_, _, _| {
            Ok(())
        })
        .unwrap();
    let main = f.main();
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, false));
    assert!(error.contains("spec_digest"), "{error}");
    f.assert_not_completed(&main);

    // Changed policy: the gate the attempt ran is no longer the configured gate.
    let f = Fixture::new(Delivery::Integration, &gate);
    let attempt = f.passing();
    let mut store = f.ctx.store.reopen().unwrap();
    store.config.review.gate = Some(vec!["true".into()]);
    store.save_config().unwrap();
    let main = f.main();
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, false));
    assert!(error.contains("policy_digest"), "{error}");
    assert!(f.review(&attempt.id, Verdict::Approve).is_err());
    f.assert_not_completed(&main);
}

#[test]
fn direct_to_main_and_no_code_completion_create_no_commits() {
    // Given a Plan whose members already landed on main.
    let f = Fixture::new(Delivery::Main, &["test -f first.txt", "test -f second.txt"]);
    let (main, commits) = (f.main(), f.commits());
    let attempt = f.reviewed();
    // When the owner accepts the exact verified main head.
    let acceptance = plan_accept(&f.ctx, &f.plan, &attempt.id, false).unwrap();
    // Then completion is recorded without a merge, empty commit or ref change.
    assert_eq!(acceptance.strategy, AggregateStrategy::Direct);
    assert_eq!(acceptance.after.as_deref(), Some(main.as_str()));
    assert_eq!((f.main(), f.commits()), (main, commits));
    assert_eq!(f.plan().status, Status::Done);

    // Given a Plan whose accepted members changed no code.
    let f = Fixture::new(Delivery::NoCode, &["false"]);
    let (main, commits) = (f.main(), f.commits());
    let attempt = f.reviewed();
    // Then it binds artifacts instead of a gate run or verification checkout.
    assert!(!attempt.code && attempt.gate.is_none());
    assert!(attempt.identity.head.is_empty());
    let acceptance = plan_accept(&f.ctx, &f.plan, &attempt.id, false).unwrap();
    assert_eq!(acceptance.strategy, AggregateStrategy::NoCode);
    assert_eq!((acceptance.before, acceptance.after), (None, None));
    assert_eq!((f.main(), f.commits()), (main, commits));
    assert!(
        !f.repo
            .join(".worktrees")
            .join(format!("{}-verify", f.plan))
            .exists()
    );
    assert_eq!(f.plan().status, Status::Done);
}

#[test]
fn plan_with_unmet_required_work_cannot_complete() {
    // Given a Plan with acceptance criteria whose members are all Done locally.
    let f = Fixture::new(Delivery::Main, &["true"]);
    let main = f.main();
    // Then all-children-Done makes it ready for aggregate acceptance, not Done.
    assert!(
        crate::tracker::reconcile_one(&f.ctx, &f.plan)
            .unwrap()
            .is_none()
    );
    let progress = crate::plan_progress(&f.ctx, &f.plan).unwrap();
    let aggregate = progress.aggregate.unwrap();
    assert!(aggregate.ready && !aggregate.accepted, "{aggregate:?}");
    // And a forced manual move cannot bypass aggregate acceptance.
    let error = message(crate::move_task(&f.ctx, &f.plan, Status::Done, true));
    assert!(error.contains("aggregate acceptance"), "{error}");
    // When required work is reopened, dropped or accepted without integration.
    for (status, strategy, reason) in [
        (Status::Review, "fast_forward", "required work is Review"),
        (Status::Dropped, "fast_forward", "required work is Dropped"),
        (Status::Done, "keep_branch", "was not integrated"),
    ] {
        f.accept_receipt(&f.members[1], &main, &main, strategy);
        f.ctx
            .store
            .update(&f.members[1], |task| {
                task.status = status;
                Ok(())
            })
            .unwrap();
        // Then verification and readiness name the gap instead of skipping it.
        let error = message(plan_verify(&f.ctx, &f.plan));
        assert!(
            error.contains(&f.members[1]) && error.contains(reason),
            "{error}"
        );
        let aggregate = crate::plan_progress(&f.ctx, &f.plan)
            .unwrap()
            .aggregate
            .unwrap();
        assert!(!aggregate.ready, "{aggregate:?}");
        assert!(
            crate::tracker::reconcile_one(&f.ctx, &f.plan)
                .unwrap()
                .is_none()
        );
    }
    assert!(f.plan().aggregate_attempts.is_empty());
    f.assert_not_completed(&main);
}

#[test]
fn plain_tracker_without_plan_scope_keeps_child_completion() {
    // Given a legacy tracker with no approved scope and every child Done.
    let f = Fixture::new(Delivery::Main, &["true"]);
    let tracker = f
        .ctx
        .store
        .create("Legacy tracker", Some("app"), Status::Backlog, "")
        .unwrap();
    let tracker = f
        .ctx
        .store
        .update(&tracker.id, |task| {
            task.tracker = true;
            task.children = vec![f.members[0].clone()];
            Ok(())
        })
        .unwrap();
    // Then today's behavior is unchanged and no aggregate projection appears.
    assert!(!tracker.plan_acceptance_required());
    assert!(
        crate::tracker::reconcile_one(&f.ctx, &tracker.id)
            .unwrap()
            .is_some()
    );
    assert_eq!(f.ctx.store.get(&tracker.id).unwrap().status, Status::Done);
}
