//! The worker Gate contract: a tiered run's prompt carries exactly one authoritative section,
//! whatever template it is rendered from, and a legacy run keeps today's gate text for life.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use corral_core::{
    ExpectedFailure, GateRules, Harness, MemberGateRecord, Profile, Run, ScopeGate, Status, Store,
    Task, TierPolicy,
};
use corral_herdr::Herdr;
use corral_ops::{Ctx, DispatchOpts, DispatchOutcome};

use super::{corral, git};

const SECTION: &str = "## Gate contract (authoritative)";
const POINTER: &str =
    "The Gate contract (authoritative) section below is the only gate instruction for this run.";
const QUICK: &str = "echo quick-ok";
const FULL: &str = "echo gate-ok";
const REASON: &str = "Fails until T-999 restores the widget cache";

struct Fx {
    _tmp: tempfile::TempDir,
    ctx: Ctx,
}

impl Fx {
    fn new(tiered: bool) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("herdr");
        std::os::unix::fs::symlink(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gate_ops/herdr.sh"),
            &bin,
        )
        .unwrap();
        let app = tmp.path().join("app");
        std::fs::create_dir_all(&app).unwrap();
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
        store.config.review.gate = Some(vec![FULL.into()]);
        if tiered {
            tier(&mut store);
        }
        store.save_config().unwrap();
        std::fs::create_dir_all(store.prompts_dir()).unwrap();
        let ctx = Ctx {
            resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
            store,
            herdr: Herdr::with_bin(bin, None),
        };
        Fx { _tmp: tmp, ctx }
    }

    fn dispatch(&self, id: &str) -> Run {
        let opts = DispatchOpts {
            profile: Some("fake".into()),
            ..DispatchOpts::default()
        };
        match corral_ops::dispatch(&self.ctx, id, &opts).unwrap() {
            DispatchOutcome::Started { task, .. } => task.current_run().unwrap().clone(),
            DispatchOutcome::Queued { reason, .. } => panic!("queued: {reason}"),
        }
    }

    fn delivered(&self, id: &str) -> String {
        std::fs::read_to_string(self.ctx.store.prompts_dir().join(format!("{id}-R1.md"))).unwrap()
    }

    /// The task as `launch` renders it: without the run being prepared.
    fn source(&self, id: &str) -> (Task, Run) {
        let mut task = self.ctx.store.get(id).unwrap();
        let run = task.runs.pop().unwrap();
        (task, run)
    }

    fn render(&self, task: &Task, run: &Run) -> String {
        corral_ops::task_prompt(&self.ctx, task, run).unwrap()
    }
}

fn tier(store: &mut Store) {
    store.config.review.gate_tiers = Some(BTreeMap::from([(
        "quick".to_owned(),
        vec![QUICK.to_owned()],
    )]));
    store.config.review.tier_policy = Some(TierPolicy {
        plan_member: Some("quick".into()),
        review_rounds: Some("quick".into()),
    });
}

/// A two-member integration Plan whose first member has an owner-declared quick gate.
fn member(fx: &Fx) -> String {
    let source = fx.ctx.store.root().join("plan.md");
    std::fs::write(
        &source,
        "---\ntitle: Integration Plan\nproject: app\napproved_scope:\n  goal: Ship A and B\n  acceptance: [{scenario: Combine, expected: Both included}]\n  delivery_target: {kind: integration, branch: feature/plan}\ntasks:\n  - {key: a, title: A, profile: fake}\n  - {key: b, title: B, profile: fake}\n---\n",
    )
    .unwrap();
    let imported =
        corral_ops::import_plan(&fx.ctx, &source, false, corral_core::Origin::Owner).unwrap();
    let id = imported.tasks[0].id.clone();
    let record = MemberGateRecord {
        revision: 1,
        gate: ScopeGate {
            tier: "quick".into(),
            tests: vec!["test(widget)".into()],
            expected_failures: vec![ExpectedFailure {
                test: "corral-ops widget::cache".into(),
                reason: REASON.into(),
            }],
        },
        reason: "declared at decomposition".into(),
        by: "owner".into(),
        at: corral_core::clock::now(),
        history: Vec::new(),
    };
    fx.ctx
        .store
        .update(&imported.tracker.id, |task| {
            task.member_gates.insert(id.clone(), record.clone());
            Ok(())
        })
        .unwrap();
    id
}

fn position(prompt: &str, needle: &str) -> usize {
    prompt
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} missing from:\n{prompt}"))
}

/// Exactly one Gate contract, carrying the whole member contract and no full-tier command.
fn assert_member_contract(prompt: &str, id: &str) {
    let corral = corral();
    assert_eq!(prompt.matches(SECTION).count(), 1, "{prompt}");
    let section = &prompt[position(prompt, SECTION)..];
    let gate = format!("`{corral} gate {id} --wait`");
    for needle in [
        "- Tier: quick, for every review round of this run (source: member-gate:",
        &format!("- Gate: {gate}, exactly once per round, after the final rebase."),
        "autopilot reuses that receipt",
        &format!("      {QUICK}"),
        "      export CORRAL_SIZE_LIMITS=",
        "- Declared tests:\n  - `test(widget)`",
        &format!("- Expected failures:\n  - `corral-ops widget::cache`: {REASON}"),
        "- Module scope:",
        "- Landing: this member lands on its Plan target `feature/plan` on the quick tier",
        "Run no other tier, no `scripts/gate.sh` and none of the commands above by hand",
        "Do not fix a failure outside the module scope, or an expected failure.",
        "`Not verified:`",
        "file a follow-up with the corral-submit skill, and ask in this pane",
    ] {
        assert!(
            section.contains(needle),
            "{needle:?} missing from:\n{prompt}"
        );
    }
    // The audited order: the gate, then evidence for that attempt, then the completion audit.
    let gate_at = position(section, &format!("run {gate}"));
    let evidence = position(section, &format!("{corral} evidence record {id}"));
    let check = position(section, &format!("{corral} done {id} --check"));
    assert!(gate_at < evidence && evidence < check, "{section}");
    assert!(
        !prompt.contains(FULL),
        "a full-tier command leaked:\n{prompt}"
    );
}

#[test]
fn a_tiered_quick_member_prompt_has_one_gate_contract_from_every_template_source() {
    // Given a tiered project and a quick Plan member.
    let fx = Fx::new(true);
    let id = member(&fx);

    // When it is dispatched from the built-in template, which the dispatch scan now accepts.
    let run = fx.dispatch(&id);
    assert_eq!(run.gate_rules, GateRules::Tiered);

    // Then the delivered prompt has one Gate contract and `{{gate}}` points to it.
    let delivered = fx.delivered(&id);
    assert_member_contract(&delivered, &id);
    assert_eq!(delivered.matches(POINTER).count(), 1, "{delivered}");

    // When a project override without {{gate}} renders it.
    let (task, run) = fx.source(&id);
    let mut fresh = run.clone();
    fresh.execution = None;
    std::fs::write(
        fx.ctx.store.prompts_dir().join("run.md"),
        "# {{id}} custom\n\nDo the work, then `{{done_cmd}}`.\n",
    )
    .unwrap();
    let prompt = fx.render(&task, &fresh);
    assert!(prompt.starts_with(&format!("# {id} custom\n")), "{prompt}");
    assert_member_contract(&prompt, &id);
    assert!(!prompt.contains(POINTER), "{prompt}");

    // When a per-harness template with {{gate}} renders it.
    std::fs::write(
        fx.ctx.store.root().join("harness.md"),
        "{{id}} harness\nGate:\n{{gate}}\n",
    )
    .unwrap();
    let mut ctx = Ctx {
        resource_probe: fx.ctx.resource_probe.clone(),
        store: fx.ctx.store.reopen().unwrap(),
        herdr: fx.ctx.herdr.clone(),
    };
    ctx.store.config.harnesses.get_mut("fake").unwrap().prompt = Some("harness.md".into());
    let prompt = corral_ops::task_prompt(&ctx, &task, &fresh).unwrap();
    assert!(
        prompt.starts_with(&format!("{id} harness\nGate:\n{POINTER}\n")),
        "{prompt}"
    );
    assert_member_contract(&prompt, &id);

    // When the run resumes from its frozen snapshot (the built-in template it was dispatched with).
    let prompt = fx.render(&task, &run);
    assert_member_contract(&prompt, &id);
    assert_eq!(prompt.matches(POINTER).count(), 1, "{prompt}");
}

#[test]
fn a_tiered_main_bound_prompt_names_quick_rounds_and_leaves_the_full_gate_to_landing() {
    // Given a tiered project with review_rounds = quick and a main-bound task.
    let fx = Fx::new(true);
    let task = fx
        .ctx
        .store
        .create("Main-bound", Some("app"), Status::Ready, "")
        .unwrap();

    // When it is dispatched from the built-in template.
    let run = fx.dispatch(&task.id);
    assert_eq!(run.gate_rules, GateRules::Tiered);

    // Then its one Gate contract names quick for review rounds and full only for corral at landing.
    let prompt = fx.delivered(&task.id);
    assert_eq!(prompt.matches(SECTION).count(), 1, "{prompt}");
    for needle in [
        "- Tier: quick, for every review round of this run (source: policy:review_rounds)",
        &format!("      {QUICK}"),
        "- Landing: corral itself runs the one full gate when this candidate lands on `main`; do not start it yourself.",
    ] {
        assert!(
            prompt.contains(needle),
            "{needle:?} missing from:\n{prompt}"
        );
    }
    assert!(!prompt.contains(FULL), "{prompt}");
    assert!(
        !prompt.to_ascii_lowercase().contains("run the full gate"),
        "{prompt}"
    );
}

#[test]
fn a_legacy_run_keeps_todays_gate_text_and_its_frozen_prompt_renders_byte_equal() {
    // Given a run dispatched before tiers were configured.
    let fx = Fx::new(false);
    let task = fx
        .ctx
        .store
        .create("Legacy", Some("app"), Status::Ready, "")
        .unwrap();
    let run = fx.dispatch(&task.id);
    assert_eq!(run.gate_rules, GateRules::Legacy);

    // Then the tier-neutral built-in template lists today's gate commands and no Gate contract.
    let delivered = fx.delivered(&task.id);
    assert!(delivered.contains("2. Run the gate below in the foreground before `done`."));
    assert!(delivered.contains(
        "The gate (corral's [review] gate), run from the worktree root:\n\n       export CORRAL_SIZE_LIMITS="
    ));
    assert!(
        delivered.contains(&format!("       {FULL}\n")),
        "{delivered}"
    );
    assert!(!delivered.contains(SECTION) && !delivered.contains(POINTER));

    // When the project becomes tiered and the run resumes from its frozen snapshot.
    let mut store = fx.ctx.store.reopen().unwrap();
    tier(&mut store);
    store.save_config().unwrap();
    let (source, run) = fx.source(&task.id);
    let ctx = Ctx {
        resource_probe: fx.ctx.resource_probe.clone(),
        store,
        herdr: fx.ctx.herdr.clone(),
    };

    // Then it renders byte-equal to the prompt it was delivered.
    assert_eq!(
        corral_ops::task_prompt(&ctx, &source, &run).unwrap(),
        delivered
    );
}

#[test]
fn the_built_in_template_passes_the_dispatch_gate_instruction_scan() {
    let template =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/run.md"))
            .unwrap();
    assert_eq!(
        corral_ops::scan_gate_instructions(&template, &[FULL.to_owned()]),
        None
    );
}
