//! PRF-09 availability fallback against a fake herdr, real git, OmO-shaped session logs and an
//! injected sync clock: no sleeps, live providers or paid calls. Log timestamps are anchored to
//! each run's recorded start, and every transition is driven by an explicit `sync_at` instant.
#![cfg(unix)] // Fixtures execute the symlinked POSIX fake engine.

#[path = "availability_ops/plan_review.rs"]
mod plan_review;
#[path = "availability_ops/recovery.rs"]
mod recovery;
#[path = "availability_ops/review.rs"]
mod review;
mod support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use corral_core::{
    AgentCmd, AvailabilityFallback, CandidateCost, ExecutionCandidate, Harness, Profile,
    ProfileChoice, ProfileEligibility, ProfileMetadata, ProfileRole, Run, RunOutcome,
    SelectionGuidance, Status, Store, Task,
};
use corral_herdr::Herdr;
use corral_ops::{Ctx, DispatchOpts, DispatchOutcome};
use jiff::{SignedDuration, Timestamp};
use serde_json::Value;

fn git(dir: &Path, args: &[&str]) {
    let out = support::command("git", dir)
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn candidate(id: &str, provider: &str, connection: &str, effort: &str) -> ExecutionCandidate {
    ExecutionCandidate {
        id: id.into(),
        harness: "omo".into(),
        connection: connection.into(),
        provider: provider.into(),
        model: format!("{provider}/{id}-fixture"),
        effort: Some(effort.into()),
        args: vec![],
        evidence: "fixture-evidence".into(),
        cost: CandidateCost::Subscription,
    }
}

fn sol() -> ExecutionCandidate {
    candidate("sol", "openai", "owner-openai", "high")
}

fn sol_mini() -> ExecutionCandidate {
    candidate("sol-mini", "openai", "owner-openai", "high")
}

fn opus() -> ExecutionCandidate {
    candidate("opus", "anthropic", "owner-anthropic", "low")
}

fn profile(candidates: Vec<ExecutionCandidate>, role: ProfileRole) -> Profile {
    let preferred = candidates[0].clone();
    let providers: BTreeSet<String> = candidates.iter().map(|c| c.provider.clone()).collect();
    let switches = u32::try_from(candidates.len() - 1).unwrap();
    Profile {
        harness: preferred.harness.clone(),
        model: Some(preferred.model.clone()),
        vendor: Some(preferred.provider.clone()),
        effort: preferred.effort.clone(),
        args: vec![],
        escalate_to: None,
        metadata: Some(ProfileMetadata {
            display_name: "Fixture profile".into(),
            purpose: "Fixture purpose kept across fallback".into(),
            guidance: SelectionGuidance::default(),
            instructions: "Fixture instructions".into(),
            eligibility: ProfileEligibility {
                enabled: true,
                automatic: true,
                roles: BTreeSet::from([role]),
                projects: BTreeSet::new(),
            },
            revision: "fixture-r1".into(),
            evidence: Some("fixture-evidence".into()),
            candidates,
            fallback: (switches > 0).then(|| AvailabilityFallback {
                automatic: true,
                approved_providers: providers,
                max_switches: switches,
                allow_unknown_subscription_cost: true,
                spending: None,
                approval: "fixture-approval".into(),
            }),
        }),
    }
}

pub(crate) struct Fx {
    _tmp: tempfile::TempDir,
    pub ctx: Ctx,
    pub bin_dir: PathBuf,
    agent_dir: PathBuf,
    pub id: String,
    t0: Timestamp,
}

pub(crate) struct Setup {
    pub general: Vec<ExecutionCandidate>,
    pub enabled: Vec<String>,
    pub dispatch: DispatchOpts,
    pub authorize: bool,
    /// False leaves the task Ready, for plan-first fixtures.
    pub dispatched: bool,
    pub configure: Box<dyn FnOnce(&mut Store)>,
}

impl Default for Setup {
    fn default() -> Self {
        Setup {
            general: vec![sol(), opus()],
            enabled: Vec::new(),
            dispatch: DispatchOpts {
                profile: Some("general".into()),
                ..DispatchOpts::default()
            },
            authorize: true,
            dispatched: true,
            configure: Box::new(|_| {}),
        }
    }
}

impl Fx {
    pub fn new(setup: Setup) -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("fake-herdr");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::os::unix::fs::symlink(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/autopilot_ops/herdr.sh"),
            bin_dir.join("herdr"),
        )
        .unwrap();
        let app = tmp.path().join("app");
        std::fs::create_dir_all(&app).unwrap();
        git(&app, &["init", "-q", "-b", "main"]);
        git(&app, &["config", "user.name", "corral test"]);
        git(&app, &["config", "user.email", "corral@test.invalid"]);
        std::fs::write(app.join("README.md"), "hello\n").unwrap();
        git(&app, &["add", "-A"]);
        git(&app, &["commit", "-q", "-m", "init"]);
        let agent_dir = tmp.path().join("omo-agent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        let mut enabled = setup.enabled;
        if enabled.is_empty() {
            enabled = setup.general.iter().map(|c| c.model.clone()).collect();
        }
        std::fs::write(
            agent_dir.join("settings.json"),
            serde_json::json!({ "enabledModels": enabled }).to_string(),
        )
        .unwrap();
        let mut store = Store::init(tmp.path()).unwrap();
        store.config.harnesses.clear();
        store.config.agents.clear();
        store
            .config
            .profiles
            .retain(|_, profile| profile.harness == "omo");
        store.config.harnesses.insert(
            "omo".into(),
            Harness {
                command: "true {model_args}".into(),
                flags: Some("omo".into()),
                model_flag: None,
                effort_flag: None,
                efforts: vec![],
                prompt: None,
            },
        );
        store.config.agents.insert(
            "rev".into(),
            AgentCmd {
                command: "true".into(),
            },
        );
        store.config.sync.omo_agent_dir = Some(agent_dir.clone());
        store.config.profiles.insert(
            "general".into(),
            profile(setup.general, ProfileRole::Implementation),
        );
        (setup.configure)(&mut store);
        store.save_config().unwrap();
        let mut ctx = Ctx {
            resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
            store,
            herdr: Herdr::with_bin(bin_dir.join("herdr"), None),
        };
        let task = ctx
            .store
            .create(
                "Ship the widget",
                Some("app"),
                Status::Ready,
                "## Goal\nShip it.\n",
            )
            .unwrap();
        if !setup.dispatched {
            return Fx {
                _tmp: tmp,
                ctx,
                bin_dir,
                agent_dir,
                id: task.id,
                t0: Timestamp::now(),
            };
        }
        let DispatchOutcome::Started { .. } =
            corral_ops::dispatch(&ctx, &task.id, &setup.dispatch).unwrap()
        else {
            panic!("expected Started");
        };
        if setup.authorize {
            let preview = ctx
                .store
                .preview_operating_policy(&corral_core::OperatingSelection {
                    tasks: [task.id.clone()].into(),
                    authority: corral_core::OperatingAuthority {
                        dispatch: true,
                        accept: true,
                        plan_accept: true,
                        aggregate_accept: false,
                    },
                    ..Default::default()
                })
                .unwrap();
            ctx.store.config = ctx
                .store
                .confirm_operating_policy(&preview, "fixture owner", Timestamp::now())
                .unwrap();
        }
        let t0 = ctx
            .store
            .get(&task.id)
            .unwrap()
            .current_run()
            .unwrap()
            .started;
        Fx {
            _tmp: tmp,
            ctx,
            bin_dir,
            agent_dir,
            id: task.id,
            t0,
        }
    }

    pub fn task(&self) -> Task {
        self.ctx.store.get(&self.id).unwrap()
    }

    pub fn run(&self) -> Run {
        self.task().current_run().unwrap().clone()
    }

    pub fn at(&self, secs: i64) -> Timestamp {
        self.t0 + SignedDuration::from_secs(secs)
    }

    pub fn sync(&self, secs: i64) -> Task {
        corral_ops::sync_at(&self.ctx, self.at(secs)).unwrap();
        self.task()
    }

    pub fn events(&self, kind: &str) -> Vec<Value> {
        self.ctx
            .store
            .events()
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == kind)
            .map(|e| e.detail)
            .collect()
    }

    pub fn prompts(&self) -> usize {
        std::fs::read_to_string(self.bin_dir.join("prompts.log"))
            .unwrap_or_default()
            .lines()
            .count()
    }

    pub fn launched(&self) -> String {
        std::fs::read_to_string(self.bin_dir.join("runs.log")).unwrap_or_default()
    }

    pub fn write_state(&self, directory: &str, key: &str, state: &Value) {
        let dir = self.ctx.store.state_dir().join(directory);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{key}.json")),
            serde_json::to_vec_pretty(state).unwrap(),
        )
        .unwrap();
    }

    pub fn pane_status(&self, status: &str) {
        std::fs::write(self.bin_dir.join("status"), status).unwrap();
    }

    /// A harness session owned by `run`, created after it started, whose last turn failed.
    pub fn fail_turn(&self, run: &Run, file: &str, message: &str, after_secs: i64) {
        let header = run.started + SignedDuration::from_secs(1);
        let failed = run.started + SignedDuration::from_secs(after_secs);
        let lines = [
            serde_json::json!({"type":"session","version":3,"id":file,
                "timestamp":header.to_string(),"cwd":run.worktree}),
            serde_json::json!({"type":"message","id":"u1","timestamp":header.to_string(),
                "message":{"role":"user","content":[{"type":"text","text":"Work"}]}}),
            serde_json::json!({"type":"message","id":"a1","timestamp":failed.to_string(),
                "message":{"role":"assistant","content":[],"stopReason":"error",
                "errorMessage":message}}),
        ];
        let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
        std::fs::create_dir_all(self.sessions(run)).unwrap();
        std::fs::write(self.sessions(run).join(file), text).unwrap();
    }

    /// The same session answers again after the failed turn.
    pub fn answer(&self, run: &Run, file: &str, after_secs: i64) {
        let at = run.started + SignedDuration::from_secs(after_secs);
        let line = serde_json::json!({"type":"message","id":"a2","timestamp":at.to_string(),
            "message":{"role":"assistant","content":[{"type":"text","text":"Back"}],
            "stopReason":"stop"}});
        let path = self.sessions(run).join(file);
        let text = std::fs::read_to_string(&path).unwrap() + &format!("{line}\n");
        std::fs::write(path, text).unwrap();
    }

    fn sessions(&self, run: &Run) -> PathBuf {
        let body = run
            .worktree
            .to_string_lossy()
            .trim_start_matches('/')
            .replace(['/', '\\', ':'], "-");
        self.agent_dir.join("sessions").join(format!("--{body}--"))
    }

    /// The portable handoff a worker records before a provider fails.
    pub fn checkpoint(&self) {
        let identity = corral_ops::progress_readback(&self.ctx, &self.id).unwrap();
        corral_ops::save_checkpoint(
            &self.ctx,
            &self.id,
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
                next_action: "Continue the widget".into(),
                decisions: vec![],
                waits: vec![],
                resources: vec![],
            },
        )
        .unwrap();
    }

    pub fn episode(&self, id: &str) -> Value {
        let path = self
            .ctx
            .store
            .state_dir()
            .join("availability")
            .join(format!("{id}.json"));
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }
}

fn binding(run: &Run) -> corral_core::CandidateBinding {
    run.execution
        .as_ref()
        .and_then(|e| e.candidate.clone())
        .expect("candidate binding")
}

const MODEL_QUOTA: &str = "quota exceeded for model sol (insufficient_quota)";

#[test]
fn sol_like_quota_continues_on_opus_like_as_a_linked_run_once() {
    // Given General preferring Sol, with Opus approved, and a recorded portable checkpoint.
    let fx = Fx::new(Setup::default());
    let first = fx.run();
    assert_eq!(binding(&first).candidate, "sol");
    fx.checkpoint();
    fx.fail_turn(&first, "s1.jsonl", MODEL_QUOTA, 10);
    // When sync observes the confirmed quota exhaustion on a quiescent worker.
    let task = fx.sync(600);
    // Then a linked continuation runs Opus with its own settings and the same profile.
    assert_eq!(task.status, Status::Running);
    assert_eq!(task.runs.len(), 2);
    assert_eq!(task.runs[0].outcome, Some(RunOutcome::Superseded));
    let next = task.current_run().unwrap();
    assert_eq!(next.agent, "general");
    assert_eq!(next.model.as_deref(), Some("anthropic/opus-fixture"));
    assert_eq!(next.vendor.as_deref(), Some("anthropic"));
    assert_eq!(next.effort.as_deref(), Some("low"));
    let bound = binding(next);
    assert_eq!(bound.candidate, "opus");
    assert_eq!(bound.episode.as_deref(), Some("T-001-R1-implementation"));
    assert!(bound.reason.as_deref().unwrap().contains("quota exhausted"));
    assert_eq!(
        next.execution.as_ref().unwrap().instructions,
        "Fixture instructions"
    );
    assert_eq!(
        fx.prompts(),
        0,
        "the failed model is never asked to recover"
    );
    let selected = fx.events("availability_fallback_selected");
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0]["from"], "sol");
    assert_eq!(selected[0]["to"], "opus");
    assert_eq!(fx.events("availability_switched").len(), 1);
    assert!(fx.events("provider_escalated").is_empty());
    // And duplicate syncs and a restart neither launch another attempt nor reset the episode.
    fx.sync(600);
    fx.sync(700);
    let reopened = Store::open_unobserved(fx.ctx.store.root()).unwrap();
    let restarted = Ctx {
        store: reopened,
        ..fx.ctx.clone()
    };
    corral_ops::sync_at(&restarted, fx.at(800)).unwrap();
    assert_eq!(fx.task().runs.len(), 2);
    assert_eq!(fx.events("availability_fallback_selected").len(), 1);
    let episode = fx.episode("T-001-R1-implementation");
    assert_eq!(episode["attempts"].as_array().unwrap().len(), 2);
    assert_eq!(episode["attempts"][0]["run"], "R1");
    assert_eq!(episode["attempts"][1]["candidate"], "opus");
    // And each model keeps its own run record for usage.
    assert_eq!(
        fx.task().runs[0].model.as_deref(),
        Some("openai/sol-fixture")
    );
}

#[test]
fn shared_account_quota_skips_same_connection_candidates() {
    // Given Sol and Sol-mini sharing one OpenAI account, then Opus.
    let fx = Fx::new(Setup {
        general: vec![sol(), sol_mini(), opus()],
        ..Setup::default()
    });
    fx.checkpoint();
    let first = fx.run();
    fx.fail_turn(
        &first,
        "s1.jsonl",
        "You have hit your usage limit for this account",
        10,
    );
    // When the account-scoped quota is classified.
    let task = fx.sync(600);
    // Then Sol-mini is skipped with a reason and Opus continues.
    assert_eq!(binding(task.current_run().unwrap()).candidate, "opus");
    let selected = fx.events("availability_fallback_selected");
    assert_eq!(selected[0]["skipped"][0]["candidate"], "sol-mini");
    assert_eq!(selected[0]["class"]["account"], true);
}

#[test]
fn rate_limits_use_the_bounded_retry_allowance_before_fallback() {
    // Given a transient rate limit on Sol.
    let fx = Fx::new(Setup::default());
    fx.checkpoint();
    let first = fx.run();
    fx.fail_turn(
        &first,
        "s1.jsonl",
        "429 Too Many Requests: rate limit reached",
        10,
    );
    // When the backoff schedule elapses without recovery.
    for (secs, prompts) in [
        (69, 0),
        (70, 1),
        (189, 1),
        (190, 2),
        (489, 2),
        (490, 3),
        (789, 3),
    ] {
        let task = fx.sync(secs);
        assert_eq!(task.runs.len(), 1, "t+{secs}");
        assert_eq!(fx.prompts(), prompts, "t+{secs}");
    }
    // Then only the exhausted allowance advances to the approved fallback.
    let task = fx.sync(790);
    assert_eq!(task.runs.len(), 2);
    assert_eq!(binding(task.current_run().unwrap()).candidate, "opus");
    assert_eq!(fx.events("provider_retry").len(), 3);
    let episode = fx.episode("T-001-R1-implementation");
    assert_eq!(episode["attempts"][0]["failures"][0]["retries"], 3);
    assert_eq!(
        episode["attempts"][0]["failures"][0]["class"]["kind"],
        "transient"
    );
}

#[test]
fn provider_recovery_keeps_the_preferred_candidate() {
    // Given a transient stall on Sol that has been retried once.
    let fx = Fx::new(Setup::default());
    let first = fx.run();
    fx.fail_turn(&first, "s1.jsonl", "Codex error: servers overloaded", 10);
    fx.sync(600);
    fx.sync(660);
    assert_eq!(fx.prompts(), 1);
    // When the same session answers again.
    fx.answer(&first, "s1.jsonl", 670);
    let task = fx.sync(700);
    // Then the run recovers in place without any availability switch.
    assert_eq!(task.runs.len(), 1);
    assert_eq!(task.status, Status::Running);
    assert_eq!(fx.events("provider_recovered").len(), 1);
    assert!(fx.events("availability_fallback_selected").is_empty());
}

#[test]
fn all_candidates_exhausted_needs_a_decision_naming_every_attempt() {
    // Given Sol unavailable and the approved Opus continuation.
    let fx = Fx::new(Setup::default());
    fx.checkpoint();
    let first = fx.run();
    fx.fail_turn(&first, "s1.jsonl", "model_not_found: sol is not served", 10);
    let task = fx.sync(600);
    let second = task.current_run().unwrap().clone();
    assert_eq!(binding(&second).candidate, "opus");
    // When Opus also exhausts its quota with no stated reset.
    fx.fail_turn(&second, "s2.jsonl", "quota exceeded for model opus", 10);
    let task = fx.sync(1200);
    // Then the task needs the owner, with the reason and every attempted candidate.
    assert_eq!(task.status, Status::NeedsMe);
    assert_eq!(task.runs.len(), 2, "never cycles back to Sol");
    let excerpt = task.current_run().unwrap().blocked_excerpt.clone().unwrap();
    assert!(excerpt.contains("quota exhausted since"), "{excerpt}");
    assert!(
        excerpt.contains("allowance (1 switch) is used"),
        "{excerpt}"
    );
    assert!(
        excerpt
            .contains("sol (openai/sol-fixture via owner-openai): model or endpoint unavailable"),
        "{excerpt}"
    );
    assert!(
        excerpt.contains("opus (anthropic/opus-fixture via owner-anthropic): quota exhausted"),
        "{excerpt}"
    );
    assert_eq!(fx.events("availability_exhausted").len(), 1);
    // And another sync neither relaunches nor repeats the decision.
    fx.sync(1500);
    assert_eq!(fx.task().runs.len(), 2);
    assert_eq!(fx.events("needs_me").len(), 1);
}

#[test]
fn a_stated_reset_waits_then_retries_the_current_candidate() {
    // Given Opus as the last approved candidate after Sol failed.
    let fx = Fx::new(Setup::default());
    fx.checkpoint();
    let first = fx.run();
    fx.fail_turn(&first, "s1.jsonl", MODEL_QUOTA, 10);
    let second = fx.sync(600).current_run().unwrap().clone();
    // When Opus reports a quota reset time.
    let reset = fx.at(5000);
    fx.fail_turn(
        &second,
        "s2.jsonl",
        &format!("quota exceeded for model opus; resets at {reset}"),
        10,
    );
    let task = fx.sync(1200);
    // Then the task waits, still Running and independent of other work, and says why.
    assert_eq!(task.status, Status::Running);
    let waiting = fx.events("availability_waiting");
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0]["until"], reset.to_string());
    let notified = std::fs::read_to_string(fx.bin_dir.join("notify.log")).unwrap_or_default();
    assert!(notified.contains("waiting for its provider"), "{notified}");
    assert_eq!(fx.sync(4999).status, Status::Running);
    assert_eq!(fx.prompts(), 0);
    // And at the reset it retries the same candidate once, without a new switch.
    assert_eq!(fx.sync(5000).runs.len(), 2);
    assert_eq!(fx.prompts(), 1);
    assert_eq!(fx.events("availability_fallback_selected").len(), 1);
}

#[test]
fn exact_model_pins_block_substitution_with_a_reason() {
    // Given a run dispatched with an explicit exact model.
    let fx = Fx::new(Setup {
        dispatch: DispatchOpts {
            profile: Some("general".into()),
            model: Some("openai/sol-fixture".into()),
            ..DispatchOpts::default()
        },
        ..Setup::default()
    });
    let first = fx.run();
    assert!(first.execution.as_ref().unwrap().candidate.is_none());
    fx.checkpoint();
    fx.fail_turn(&first, "s1.jsonl", MODEL_QUOTA, 10);
    // When its provider is exhausted.
    let task = fx.sync(600);
    // Then no substitute starts and the owner sees why.
    assert_eq!(task.runs.len(), 1);
    assert_eq!(task.status, Status::NeedsMe);
    let excerpt = task.current_run().unwrap().blocked_excerpt.clone().unwrap();
    assert!(excerpt.contains("pinned to an exact model"), "{excerpt}");
}

#[test]
fn a_working_old_worker_is_never_joined_by_a_replacement() {
    // Given a quota failure while herdr still reports the old pane working.
    let fx = Fx::new(Setup::default());
    fx.checkpoint();
    let first = fx.run();
    fx.fail_turn(&first, "s1.jsonl", MODEL_QUOTA, 10);
    fx.pane_status("working");
    // When syncs observe the live worker.
    for secs in [600, 900] {
        assert_eq!(fx.sync(secs).runs.len(), 1, "t+{secs}");
    }
    assert!(fx.events("availability_fallback_selected").is_empty());
    // Then the continuation starts only once the old worker is quiescent.
    fx.pane_status("idle");
    assert_eq!(fx.sync(1000).runs.len(), 2);
}

#[test]
fn an_untrustworthy_handoff_is_reported_and_not_replayed() {
    // Given a run with no portable checkpoint when Sol fails.
    let fx = Fx::new(Setup::default());
    let first = fx.run();
    fx.fail_turn(&first, "s1.jsonl", MODEL_QUOTA, 10);
    // When the approved fallback is selected.
    let task = fx.sync(600);
    // Then the old run stays current and the precise blocker is shown, once.
    assert_eq!(task.runs.len(), 1);
    assert_eq!(task.status, Status::NeedsMe);
    let excerpt = task.current_run().unwrap().blocked_excerpt.clone().unwrap();
    assert!(excerpt.contains("checkpoint_required"), "{excerpt}");
    assert_eq!(fx.events("availability_blocked").len(), 1);
    fx.sync(900);
    assert_eq!(fx.events("availability_blocked").len(), 1);
    assert_eq!(fx.task().runs.len(), 1);
    // And once the handoff exists, an owner retry continues with the selected candidate.
    fx.checkpoint();
    corral_ops::retry(&fx.ctx, &fx.id, &ProfileChoice::default()).unwrap();
    assert_eq!(binding(&fx.run()).candidate, "opus");
}

#[test]
fn unauthorized_automatic_dispatch_becomes_a_decision() {
    // Given no automatic dispatch authority for the task.
    let fx = Fx::new(Setup {
        authorize: false,
        ..Setup::default()
    });
    fx.checkpoint();
    let first = fx.run();
    fx.fail_turn(&first, "s1.jsonl", MODEL_QUOTA, 10);
    // When the approved fallback is selected.
    let task = fx.sync(600);
    // Then model choice grants no dispatch authority: the owner decides.
    assert_eq!(task.runs.len(), 1);
    assert_eq!(task.status, Status::NeedsMe);
    let excerpt = task.current_run().unwrap().blocked_excerpt.clone().unwrap();
    assert!(excerpt.contains("not authorized"), "{excerpt}");
}

#[test]
fn an_unavailable_preferred_model_resolves_before_start_without_cycling_back() {
    // Given local settings that do not enable Sol.
    let fx = Fx::new(Setup {
        enabled: vec![opus().model],
        ..Setup::default()
    });
    // When the task is dispatched.
    let first = fx.run();
    // Then launch resolution selects Opus within the same profile, with the reason.
    assert_eq!(first.agent, "general");
    assert_eq!(first.model.as_deref(), Some("anthropic/opus-fixture"));
    let bound = binding(&first);
    assert_eq!(bound.candidate, "opus");
    assert_eq!(bound.skipped[0].candidate, "sol");
    assert!(bound.reason.unwrap().contains("unavailable before start"));
    // And a later Opus failure never returns to the skipped Sol.
    fx.checkpoint();
    fx.fail_turn(&first, "s1.jsonl", "quota exceeded for model opus", 10);
    let task = fx.sync(600);
    assert_eq!(task.runs.len(), 1);
    assert_eq!(task.status, Status::NeedsMe);
}

#[test]
fn planning_routes_resolve_their_own_approved_candidates_before_start() {
    // Given a planning profile whose preferred model is not enabled locally.
    let plan_sol = candidate("plan-sol", "openai", "owner-openai", "high");
    let plan_opus = candidate("plan-opus", "anthropic", "owner-anthropic", "low");
    let enabled = vec![sol().model, opus().model, plan_opus.model.clone()];
    let fx = Fx::new(Setup {
        enabled,
        configure: Box::new(move |store| {
            store.config.profiles.insert(
                "planner".into(),
                profile(vec![plan_sol, plan_opus], ProfileRole::Planning),
            );
        }),
        ..Setup::default()
    });
    // When the planning route is resolved.
    let explained = corral_ops::route_snapshot(
        &fx.ctx,
        &fx.task(),
        &ProfileChoice {
            profile: Some("planner".into()),
            ..ProfileChoice::default()
        },
        true,
    )
    .unwrap();
    // Then the planning profile keeps its purpose and runs its next approved candidate.
    let route = explained.implementer;
    assert_eq!(route.profile, "planner");
    assert_eq!(route.model.as_deref(), Some("anthropic/plan-opus-fixture"));
    assert_eq!(route.availability.unwrap().candidate, "plan-opus");
}

#[test]
fn legacy_profiles_keep_transient_only_stall_handling() {
    // Given a profile without candidates.
    let fx = Fx::new(Setup {
        dispatch: DispatchOpts {
            profile: Some("legacy".into()),
            ..DispatchOpts::default()
        },
        configure: Box::new(|store| {
            store.config.profiles.insert(
                "legacy".into(),
                Profile {
                    harness: "omo".into(),
                    model: Some("openai/sol-fixture".into()),
                    vendor: None,
                    effort: None,
                    args: vec![],
                    escalate_to: None,
                    metadata: None,
                },
            );
        }),
        ..Setup::default()
    });
    let first = fx.run();
    assert!(first.execution.as_ref().unwrap().candidate.is_none());
    fx.fail_turn(&first, "s1.jsonl", MODEL_QUOTA, 10);
    // When a non-transient availability error ends its turn.
    let task = fx.sync(600);
    // Then the previous behavior is unchanged: no stall record and no fallback.
    assert_eq!(task.runs.len(), 1);
    assert!(fx.events("provider_stall").is_empty());
    assert!(fx.events("availability_fallback_selected").is_empty());
}
