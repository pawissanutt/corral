//! Gate tiers through the public API. The fake herdr's `pane run` executes a gate synchronously,
//! so its result file exists as soon as `gate_start` returns and no test waits on time.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use corral_core::{
    ArtifactKind, CandidateIdentity, Config, CriteriaManifest, EvidenceObservation, GatePromotion,
    GateRecord, GateResult, GateRules, Harness, Profile, Run, Status, Store, TierPolicy,
};
use corral_herdr::Herdr;
use corral_ops::{Ctx, DispatchOpts, DispatchOutcome, GateInstructionConflict};
use serde_json::{Value, json};
use sha2::Digest;

use super::{fake_herdr, git};

const CLEAN_TEMPLATE: &str =
    "{{id}} {{title}}\n{{body}}\nRun the gate below once after the final rebase.\n{{gate}}\n";
const SUMMARY: &str = "Implemented the scenario.\nVerified: corral gate\nNot verified: none\n";

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", sha2::Sha256::digest(bytes))
}

fn quick_command(out: &Path) -> String {
    let out = out.display();
    format!(
        "echo \"quick:$CORRAL_GATE_TIER\" >> {out}/runs && cp \"$CORRAL_GATE_CONTEXT\" {out}/context.json && env > {out}/quick.env"
    )
}

fn full_command(out: &Path) -> String {
    let out = out.display();
    format!("echo \"full:${{CORRAL_GATE_TIER-unset}}\" >> {out}/runs && test ! -e {out}/full-fails")
}

/// Quick for Plan members and main-bound review rounds.
fn configure_tiers(config: &mut Config, out: &Path) {
    config.review.gate_tiers = Some(BTreeMap::from([(
        "quick".to_owned(),
        vec![quick_command(out)],
    )]));
    config.review.tier_policy = Some(TierPolicy {
        plan_member: Some("quick".into()),
        review_rounds: Some("quick".into()),
    });
}

struct Fx {
    _tmp: tempfile::TempDir,
    ctx: Ctx,
    app: PathBuf,
    /// Outside every worktree: what each gate command ran and saw.
    out: PathBuf,
}

impl Fx {
    fn new(tiered: bool, configure: impl FnOnce(&mut Store)) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let bin = fake_herdr(tmp.path());
        let app = tmp.path().join("app");
        let out = tmp.path().join("out");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        git(&app, &["init", "-q", "-b", "main"]);
        git(&app, &["config", "user.name", "corral test"]);
        git(&app, &["config", "user.email", "corral@test.invalid"]);
        std::fs::write(app.join("README.md"), "hello\n").unwrap();
        std::fs::write(app.join(".gitignore"), ".worktrees/\n").unwrap();
        git(&app, &["add", "-A"]);
        git(&app, &["commit", "-q", "-m", "init"]);
        let mut store = Store::init(tmp.path()).unwrap();
        store.config.harnesses.insert(
            "fake".into(),
            Harness {
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
            Profile {
                harness: "fake".into(),
                model: None,
                vendor: None,
                effort: None,
                args: vec![],
                escalate_to: None,
                metadata: None,
            },
        );
        store.config.review.gate = Some(vec![full_command(&out)]);
        if tiered {
            configure_tiers(&mut store.config, &out);
        }
        configure(&mut store);
        store.save_config().unwrap();
        std::fs::create_dir_all(store.prompts_dir()).unwrap();
        std::fs::write(store.prompts_dir().join("run.md"), CLEAN_TEMPLATE).unwrap();
        let ctx = Ctx {
            resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
            store,
            herdr: Herdr::with_bin(bin, None),
        };
        Fx {
            _tmp: tmp,
            ctx,
            app,
            out,
        }
    }

    fn dispatch(&self, id: &str) -> anyhow::Result<Run> {
        let opts = DispatchOpts {
            profile: Some("fake".into()),
            ..DispatchOpts::default()
        };
        match corral_ops::dispatch(&self.ctx, id, &opts)? {
            DispatchOutcome::Started { task, .. } => Ok(task.current_run().unwrap().clone()),
            DispatchOutcome::Queued { reason, .. } => panic!("queued: {reason}"),
        }
    }

    fn create(&self, title: &str) -> String {
        let task = self
            .ctx
            .store
            .create(title, Some("app"), Status::Ready, "")
            .unwrap();
        task.id
    }

    fn main_bound(&self) -> String {
        let id = self.create("Main-bound");
        self.dispatch(&id).unwrap();
        id
    }

    /// The two members of an imported integration Plan, not yet dispatched.
    fn plan(&self) -> [String; 2] {
        let source = self.ctx.store.root().join("plan.md");
        std::fs::write(
            &source,
            "---\ntitle: Integration Plan\nproject: app\napproved_scope:\n  goal: Ship A and B\n  acceptance: [{scenario: Combine, expected: Both included}]\n  delivery_target: {kind: integration, branch: feature/plan}\ntasks:\n  - {key: a, title: A, profile: fake}\n  - {key: b, title: B, profile: fake}\n---\n",
        )
        .unwrap();
        let imported =
            corral_ops::import_plan(&self.ctx, &source, false, corral_core::Origin::Owner).unwrap();
        std::array::from_fn(|i| imported.tasks[i].id.clone())
    }

    fn run(&self, id: &str) -> Run {
        self.ctx
            .store
            .get(id)
            .unwrap()
            .current_run()
            .unwrap()
            .clone()
    }

    fn update_run(&self, id: &str, edit: impl FnOnce(&mut Run)) {
        self.ctx
            .store
            .update(id, |task| {
                edit(task.current_run_mut().unwrap());
                Ok(())
            })
            .unwrap();
    }

    fn commit(&self, id: &str, file: &str) {
        let worktree = self.run(id).worktree;
        std::fs::write(worktree.join(file), format!("{file}\n")).unwrap();
        git(&worktree, &["add", "-A"]);
        git(&worktree, &["commit", "-q", "-m", file]);
    }

    fn start(&self, id: &str) -> GateRecord {
        corral_ops::gate_start(&self.ctx, id).unwrap()
    }

    fn gate(&self, id: &str) -> GateRecord {
        self.start(id);
        corral_ops::gate_wait(&self.ctx, id).unwrap()
    }

    fn runs(&self) -> Vec<String> {
        std::fs::read_to_string(self.out.join("runs"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn events(&self, id: &str, kind: &str) -> Vec<Value> {
        self.ctx
            .store
            .events()
            .unwrap()
            .into_iter()
            .filter(|e| e.task.as_deref() == Some(id) && e.kind == kind)
            .map(|e| e.detail)
            .collect()
    }

    fn review(&self, id: &str, verdict: &str, identity: &CandidateIdentity) {
        let review = serde_json::from_value(json!({
            "identity": identity, "verdict": verdict, "reviewer": "owner",
            "sha": identity.head, "at": corral_core::clock::now(),
        }))
        .unwrap();
        self.update_run(id, |run| run.review = Some(review));
    }

    fn checkpoint(&self, id: &str) {
        let identity = corral_ops::progress_readback(&self.ctx, id).unwrap();
        corral_ops::save_checkpoint(
            &self.ctx,
            id,
            &corral_core::CheckpointUpdate {
                task: identity.task,
                run: identity.run,
                spec_revision: identity.spec_revision,
                expected_revision: identity
                    .checkpoints
                    .last()
                    .map_or(0, |r| r.checkpoint.revision),
                progress_revision: identity.progress.as_ref().map_or(0, |p| p.revision),
                head: identity.head,
                base: identity.base,
                tree: identity.tree,
                criterion_ids: vec![],
                evidence_refs: vec![],
                next_action: "Review the candidate".into(),
                decisions: vec![],
                waits: vec![],
                resources: vec![],
            },
        )
        .unwrap();
    }

    /// Makes `id` audited with one required criterion before its dispatch.
    fn audited(&self, id: &str) {
        let task = self.ctx.store.get(id).unwrap();
        let manifest: CriteriaManifest = serde_json::from_value(json!({
            "version": 1, "spec_digest": corral_ops::completion_spec_digest(&task).unwrap(),
            "reviewed_by": "owner", "audited": true, "require_gate": true,
            "criteria": [{"id": "C1", "required": true, "live": false,
                "scenario": "the member works", "expected": "pass"}],
            "owner_decisions": [],
        }))
        .unwrap();
        corral_ops::register_criteria(&self.ctx, id, &manifest).unwrap();
    }

    fn observe(&self, id: &str, attempt: &str) {
        let proof = self.out.join(format!("{id}-proof.txt"));
        std::fs::write(&proof, "pass\n").unwrap();
        let artifact = corral_ops::add_artifact(
            &self.ctx,
            id,
            &proof,
            Some(ArtifactKind::Evidence),
            Some("Member scenario proof"),
        )
        .unwrap();
        let observation: EvidenceObservation = serde_json::from_value(json!({
            "version": 1, "key": "k1", "criterion": "C1", "run": self.run(id).id,
            "attempt": attempt, "status": "pass", "action": "run the scenario",
            "environment": "fixture", "expected": "pass", "observed": "pass",
            "artifacts": [artifact.path], "captured": corral_core::clock::now(),
            "producer": "test", "live": false,
        }))
        .unwrap();
        corral_ops::record_evidence(&self.ctx, id, &observation).unwrap();
    }
}

fn assert_legacy(fx: &Fx, id: &str, gate: &GateRecord) {
    assert_eq!(gate.result, GateResult::Pass, "{:?}", gate.failed_cmd);
    assert_eq!(gate.tier, None);
    assert_eq!(gate.identity.as_ref().unwrap().gate_tier, None);
    let commands: Vec<_> = gate.commands.iter().map(|r| r.command.clone()).collect();
    assert_eq!(commands, [full_command(&fx.out)]);
    let started = fx.events(id, "gate_started").pop().unwrap();
    assert!(started.get("tier").is_none(), "{started}");
    let collected = fx.events(id, "gate").pop().unwrap();
    assert!(collected.get("tier").is_none() && collected.get("context_sha256").is_none());
    let contexts = std::fs::read_dir(fx.ctx.store.state_dir().join("gates"))
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .path()
                .to_string_lossy()
                .ends_with(".context.json")
        })
        .count();
    assert_eq!(contexts, 0);
}

#[test]
fn quick_member_gate_runs_exactly_the_quick_commands_with_its_context() {
    let fx = Fx::new(true, |_| {});
    let [member, _] = fx.plan();
    let run = fx.dispatch(&member).unwrap();
    let quick = vec![quick_command(&fx.out)];
    assert_eq!(run.gate_rules, GateRules::Tiered);
    assert_eq!(run.completion.required_checks.as_ref(), Some(&quick));
    fx.commit(&member, "a.txt");
    // When the worker runs `corral gate <id> --wait` after its final rebase.
    let gate = fx.gate(&member);
    // Then exactly the quick commands ran, bound to the member's selection.
    assert_eq!(gate.result, GateResult::Pass, "{:?}", gate.failed_cmd);
    assert_eq!(fx.runs(), ["quick:quick"]);
    let receipts: Vec<_> = gate.commands.iter().map(|r| r.command.clone()).collect();
    assert_eq!(receipts, quick);
    let identity = gate.identity.clone().unwrap();
    let binding = identity.gate_tier.clone().unwrap();
    assert_eq!(
        (binding.tier.as_str(), binding.source.as_str()),
        ("quick", "policy:plan_member")
    );
    // And the context file the command read is the one the receipt names.
    let tier = gate.tier.clone().unwrap();
    assert_eq!((tier.tier.as_str(), &tier.promoted_from), ("quick", &None));
    let bytes = std::fs::read(fx.out.join("context.json")).unwrap();
    assert_eq!(tier.context_sha256, sha256(&bytes));
    let context: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(context["schema_version"], 1);
    assert_eq!(context["tier"], "quick");
    assert_eq!(context["attempt"], identity.attempt);
    assert_eq!(context["commands"], json!(quick));
    assert_eq!(context["tests"], json!([]));
    assert_eq!(context["expected_failures"], json!([]));
    assert_eq!(
        context["destination"],
        json!({"kind": "plan_target", "branch": "feature/plan"})
    );
    assert_eq!(context["integration_base"], identity.integration_base);
    assert!(
        context["changed_paths"]
            .as_array()
            .unwrap()
            .contains(&json!("a.txt"))
    );
    // The exports reach the command but never the captured environment.
    let env = std::fs::read_to_string(fx.out.join("quick.env")).unwrap();
    assert!(env.lines().any(|line| line == "CORRAL_GATE_TIER=quick"));
    assert!(env.lines().any(|line| {
        line.strip_prefix("CORRAL_GATE_CONTEXT=")
            .is_some_and(|path| path.ends_with(&format!("-{}.context.json", identity.attempt)))
    }));
    assert!(
        identity
            .environment
            .iter()
            .all(|(name, _)| name != "CORRAL_GATE_TIER" && name != "CORRAL_GATE_CONTEXT")
    );
    let started = fx.events(&member, "gate_started").pop().unwrap();
    assert_eq!(started["tier"], "quick");
    assert_eq!(started["commands"], json!(quick));
    assert_eq!(
        started["identity"]["gate_tier"]["source"],
        "policy:plan_member"
    );
    let collected = fx.events(&member, "gate").pop().unwrap();
    assert_eq!(collected["tier"], "quick");
    assert_eq!(collected["context_sha256"], tier.context_sha256);
    // Receipts validate against the quick commands when collected again.
    let again = corral_ops::gate_collect(&fx.ctx, &member).unwrap().unwrap();
    assert_eq!(again.result, GateResult::Pass);
    // `corral show --json` reads back the contract and receipt from the same resolver.
    let task = fx.ctx.store.get(&member).unwrap();
    let readback = corral_ops::gate_readback(&fx.ctx, &task).unwrap();
    let contract = readback.gate_contract.unwrap();
    assert_eq!(
        (contract.tier.as_str(), contract.source.as_str()),
        ("quick", "policy:plan_member")
    );
    assert_eq!(contract.commands, quick);
    assert_eq!(contract.landing_tier, "quick");
    let gate_ref = readback.gate_ref.unwrap();
    assert_eq!(
        (gate_ref.attempt.as_str(), gate_ref.tier.as_str()),
        (identity.attempt.as_str(), "quick")
    );
    assert_eq!(
        gate_ref.receipt_sha256,
        sha256(&serde_json::to_vec(&identity).unwrap())
    );
}

#[test]
fn audited_quick_member_passes_done_on_its_worker_gate_and_fails_without_it() {
    let fx = Fx::new(true, |_| {});
    let [gated, ungated] = fx.plan();
    let quick = vec![quick_command(&fx.out)];
    for (id, with_gate) in [(&gated, true), (&ungated, false)] {
        fx.audited(id);
        fx.dispatch(id).unwrap();
        fx.commit(id, "work.txt");
        if with_gate {
            // Given the worker's one receipted gate, then evidence for the attempt it printed.
            let gate = fx.gate(id);
            assert_eq!(gate.result, GateResult::Pass, "{:?}", gate.failed_cmd);
            fx.observe(id, &gate.identity.unwrap().attempt);
        }
        fx.checkpoint(id);
        let audit = corral_ops::completion_check(&fx.ctx, id, SUMMARY).unwrap();
        // Then the audit requires the quick commands, never the full one.
        assert_eq!(audit.required_checks, quick);
        if with_gate {
            assert!(audit.passed, "{:?}", audit.failures);
            corral_ops::done(&fx.ctx, id, SUMMARY).unwrap();
            assert_eq!(fx.ctx.store.get(id).unwrap().status, Status::Review);
        } else {
            assert!(audit.failures.contains(&"gate_missing".to_owned()));
            let error = corral_ops::done(&fx.ctx, id, SUMMARY).unwrap_err();
            assert!(format!("{error:#}").contains("gate_missing"), "{error:#}");
        }
    }
    assert_eq!(fx.runs(), ["quick:quick"]);
}

fn assert_refused(fx: &Fx, id: &str, surface: &str, path: &str, line: usize, phrase: &str) {
    let error = fx.dispatch(id).unwrap_err();
    let conflict = error
        .downcast_ref::<GateInstructionConflict>()
        .unwrap_or_else(|| panic!("not a gate instruction conflict: {error:#}"));
    assert_eq!(
        (
            conflict.surface.as_str(),
            conflict.line,
            conflict.phrase.as_str()
        ),
        (surface, line, phrase)
    );
    assert!(conflict.path.ends_with(path), "{}", conflict.path);
    // `corral dispatch` prints the error with every field.
    let printed = format!("{error:#}");
    for part in [surface, path, phrase, &format!("line {line}")] {
        assert!(printed.contains(part), "{printed}");
    }
    let task = fx.ctx.store.get(id).unwrap();
    assert!(task.runs.is_empty() && task.launch.is_none());
    assert!(!fx.app.join(".worktrees").join(id).exists());
    assert!(fx.events(id, "dispatch_claimed").is_empty());
}

/// The first member's dispatch would initialize the Plan target: its ownership receipt,
/// integration ref and integration checkout.
fn assert_target_unclaimed(fx: &Fx, member: &str) {
    let tracker = fx.ctx.store.get(member).unwrap().parent.unwrap();
    let checkout = fx
        .app
        .join(".worktrees")
        .join(format!("{tracker}-integration"));
    assert!(!checkout.exists(), "{}", checkout.display());
    assert!(!fx.app.join(".git/corral-plan-targets").exists());
    let reference = super::support::command("git", &fx.app)
        .args([
            "rev-parse",
            "--verify",
            "--quiet",
            "refs/heads/feature/plan",
        ])
        .output()
        .unwrap();
    assert!(!reference.status.success(), "{reference:?}");
}

#[test]
fn refused_first_member_dispatch_leaves_the_plan_target_unclaimed() {
    let instructions = |store: &mut Store| {
        store.config.profiles.get_mut("fake").unwrap().metadata = Some(
            serde_json::from_value(json!({
                "display_name":"Worker", "purpose":"Implementation",
                "guidance":{"use_when":["Implement bounded work"]},
                "instructions":"Always run scripts/gate.sh before done.",
                "eligibility":{"enabled":true,"automatic":true,"roles":["implementation"]},
                "revision":"r1","evidence":"e1"
            }))
            .unwrap(),
        );
    };
    let harness = |store: &mut Store| {
        store.config.harnesses.get_mut("fake").unwrap().prompt = Some("harness.md".into());
    };
    for (surface, configure) in [
        ("run_template", (|_: &mut Store| {}) as fn(&mut Store)),
        ("harness_template", harness),
        ("worker_instructions", instructions),
    ] {
        // Given the first member of an integration Plan whose target is not yet claimed.
        let fx = Fx::new(true, configure);
        if surface == "run_template" {
            std::fs::write(
                fx.ctx.store.prompts_dir().join("run.md"),
                "{{id}}\nRun the full gate before done.\n",
            )
            .unwrap();
        }
        std::fs::write(
            fx.ctx.store.root().join("harness.md"),
            "{{id}}\nThen re-run the gate.\n",
        )
        .unwrap();
        let [member, _] = fx.plan();
        // When its dispatch is refused for the selected surface.
        let error = fx.dispatch(&member).unwrap_err();
        let conflict = error.downcast_ref::<GateInstructionConflict>().unwrap();
        assert_eq!(conflict.surface, surface, "{error:#}");
        // Then neither the member nor its Plan target has any state.
        let task = fx.ctx.store.get(&member).unwrap();
        assert!(task.runs.is_empty() && task.launch.is_none());
        assert!(!fx.app.join(".worktrees").join(&member).exists());
        assert_target_unclaimed(&fx, &member);
    }
}

#[test]
fn tiered_dispatch_refuses_full_gate_directives_before_creating_anything() {
    // A run template override.
    let fx = Fx::new(true, |_| {});
    std::fs::write(
        fx.ctx.store.prompts_dir().join("run.md"),
        "{{id}}\n2. Run the full gate before `done`.\n",
    )
    .unwrap();
    let id = fx.create("Run template");
    assert_refused(
        &fx,
        &id,
        "run_template",
        "prompts/run.md",
        2,
        "Run the full gate",
    );
    // A per-harness template.
    let fx = Fx::new(true, |store| {
        store.config.harnesses.get_mut("fake").unwrap().prompt = Some("harness.md".into());
    });
    std::fs::write(
        fx.ctx.store.root().join("harness.md"),
        "{{id}}\n{{gate}}\nThen re-run the gate on the rebased tree.\n",
    )
    .unwrap();
    let id = fx.create("Harness template");
    assert_refused(
        &fx,
        &id,
        "harness_template",
        "harness.md",
        3,
        "re-run the gate",
    );
    // The profile's configured working instructions.
    let fx = Fx::new(true, |store| {
        store.config.profiles.get_mut("fake").unwrap().metadata = Some(
            serde_json::from_value(json!({
                "display_name":"Worker", "purpose":"Implementation",
                "guidance":{"use_when":["Implement bounded work"]},
                "instructions":"Follow the repository.\nAlways run scripts/gate.sh before done.",
                "eligibility":{"enabled":true,"automatic":true,"roles":["implementation"]},
                "revision":"r1","evidence":"e1"
            }))
            .unwrap(),
        );
    });
    let id = fx.create("Worker instructions");
    assert_refused(
        &fx,
        &id,
        "worker_instructions",
        "profiles.fake.metadata.instructions",
        2,
        "scripts/gate.sh",
    );
    // A clean template dispatches a tiered run; an untiered project ignores the scan.
    let fx = Fx::new(true, |_| {});
    let run = fx.dispatch(&fx.create("Clean")).unwrap();
    assert_eq!(run.gate_rules, GateRules::Tiered);
    let fx = Fx::new(false, |_| {});
    std::fs::write(
        fx.ctx.store.prompts_dir().join("run.md"),
        "{{id}}\nRun the full gate.\n",
    )
    .unwrap();
    let run = fx.dispatch(&fx.create("Untiered")).unwrap();
    assert_eq!(run.gate_rules, GateRules::Legacy);
}

#[test]
fn explicit_full_tier_invocations_are_refused_on_every_surface() {
    let directive = "Then run bash scripts/gate.sh --tier=full before done.";
    let instructions = |store: &mut Store| {
        store.config.review.gate = Some(vec!["bash scripts/gate.sh".into()]);
        store.config.profiles.get_mut("fake").unwrap().metadata = Some(serde_json::from_value(json!({
            "display_name":"Worker", "purpose":"Implementation",
            "guidance":{"use_when":["Implement bounded work"]},
            "instructions":"Follow the repository.\nThen run bash scripts/gate.sh --tier=full before done.",
            "eligibility":{"enabled":true,"automatic":true,"roles":["implementation"]},
            "revision":"r1","evidence":"e1"
        })).unwrap());
    };
    let harness = |store: &mut Store| {
        store.config.review.gate = Some(vec!["bash scripts/gate.sh".into()]);
        store.config.harnesses.get_mut("fake").unwrap().prompt = Some("harness.md".into());
    };
    let run_template = |store: &mut Store| {
        store.config.review.gate = Some(vec!["bash scripts/gate.sh".into()]);
    };
    for (surface, path, configure) in [
        (
            "run_template",
            "prompts/run.md",
            run_template as fn(&mut Store),
        ),
        ("harness_template", "harness.md", harness),
        (
            "worker_instructions",
            "profiles.fake.metadata.instructions",
            instructions,
        ),
    ] {
        // Given a surface that asks for the reserved full tier explicitly.
        let fx = Fx::new(true, configure);
        let template = format!("{{{{id}}}}\n{directive}\n");
        if surface == "run_template" {
            std::fs::write(fx.ctx.store.prompts_dir().join("run.md"), &template).unwrap();
        }
        std::fs::write(fx.ctx.store.root().join("harness.md"), &template).unwrap();
        let id = fx.create(surface);
        // Then dispatch refuses it before creating any state.
        assert_refused(&fx, &id, surface, path, 2, "bash scripts/gate.sh");
    }
    // An explicit quick invocation stays clean and dispatches a tiered run.
    let fx = Fx::new(true, run_template);
    std::fs::write(
        fx.ctx.store.prompts_dir().join("run.md"),
        "{{id}}\nThe quick tier is bash scripts/gate.sh --tier quick.\n",
    )
    .unwrap();
    let run = fx.dispatch(&fx.create("Quick")).unwrap();
    assert_eq!(run.gate_rules, GateRules::Tiered);
}

#[test]
fn untiered_and_pre_tier_runs_stay_legacy_with_todays_gate() {
    let fx = Fx::new(false, |_| {});
    let id = fx.main_bound();
    let full = vec![full_command(&fx.out)];
    let run = fx.run(&id);
    assert_eq!(run.gate_rules, GateRules::Legacy);
    assert_eq!(run.completion.required_checks.as_ref(), Some(&full));
    fx.commit(&id, "a.txt");
    let gate = fx.gate(&id);
    assert_legacy(&fx, &id, &gate);
    let readback = corral_ops::gate_readback(&fx.ctx, &fx.ctx.store.get(&id).unwrap()).unwrap();
    let contract = readback.gate_contract.unwrap();
    assert_eq!(
        (contract.tier.as_str(), contract.rules),
        ("full", GateRules::Legacy)
    );
    assert_eq!(readback.gate_ref.unwrap().tier, "full");
    // When tiers are configured while the run is in flight.
    let mut store = fx.ctx.store.clone();
    configure_tiers(&mut store.config, &fx.out);
    store.save_config().unwrap();
    fx.commit(&id, "b.txt");
    let gate = fx.gate(&id);
    // Then it keeps full-gate rules with today's commands, receipts and required checks.
    assert_legacy(&fx, &id, &gate);
    assert_eq!(fx.runs(), ["full:unset", "full:unset"]);
    let readback = corral_ops::gate_readback(&fx.ctx, &fx.ctx.store.get(&id).unwrap()).unwrap();
    assert_eq!(readback.gate_contract.unwrap().rules, GateRules::Legacy);
    let audit = corral_ops::completion_check(&fx.ctx, &id, SUMMARY).unwrap();
    assert!(
        !audit.failures.contains(&"gate_stale".to_owned()),
        "{:?}",
        audit.failures
    );
    assert_eq!(fx.run(&id).completion.required_checks.as_ref(), Some(&full));
}

/// A main-bound tiered run approved on quick receipt A. A was captured in a worker shell whose
/// environment the landing caller lacks, so a landing gate's environment shows its source.
fn approved_on_quick(fx: &Fx) -> (String, GatePromotion) {
    let id = fx.main_bound();
    fx.commit(&id, "a.txt");
    let a = fx.gate(&id);
    assert_eq!(a.result, GateResult::Pass, "{:?}", a.failed_cmd);
    let mut identity = a.identity.clone().unwrap();
    assert_eq!(
        identity.gate_tier.as_ref().map(|b| b.source.as_str()),
        Some("policy:review_rounds")
    );
    let session = identity
        .environment
        .iter_mut()
        .find(|(name, _)| name == "CORRAL_HERDR_SESSION")
        .unwrap();
    session.1 = Some("approved-worker-shell".into());
    fx.update_run(&id, |run| {
        run.gate.as_mut().unwrap().identity = Some(identity.clone());
    });
    fx.review(&id, "approve", &identity);
    (
        id,
        GatePromotion {
            identity,
            gate_at: a.at,
        },
    )
}

/// A landing gate: full, captured with the approved environment, copying the current origin.
fn assert_promotable(fx: &Fx, id: &str, gate: &GateRecord, origin: &GatePromotion) {
    assert_eq!(fx.run(id).promotion_origin.as_deref(), Some(origin));
    let identity = gate.identity.as_ref().unwrap();
    assert_eq!(identity.gate_tier, None);
    assert_eq!(identity.environment, origin.identity.environment);
    let tier = gate.tier.as_ref().unwrap();
    assert_eq!(tier.tier, "full");
    assert_eq!(tier.promoted_from.as_ref(), Some(origin));
    assert_eq!(fx.events(id, "gate_started").pop().unwrap()["tier"], "full");
}

fn result_file(fx: &Fx, id: &str) -> PathBuf {
    corral_ops::gate_paths(&fx.ctx, id, &fx.run(id).id).result
}

#[test]
fn landing_gate_records_the_promotion_origin_once_and_every_replacement_keeps_it() {
    let fx = Fx::new(true, |_| {});
    let (id, origin) = approved_on_quick(&fx);
    // When landing gate B starts, its update records the origin.
    let b = fx.start(&id);
    assert_promotable(&fx, &id, &b, &origin);
    // B goes stale: main moves and corral replays the candidate.
    std::fs::write(fx.app.join("main.txt"), "moved\n").unwrap();
    git(&fx.app, &["add", "-A"]);
    git(&fx.app, &["commit", "-q", "-m", "main moved"]);
    git(&fx.run(&id).worktree, &["rebase", "-q", "main"]);
    let stale = corral_ops::gate_collect(&fx.ctx, &id).unwrap().unwrap();
    assert!(stale.failed_cmd.unwrap().starts_with("stale_result"));
    assert_eq!(fx.run(&id).promotion_origin.as_deref(), Some(&origin));
    let c = fx.start(&id);
    assert_promotable(&fx, &id, &c, &origin);
    // C's pane is lost on the same head before it wrote a result.
    std::fs::remove_file(result_file(&fx, &id)).unwrap();
    fx.update_run(&id, |run| {
        let gate = run.gate.as_mut().unwrap();
        gate.pane = None;
        gate.process_lost = true;
    });
    let d = fx.start(&id);
    assert_promotable(&fx, &id, &d, &origin);
    // Corral restarts after D finished.
    assert_eq!(
        corral_ops::gate_collect(&fx.ctx, &id)
            .unwrap()
            .unwrap()
            .result,
        GateResult::Pass
    );
    let restarted = Fx {
        ctx: Ctx {
            store: Store::open(fx.ctx.store.root()).unwrap(),
            ..fx.ctx.clone()
        },
        ..fx
    };
    let e = restarted.start(&id);
    assert_promotable(&restarted, &id, &e, &origin);
    // A retained launch preparation cleared run.gate and the owner disposed of it.
    corral_ops::gate_collect(&restarted.ctx, &id).unwrap();
    restarted.update_run(&id, |run| run.gate = None);
    let f = restarted.start(&id);
    assert_promotable(&restarted, &id, &f, &origin);
    assert_eq!(
        restarted.runs(),
        [
            "quick:quick",
            "full:full",
            "full:full",
            "full:full",
            "full:full",
            "full:full"
        ]
    );
}

#[test]
fn approval_bound_to_a_full_receipt_makes_the_origin_inert_and_the_next_landing_gate_clears_it() {
    let fx = Fx::new(true, |_| {});
    let (id, origin) = approved_on_quick(&fx);
    let b = fx.gate(&id);
    assert_promotable(&fx, &id, &b, &origin);
    // When a new review approves the full receipt itself.
    fx.review(&id, "approve", b.identity.as_ref().unwrap());
    let c = fx.start(&id);
    // Then no gate copies the origin, it is cleared, and C captures the caller's environment.
    assert_eq!(c.tier.as_ref().unwrap().promoted_from, None);
    assert_eq!(fx.run(&id).promotion_origin, None);
    assert_ne!(c.identity.unwrap().environment, origin.identity.environment);
}

#[test]
fn changes_verdict_leaves_the_origin_inert_until_a_new_promotable_approval_records_one() {
    let fx = Fx::new(true, |_| {});
    let (id, origin) = approved_on_quick(&fx);
    let b = fx.gate(&id);
    assert_promotable(&fx, &id, &b, &origin);
    // When the reviewer asks for changes, the next gate is a quick review round.
    fx.review(&id, "changes", b.identity.as_ref().unwrap());
    fx.commit(&id, "fix.txt");
    let q = fx.gate(&id);
    assert_eq!(q.tier.as_ref().unwrap().tier, "quick");
    assert_eq!(q.tier.as_ref().unwrap().promoted_from, None);
    // Then a new approval bound to that current quick receipt records a new origin.
    let q_identity = q.identity.clone().unwrap();
    fx.review(&id, "approve", &q_identity);
    let c = fx.start(&id);
    let renewed = GatePromotion {
        identity: q_identity,
        gate_at: q.at,
    };
    assert_promotable(&fx, &id, &c, &renewed);
}

#[test]
fn full_command_failure_clears_the_origin_but_invalid_receipts_keep_it() {
    let fx = Fx::new(true, |_| {});
    let (id, origin) = approved_on_quick(&fx);
    // An invalid receipt is not a test result: the origin stays.
    let b = fx.start(&id);
    assert_promotable(&fx, &id, &b, &origin);
    let log = corral_ops::gate_paths(&fx.ctx, &id, &fx.run(&id).id).log;
    let mut bytes = std::fs::read(&log).unwrap();
    bytes.extend_from_slice(b"tampered\n");
    std::fs::write(&log, bytes).unwrap();
    let invalid = corral_ops::gate_collect(&fx.ctx, &id).unwrap().unwrap();
    assert_eq!(
        invalid.failed_cmd.as_deref(),
        Some("invalid_command_receipts")
    );
    assert_eq!(fx.run(&id).promotion_origin.as_deref(), Some(&origin));
    // A full landing gate whose command fails clears it in its collection update.
    std::fs::write(fx.out.join("full-fails"), "").unwrap();
    let failed = fx.gate(&id);
    assert_eq!(
        failed.failed_cmd.as_deref(),
        Some(full_command(&fx.out).as_str())
    );
    assert_eq!(fx.run(&id).promotion_origin, None);
    // A later full PASS is never promoted from the quick approval.
    std::fs::remove_file(fx.out.join("full-fails")).unwrap();
    let pass = fx.gate(&id);
    assert_eq!(pass.result, GateResult::Pass);
    assert_eq!(pass.tier.as_ref().unwrap().promoted_from, None);
    assert_eq!(fx.run(&id).promotion_origin, None);
}
