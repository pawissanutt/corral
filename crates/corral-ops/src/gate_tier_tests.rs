use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use corral_core::{
    CandidateIdentity, ExecutionSnapshot, GateResult, Origin, ResolvedProfile, ScopeGate,
    TierPolicy,
};
use serde_json::json;

use super::*;
use crate::verification;

const FULL: &str = "full-gate";
const QUICK: &str = "quick-gate";

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

pub(crate) struct Fx {
    _dir: tempfile::TempDir,
    pub(crate) ctx: Ctx,
    pub(crate) repo: PathBuf,
}

/// Project "app" on `main`. A tiered fixture defines quick and selects it for Plan members and
/// main-bound review rounds. Receipts are bound identities; no gate command runs.
pub(crate) fn fixture(tiered: bool) -> Fx {
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
    let mut store = corral_core::Store::init(dir.path()).unwrap();
    store.config.review.gate = Some(vec![FULL.into()]);
    if tiered {
        store.config.review.gate_tiers =
            Some(BTreeMap::from([("quick".into(), vec![QUICK.into()])]));
        store.config.review.tier_policy = Some(TierPolicy {
            plan_member: Some("quick".into()),
            review_rounds: Some("quick".into()),
        });
    }
    store.save_config().unwrap();
    let mut ctx = crate::resources::FixedResourceProbe::ctx(store);
    let herdr = dir.path().join("herdr");
    std::os::unix::fs::symlink(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gate_ops/herdr.sh"),
        &herdr,
    )
    .unwrap();
    ctx.herdr = corral_herdr::Herdr::with_bin(herdr, None);
    Fx {
        _dir: dir,
        ctx,
        repo,
    }
}

impl Fx {
    /// A tiered run of `task` in its own worktree off `base` at `head`.
    fn run(&self, task: &Task, base: &str, head: &str, target: Option<RunPlanTargetJson>) -> Task {
        let worktree = self.repo.join(".worktrees").join(&task.id);
        let branch = format!("corral/{}", task.id);
        corral_git::worktree_add(&self.repo, &worktree, &branch, head).unwrap();
        std::fs::write(worktree.join("work.txt"), "candidate\n").unwrap();
        corral_git::commit_all(&worktree, "work").unwrap();
        let run: Run = serde_json::from_value(json!({
            "id":"R1", "agent":"fixture", "worktree":worktree, "branch":branch, "base":base,
            "base_sha":head, "plan_target":target, "started":"2026-10-23T00:00:00Z",
            "gate_rules":"tiered",
        }))
        .unwrap();
        self.ctx
            .store
            .update(&task.id, |task| {
                task.status = corral_core::Status::Running;
                task.runs.push(run);
                Ok(())
            })
            .unwrap()
    }

    pub(crate) fn main_bound(&self) -> Task {
        let task = self
            .ctx
            .store
            .create("Main-bound", Some("app"), corral_core::Status::Ready, "")
            .unwrap();
        let main = corral_git::rev_sha(&self.repo, "main").unwrap();
        self.run(&task, "main", &main, None)
    }

    /// The first member of an imported integration Plan, bound to its owned target.
    fn member(&self) -> (Task, String) {
        let (mut members, tracker) = self.members(1);
        (members.remove(0), tracker)
    }

    /// The first `running` of the two members of an imported integration Plan, each bound to
    /// the owned target, and the tracker.
    pub(crate) fn members(&self, running: usize) -> (Vec<Task>, String) {
        let source = self.repo.parent().unwrap().join("plan.md");
        std::fs::write(&source, "---\ntitle: Plan\nproject: app\napproved_scope:\n  goal: Combined change\n  acceptance: [{scenario: together, expected: pass}]\n  delivery_target: {kind: integration, branch: feature/plan}\ntasks:\n  - {key: first, title: First}\n  - {key: second, title: Second}\n---\n").unwrap();
        let imported = crate::import_plan(&self.ctx, &source, false, Origin::Agent).unwrap();
        let members = imported.tasks[..running]
            .iter()
            .map(|task| {
                let (target, binding) = {
                    let _lock = corral_git::landing_lock(&self.repo).unwrap();
                    crate::plan_target::prepare_dispatch(&self.ctx, task)
                        .unwrap()
                        .unwrap()
                };
                let binding = serde_json::to_value(binding).unwrap();
                self.run(task, "feature/plan", &target.head, Some(binding))
            })
            .collect();
        (members, imported.tracker.id)
    }

    pub(crate) fn current(&self, id: &str) -> (Task, Run) {
        let task = self.ctx.store.get(id).unwrap();
        let run = task.current_run().unwrap().clone();
        (task, run)
    }

    /// A passing receipt bound like a gate started now in `phase`, saved as the run's gate.
    pub(crate) fn receipt(&self, id: &str, phase: GatePhase, attempt: &str) -> GateRecord {
        let (task, run) = self.current(id);
        let contract = select(&self.ctx, &task, &run, phase).unwrap();
        let identity = verification::capture_identity_with(
            &self.ctx,
            &task,
            &run,
            attempt,
            contract.binding(),
        )
        .unwrap();
        let gate = GateRecord {
            sha: identity.head.clone(),
            identity: Some(identity),
            commands: Vec::new(),
            log_sha256: None,
            result: GateResult::Pass,
            process_lost: false,
            failed_cmd: None,
            at: corral_core::clock::now(),
            pane: None,
            socket: None,
            tier: None,
        };
        self.ctx
            .store
            .update(id, |task| {
                task.current_run_mut().unwrap().gate = Some(gate.clone());
                Ok(())
            })
            .unwrap();
        gate
    }

    pub(crate) fn fresh(&self, id: &str, gate: &GateRecord) -> bool {
        let (task, run) = self.current(id);
        verification::gate_fresh(&self.ctx, &task, &run, gate).unwrap()
    }

    pub(crate) fn review(&self, id: &str, verdict: Verdict, identity: &CandidateIdentity) {
        let (_, run) = self.current(id);
        let evidence = verification::evidence_digest(&run).unwrap();
        let review: corral_core::ReviewRecord = serde_json::from_value(json!({
            "identity": identity, "evidence_digest": evidence, "verdict": verdict,
            "reviewer": "owner", "sha": identity.head, "at": "2026-10-23T01:00:00Z",
        }))
        .unwrap();
        self.ctx
            .store
            .update(id, |task| {
                task.current_run_mut().unwrap().review = Some(review.clone());
                Ok(())
            })
            .unwrap();
    }

    fn amend(&self, tracker: &str, member: &str, revision: u64, tier: &str) {
        let amendment = corral_core::MemberGateAmendment {
            member: member.into(),
            expected_revision: revision - 1,
            gate: ScopeGate {
                tier: tier.into(),
                tests: Vec::new(),
                expected_failures: Vec::new(),
            },
            reason: "tighten".into(),
            by: "owner".into(),
        };
        self.ctx
            .store
            .amend_member_gate(tracker, &amendment, |_, _| Ok(()), |_, _| Ok(None))
            .unwrap();
    }
}

type RunPlanTargetJson = serde_json::Value;

fn quick_binding(source: &str) -> GateTierBinding {
    requirement::binding("quick", &[QUICK.into()], &[], &[], source).unwrap()
}

#[test]
fn satisfaction_is_binding_equality_with_full_dominance_only_where_allowed() {
    let member = quick_binding("plan:T-1@P1#a@digest");
    let review = quick_binding(REVIEW_ROUNDS_SOURCE);
    let member_requirement = GateRequirement {
        binding: Some(member.clone()),
        full_dominates: false,
    };
    let review_requirement = GateRequirement {
        binding: Some(review.clone()),
        full_dominates: true,
    };
    // A member receipt is fresh only with exactly its binding; full never stands in.
    assert!(satisfies(Some(&member), &member_requirement));
    assert!(!satisfies(None, &member_requirement));
    assert!(!satisfies(Some(&review), &member_requirement));
    // A main-bound full receipt satisfies the review-phase requirement.
    assert!(satisfies(Some(&review), &review_requirement));
    assert!(satisfies(None, &review_requirement));
    // A quick PASS never satisfies the main landing tier.
    assert_eq!(MAIN_LANDING_TIER, FULL_TIER);
    assert!(!satisfies(Some(&review), &GateRequirement::full()));
    assert!(!satisfies(Some(&member), &GateRequirement::full()));
    assert!(satisfies(None, &GateRequirement::full()));
}

#[test]
fn bindings_digest_every_selection_input() {
    let base = quick_binding("policy:plan_member");
    assert_eq!(base, quick_binding("policy:plan_member"));
    let failure = ExpectedFailure {
        test: "corral-ops::gate_ops tests::x".into(),
        reason: "restored by b".into(),
    };
    for other in [
        requirement::binding("quick", &["other".into()], &[], &[], "policy:plan_member"),
        requirement::binding(
            "quick",
            &[QUICK.into()],
            &["package(a)".into()],
            &[],
            "policy:plan_member",
        ),
        requirement::binding(
            "quick",
            &[QUICK.into()],
            &[],
            &[failure],
            "policy:plan_member",
        ),
        requirement::binding("quick", &[QUICK.into()], &[], &[], "member-gate:T-1@1"),
        requirement::binding("lint", &[QUICK.into()], &[], &[], "policy:plan_member"),
    ] {
        assert_ne!(other.unwrap().selection_sha256, base.selection_sha256);
    }
}

#[test]
fn selection_by_rules_destination_and_phase() {
    let fx = fixture(true);
    let task = fx.main_bound();
    let run = task.current_run().unwrap().clone();
    // A main-bound tiered run: quick review rounds, full at landing.
    let review = select(&fx.ctx, &task, &run, GatePhase::Review).unwrap();
    assert_eq!(
        (
            review.tier.as_str(),
            review.commands.as_slice(),
            review.source.as_str()
        ),
        ("quick", [QUICK.to_owned()].as_slice(), REVIEW_ROUNDS_SOURCE)
    );
    assert_eq!(
        review.requirement.binding,
        Some(quick_binding(REVIEW_ROUNDS_SOURCE))
    );
    assert!(review.requirement.full_dominates);
    assert_eq!(review.destination.kind, DestinationKind::Main);
    assert_eq!(review.landing_tier, MAIN_LANDING_TIER);
    let landing = select(&fx.ctx, &task, &run, GatePhase::Landing).unwrap();
    assert_eq!(
        (landing.tier.as_str(), landing.commands.as_slice()),
        (FULL_TIER, [FULL.to_owned()].as_slice())
    );
    assert_eq!(landing.requirement, GateRequirement::full());
    // A queue base override keeps full in every phase.
    let other = Run {
        base: "release".into(),
        ..run.clone()
    };
    let other = select(&fx.ctx, &task, &other, GatePhase::Review).unwrap();
    assert_eq!(
        (other.destination.kind, other.tier.as_str()),
        (DestinationKind::Other, FULL_TIER)
    );
    assert_eq!(other.requirement, GateRequirement::full());
    // A legacy run in a tiered project is full with today's commands.
    let legacy = Run {
        gate_rules: GateRules::Legacy,
        ..run.clone()
    };
    let legacy = select(&fx.ctx, &task, &legacy, GatePhase::Review).unwrap();
    assert_eq!(
        (legacy.rules, legacy.tier.as_str()),
        (GateRules::Legacy, FULL_TIER)
    );
    assert_eq!(legacy.commands, [FULL]);
    assert_eq!(legacy.source, "config:review.gate");
}

#[test]
fn untiered_project_selects_full_for_every_run() {
    let fx = fixture(false);
    let task = fx.main_bound();
    let run = task.current_run().unwrap();
    for phase in [GatePhase::Review, GatePhase::Landing] {
        let contract = select(&fx.ctx, &task, run, phase).unwrap();
        assert_eq!(
            (contract.rules, contract.tier.as_str()),
            (GateRules::Legacy, FULL_TIER)
        );
        assert_eq!(contract.commands, [FULL]);
        assert_eq!(contract.binding(), None);
    }
}

#[test]
fn members_bind_their_gate_in_every_phase_and_owner_records_win() {
    let fx = fixture(true);
    let (task, tracker) = fx.member();
    let run = task.current_run().unwrap();
    for phase in [GatePhase::Review, GatePhase::Landing] {
        let contract = select(&fx.ctx, &task, run, phase).unwrap();
        assert_eq!(contract.destination.kind, DestinationKind::PlanTarget);
        assert_eq!(contract.destination.branch, "feature/plan");
        assert_eq!(
            (contract.tier.as_str(), contract.landing_tier.as_str()),
            ("quick", "quick")
        );
        assert_eq!(contract.source, "policy:plan_member");
        assert_eq!(
            contract.requirement.binding,
            Some(quick_binding("policy:plan_member"))
        );
        assert!(!contract.requirement.full_dominates);
    }
    // An owner record for tier full still binds the member, so full never dominates.
    fx.amend(&tracker, &task.id, 1, FULL_TIER);
    let (task, run) = fx.current(&task.id);
    let full = select(&fx.ctx, &task, &run, GatePhase::Review).unwrap();
    assert_eq!(
        (full.tier.as_str(), full.commands.as_slice()),
        (FULL_TIER, [FULL.to_owned()].as_slice())
    );
    let binding = full.requirement.binding.clone().unwrap();
    assert_eq!(binding.source, format!("member-gate:{tracker}@1"));
    assert!(!full.requirement.full_dominates);
}

#[test]
fn main_bound_receipts_fresh_by_phase_with_full_dominance() {
    let fx = fixture(true);
    let id = fx.main_bound().id;
    // Given a quick review-round receipt and an approval bound to it.
    let quick = fx.receipt(&id, GatePhase::Review, "a1");
    assert_eq!(
        quick.identity.as_ref().unwrap().gate_tier.as_deref(),
        Some(&quick_binding(REVIEW_ROUNDS_SOURCE))
    );
    assert!(fx.fresh(&id, &quick));
    fx.review(&id, Verdict::Approve, quick.identity.as_ref().unwrap());
    let (task, run) = fx.current(&id);
    assert!(verification::review_fresh(&fx.ctx, &task, &run).unwrap());
    // Then the landing phase refuses the quick PASS, and the audit still judges it by review.
    assert!(!fx.fresh(&id, &quick));
    assert!(!verification::gate_passed(&fx.ctx, &task, &run).unwrap());
    let review = requirement_for(&fx.ctx, &task, &run, GatePhase::Review).unwrap();
    assert!(verification::gate_fresh_for(&fx.ctx, &task, &run, &quick, &review).unwrap());
    // A full receipt satisfies both phases.
    let full = fx.receipt(&id, GatePhase::Landing, "a2");
    assert_eq!(full.identity.as_ref().unwrap().gate_tier, None);
    assert!(fx.fresh(&id, &full));
    let (task, run) = fx.current(&id);
    assert!(verification::gate_fresh_for(&fx.ctx, &task, &run, &full, &review).unwrap());
    // And the quick and full identities differ exactly in attempt, binding and policy digest.
    let changed: Vec<_> = quick
        .identity
        .as_ref()
        .unwrap()
        .differences(full.identity.as_ref().unwrap())
        .into_iter()
        .map(|change| change.field)
        .collect();
    assert_eq!(changed, ["attempt", "policy_digest", "gate_tier"]);
}

#[test]
fn a_changed_member_selection_stales_its_receipts_of_every_tier() {
    let fx = fixture(true);
    let (task, tracker) = fx.member();
    let id = task.id;
    let quick = fx.receipt(&id, GatePhase::Review, "m1");
    assert!(fx.fresh(&id, &quick));
    fx.review(&id, Verdict::Approve, quick.identity.as_ref().unwrap());
    let (task, run) = fx.current(&id);
    assert!(verification::review_fresh(&fx.ctx, &task, &run).unwrap());
    // When the owner's record changes the member's selection, the quick receipt and its
    // review go stale.
    fx.amend(&tracker, &id, 1, FULL_TIER);
    assert!(!fx.fresh(&id, &quick));
    let (task, run) = fx.current(&id);
    assert!(!verification::review_fresh(&fx.ctx, &task, &run).unwrap());
    // A full member receipt is bound too, and the next change stales it despite its tier.
    let full = fx.receipt(&id, GatePhase::Review, "m2");
    let binding = full.identity.as_ref().unwrap().gate_tier.clone().unwrap();
    assert_eq!(
        (binding.tier.as_str(), binding.source),
        (FULL_TIER, format!("member-gate:{tracker}@1"))
    );
    assert!(fx.fresh(&id, &full));
    fx.amend(&tracker, &id, 2, "quick");
    assert!(!fx.fresh(&id, &full));
}

#[test]
fn receipts_validate_against_their_own_tier_commands() {
    let mut policy = ReviewPolicy {
        gate: vec![FULL.into()],
        ..ReviewPolicy::default()
    };
    policy.gate_tiers.insert("quick".into(), vec![QUICK.into()]);
    let mut gate: GateRecord = serde_json::from_value(json!({
        "result":"pass", "at":"2026-10-23T00:00:00Z", "sha":"h",
    }))
    .unwrap();
    assert_eq!(receipt_commands(&policy, &gate), [FULL]);
    gate.tier = Some(Box::new(corral_core::GateTierReceipt {
        tier: "quick".into(),
        context_sha256: "c".into(),
        carried: None,
        promoted_from: None,
    }));
    assert_eq!(receipt_commands(&policy, &gate), [QUICK]);
    gate.tier.as_mut().unwrap().tier = "removed".into();
    assert!(receipt_commands(&policy, &gate).is_empty());
}

fn identity(attempt: &str, tiered: bool) -> CandidateIdentity {
    serde_json::from_value(json!({
        "version":2, "task":"T-1", "run":"R1", "attempt":attempt, "head":"h", "tree":"t",
        "integration_base":"b", "dispatch_base":"b", "spec_digest":"s", "policy_digest":"p",
        "inputs_digest":"i", "environment":[["PATH","/approved"]],
        "gate_tier": tiered.then(|| quick_binding(REVIEW_ROUNDS_SOURCE)),
    }))
    .unwrap()
}

fn landing_contract() -> GateContract {
    GateContract {
        tier: FULL_TIER.into(),
        commands: vec![FULL.into()],
        scope: Vec::new(),
        tests: Vec::new(),
        expected_failures: Vec::new(),
        destination: GateDestination {
            kind: DestinationKind::Main,
            branch: "main".into(),
        },
        landing_tier: FULL_TIER.into(),
        phase: GatePhase::Landing,
        source: "config:review.gate".into(),
        rules: GateRules::Tiered,
        requirement: GateRequirement::full(),
    }
}

fn origin_run(
    approved: Option<(&CandidateIdentity, Verdict)>,
    receipt: Option<&CandidateIdentity>,
    origin: Option<&GatePromotion>,
) -> Run {
    let review = approved.map(|(identity, verdict)| {
        json!({"identity":identity, "verdict":verdict, "reviewer":"owner", "sha":"h", "at":"2026-10-23T01:00:00Z"})
    });
    let gate = receipt.map(|identity| {
        json!({"identity":identity, "result":"pass", "at":"2026-10-23T00:30:00Z", "sha":"h"})
    });
    serde_json::from_value(json!({
        "id":"R1", "agent":"fixture", "worktree":"/unused", "branch":"corral/T-1", "base":"main",
        "base_sha":"b", "started":"2026-10-23T00:00:00Z", "gate_rules":"tiered",
        "review":review, "gate":gate, "promotion_origin":origin,
    }))
    .unwrap()
}

#[test]
fn promotion_origin_is_recorded_once_kept_while_current_and_cleared_when_inert() {
    let a = identity("a1", true);
    let b = identity("b1", false);
    let contract = landing_contract();
    let origin = GatePromotion {
        identity: a.clone(),
        gate_at: "2026-10-23T00:30:00Z".parse().unwrap(),
    };
    let approve = Some((&a, Verdict::Approve));
    // The first landing gate while the approved quick receipt is current records it.
    assert_eq!(
        origin_step(&origin_run(approve, Some(&a), None), &contract),
        OriginStep::Record(origin.clone())
    );
    // Every later landing gate copies it, whatever replaced the receipt (or cleared it).
    for receipt in [Some(&b), None] {
        let run = origin_run(approve, receipt, Some(&origin));
        assert_eq!(
            origin_step(&run, &contract),
            OriginStep::Copy(origin.clone())
        );
    }
    // Without a current quick receipt nothing is recorded.
    assert_eq!(
        origin_step(&origin_run(approve, Some(&b), None), &contract),
        OriginStep::Keep
    );
    // A changed approval makes the origin inert: a full-bound approval or a changes verdict.
    for review in [
        Some((&b, Verdict::Approve)),
        Some((&a, Verdict::Changes)),
        None,
    ] {
        let run = origin_run(review, Some(&b), Some(&origin));
        assert_eq!(origin_step(&run, &contract), OriginStep::Clear);
    }
    // A new promotable approval whose receipt is current replaces an inert origin.
    let a2 = identity("a2", true);
    let renewed = origin_step(
        &origin_run(Some((&a2, Verdict::Approve)), Some(&a2), Some(&origin)),
        &contract,
    );
    assert!(matches!(renewed, OriginStep::Record(o) if o.identity == a2));
    // Review-phase, legacy, Plan-target and other-base gates never touch it.
    let run = origin_run(approve, Some(&a), Some(&origin));
    for contract in [
        GateContract {
            phase: GatePhase::Review,
            ..landing_contract()
        },
        GateContract {
            rules: GateRules::Legacy,
            ..landing_contract()
        },
        GateContract {
            destination: GateDestination {
                kind: DestinationKind::PlanTarget,
                branch: "p".into(),
            },
            ..landing_contract()
        },
        GateContract {
            destination: GateDestination {
                kind: DestinationKind::Other,
                branch: "r".into(),
            },
            ..landing_contract()
        },
    ] {
        assert_eq!(origin_step(&run, &contract), OriginStep::Keep);
    }
    // Applying a step records or clears only what it names.
    let mut run = origin_run(approve, Some(&a), None);
    OriginStep::Record(origin.clone()).apply(&mut run);
    assert_eq!(run.promotion_origin.as_deref(), Some(&origin));
    OriginStep::Copy(origin.clone()).apply(&mut run);
    OriginStep::Keep.apply(&mut run);
    assert_eq!(run.promotion_origin.as_deref(), Some(&origin));
    OriginStep::Clear.apply(&mut run);
    assert_eq!(run.promotion_origin, None);
}

#[test]
fn scanner_matches_imperative_full_gate_directives_only() {
    let full = ["bash -c 'source scripts/test-herdr.sh && scripts/gate.sh'".to_owned()];
    for (text, line, phrase) in [
        (
            "1. Read.\n2. Run the full gate before done.\n",
            2,
            "Run the full gate",
        ),
        (
            "then RE-RUN THE GATE on the rebased tree",
            1,
            "RE-RUN THE GATE",
        ),
        ("rerun the gate", 1, "rerun the gate"),
        ("Always run scripts/gate.sh first", 1, "scripts/gate.sh"),
        ("run `scripts/gate.sh`", 1, "scripts/gate.sh"),
        (
            "x\ny\nBASH -C 'source scripts/test-herdr.sh && scripts/gate.sh'",
            3,
            "BASH -C 'source scripts/test-herdr.sh && scripts/gate.sh'",
        ),
    ] {
        assert_eq!(
            scan_gate_instructions(text, &full),
            Some((line, phrase.to_owned())),
            "{text}"
        );
    }
    for clean in [
        "corral runs the full gate when this lands on main; do not run it yourself",
        "Run the gate below once after the final rebase: {{gate}}",
        "scripts/gate.sh --tier quick",
        "{{run the full gate}} is only a placeholder",
        "",
    ] {
        assert_eq!(scan_gate_instructions(clean, &full), None, "{clean}");
    }
    // A full command that prefixes an explicitly reduced invocation is not that command.
    for (full, reduced, invocation) in [
        (
            "scripts/gate.sh",
            "scripts/gate.sh --tier quick",
            "run scripts/gate.sh",
        ),
        (
            "bash scripts/gate.sh",
            "bash scripts/gate.sh  --tier quick",
            "bash scripts/gate.sh",
        ),
    ] {
        let full = [full.to_owned()];
        assert_eq!(scan_gate_instructions(reduced, &full), None, "{reduced}");
        let found = scan_gate_instructions(invocation, &full).map(|(line, _)| line);
        assert_eq!(found, Some(1), "{invocation}");
        let later = format!("{reduced} && {}", full[0]);
        assert_eq!(
            scan_gate_instructions(&later, &full),
            Some((1, full[0].clone())),
            "{later}"
        );
    } // Only an explicit non-full tier is reduced: full, a missing value or no separator is not.
    for full in ["scripts/gate.sh", "bash scripts/gate.sh"] {
        let commands = [full.to_owned()];
        for suffix in [
            "--tier full",
            "--tier=full",
            "--tier 'full'",
            "--TIER=FULL",
            "--tier",
            "--tier-quick",
        ] {
            let text = format!("{full} {suffix}");
            let found = scan_gate_instructions(&text, &commands);
            assert_eq!(found, Some((1, full.to_owned())), "{text}");
        }
        for suffix in ["--tier=quick", "--tier \"quick\"", "--tier lint-2"] {
            let text = format!("{full} {suffix}");
            assert_eq!(scan_gate_instructions(&text, &commands), None, "{text}");
        }
    }
}

#[test]
fn automatic_dispatch_refusal_is_reported_once_and_creates_nothing() {
    let fx = fixture(true);
    let task = fx
        .ctx
        .store
        .create("Refused", Some("app"), corral_core::Status::Ready, "")
        .unwrap();
    let mut execution = ExecutionSnapshot::new(
        &fx.ctx.store.config,
        ResolvedProfile {
            profile: "fake".into(),
            harness: "fake".into(),
            command: "true".into(),
            model: None,
            vendor: None,
            effort: None,
            model_args: Vec::new(),
        },
    );
    execution.prompt_template = Some("{{id}}\n2. Run the full gate before done.\n".into());
    // Given an automatic admission that hits a full-gate directive twice.
    for _ in 0..2 {
        let error = instructions::dispatch_rules(&fx.ctx, &task, &execution, true).unwrap_err();
        let conflict = error.downcast::<GateInstructionConflict>().unwrap();
        assert_eq!(
            (
                conflict.surface.as_str(),
                conflict.line,
                conflict.phrase.as_str()
            ),
            ("run_template", 2, "Run the full gate")
        );
        assert_eq!(conflict.path, "built-in assets/run.md");
    }
    // Then automation reports it once, and the task has no run.
    let blocked: Vec<_> = fx
        .ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|event| {
            event.kind == "automation_blocked" && event.task.as_deref() == Some(task.id.as_str())
        })
        .collect();
    assert_eq!(blocked.len(), 1, "{blocked:?}");
    assert_eq!(blocked[0].detail["conflict"]["surface"], "run_template");
    assert!(fx.ctx.store.get(&task.id).unwrap().runs.is_empty());
    // A manual dispatch reports nothing to automation; a clean template is tiered.
    instructions::dispatch_rules(&fx.ctx, &task, &execution, false).unwrap_err();
    execution.prompt_template = Some("{{id}}\nRun the gate below.\n{{gate}}\n".into());
    assert_eq!(
        instructions::dispatch_rules(&fx.ctx, &task, &execution, true).unwrap(),
        GateRules::Tiered
    );
    let untiered = fixture(false);
    assert_eq!(
        instructions::dispatch_rules(&untiered.ctx, &task, &execution, true).unwrap(),
        GateRules::Legacy
    );
    let count = fx
        .ctx
        .store
        .events()
        .unwrap()
        .iter()
        .filter(|e| e.kind == "automation_blocked")
        .count();
    assert_eq!(count, 1);
}

#[test]
fn autopilot_dispatch_reports_a_template_conflict_once_and_creates_no_run() {
    let fx = fixture(true);
    let mut store = fx.ctx.store.clone();
    store.config.harnesses.insert(
        "fake".into(),
        corral_core::Harness {
            command: "true".into(),
            flags: None,
            model_flag: None,
            effort_flag: None,
            efforts: vec![],
            prompt: None,
        },
    );
    store.config.profiles.insert(
        "fake".into(),
        corral_core::Profile {
            harness: "fake".into(),
            model: None,
            vendor: None,
            effort: None,
            args: vec![],
            escalate_to: None,
            metadata: None,
        },
    );
    store.save_config().unwrap();
    std::fs::create_dir_all(fx.ctx.store.prompts_dir()).unwrap();
    std::fs::write(
        fx.ctx.store.prompts_dir().join("run.md"),
        "{{id}}\nRun the full gate before done.\n",
    )
    .unwrap();
    let task = fx
        .ctx
        .store
        .create("Automatic", Some("app"), corral_core::Status::Ready, "")
        .unwrap();
    fx.ctx
        .store
        .update(&task.id, |task| {
            task.profile = Some("fake".into());
            Ok(())
        })
        .unwrap();
    // Given two automatic admissions of a task whose run template hardcodes the full gate.
    for _ in 0..2 {
        let error = crate::dispatch::dispatch_automatic(
            &fx.ctx,
            &task.id,
            json!({"source": "test"}),
            jiff::Timestamp::now(),
        )
        .unwrap_err();
        let conflict = error.downcast_ref::<GateInstructionConflict>().unwrap();
        assert_eq!(
            (conflict.surface.as_str(), conflict.line),
            ("run_template", 2)
        );
    }
    // Then automation reports it once, and no run or worktree exists.
    let events = fx.ctx.store.events().unwrap();
    let blocked = events
        .iter()
        .filter(|e| e.kind == "automation_blocked" && e.task.as_deref() == Some(task.id.as_str()))
        .count();
    assert_eq!(blocked, 1);
    assert!(fx.ctx.store.get(&task.id).unwrap().runs.is_empty());
    assert!(!fx.repo.join(".worktrees").join(&task.id).exists());
    // The same refusal for the first member of an integration Plan claims no target.
    let source = fx.repo.parent().unwrap().join("plan.md");
    std::fs::write(&source, "---\ntitle: Plan\nproject: app\napproved_scope:\n  goal: Combined change\n  acceptance: [{scenario: together, expected: pass}]\n  delivery_target: {kind: integration, branch: feature/plan}\ntasks:\n  - {key: first, title: First, profile: fake}\n  - {key: second, title: Second, profile: fake}\n---\n").unwrap();
    let imported = crate::import_plan(&fx.ctx, &source, false, Origin::Agent).unwrap();
    let member = &imported.tasks[0].id;
    fx.ctx
        .store
        .update(member, |task| {
            task.status = corral_core::Status::Ready;
            Ok(())
        })
        .unwrap();
    let error = crate::dispatch::dispatch_automatic(
        &fx.ctx,
        member,
        json!({"source": "test"}),
        jiff::Timestamp::now(),
    )
    .unwrap_err();
    assert!(
        error.downcast_ref::<GateInstructionConflict>().is_some(),
        "{error:#}"
    );
    let checkout = format!("{}-integration", imported.tracker.id);
    assert!(!fx.repo.join(".worktrees").join(checkout).exists());
    assert!(!fx.repo.join(".git/corral-plan-targets").exists());
    assert!(corral_git::rev_sha(&fx.repo, "refs/heads/feature/plan").is_err());
}
