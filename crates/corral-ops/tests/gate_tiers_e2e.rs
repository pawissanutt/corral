//! Gate tiers end to end, against the synchronous fake herdr and real git repos. A scripted worker
//! executes only the commands of its rendered Gate contract and a scripted reviewer judges from its
//! rendered review prompt; gate commands run `gate_tiers_e2e/suite.py`, a fake test runner that
//! selects tests from the gate context, so the tier command enforces the expected-failure set.
#![cfg(unix)] // Fixtures execute the symlinked POSIX fake engine.

#[path = "gate_tiers_e2e/conflicts.rs"]
mod conflicts;
#[path = "gate_tiers_e2e/main_bound.rs"]
mod main_bound;
#[path = "gate_tiers_e2e/plan.rs"]
mod plan;
mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use corral_core::{
    Accept, AgentCmd, ArtifactKind, CriteriaManifest, EvidenceObservation, GateRecord, GateResult,
    ReviewOverride, ReviewRecord, Store, Task, TierPolicy, Verdict,
};
use corral_herdr::Herdr;
use corral_ops::Ctx;
use jiff::Timestamp;
use serde_json::{Value, json};

const WORKER_SECTION: &str = "## Gate contract (authoritative)";
const REVIEW_SECTION: &str = "## Gate contract (review, authoritative)";

fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn now() -> Timestamp {
    "2026-10-15T09:00:00Z".parse().unwrap()
}

fn corral() -> String {
    std::env::current_exe().unwrap().display().to_string()
}

/// The fake suite's initial tests: a passing one, then optionally the expected failure the Plan
/// members declare and a failing test outside their tier selection.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Suite {
    Green,
    Broken,
}

/// Project "app" with the fake suite on main, agents "fake" (worker) and "rev" (reviewer), and a
/// tiered `[review]`: full and quick gate commands that both run the fake suite, quick for Plan
/// members and review rounds, automatic acceptance.
struct Fx {
    tmp: tempfile::TempDir,
    ctx: Ctx,
    app: PathBuf,
    log: PathBuf,
}

impl Fx {
    fn new(suite: Suite, policy: impl FnOnce(&mut ReviewOverride)) -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("fake-herdr");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let bin = bin_dir.join("herdr");
        // Never write the executable: parallel forks can inherit a writer and cause ETXTBSY.
        std::os::unix::fs::symlink(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/autopilot_ops/herdr.sh"),
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
        let broken = if suite == Suite::Broken {
            "fail"
        } else {
            "pass"
        };
        for (test, outcome) in [
            ("core/widget", "pass"),
            ("core/cache", broken),
            ("other/legacy", broken),
        ] {
            let file = app.join("suite").join(test);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, format!("{outcome}\n")).unwrap();
        }
        git(&app, &["add", "-A"]);
        git(&app, &["commit", "-q", "-m", "init"]);
        let log = tmp.path().join("suite.log");
        let mut store = Store::init(tmp.path()).unwrap();
        for name in ["fake", "rev"] {
            store.config.agents.insert(
                name.into(),
                AgentCmd {
                    command: "true".into(),
                },
            );
        }
        store.config.limits.running = 8;
        store.config.limits.review = 4;
        let review = &mut store.config.review;
        review.gate = Some(vec![suite_command("full", &log)]);
        review.gate_tiers = Some(BTreeMap::from([(
            "quick".to_owned(),
            vec![suite_command("quick", &log)],
        )]));
        review.tier_policy = Some(TierPolicy {
            plan_member: Some("quick".into()),
            review_rounds: Some("quick".into()),
        });
        review.reviewer = Some("rev".into());
        review.accept = Some(Accept::Auto);
        review.remove_worktree = Some(false);
        policy(review);
        store.save_config().unwrap();
        let ctx = Ctx {
            resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
            store,
            herdr: Herdr::with_bin(bin, None),
        };
        Fx { tmp, ctx, app, log }
    }

    fn task(&self, id: &str) -> Task {
        self.ctx.store.get(id).unwrap()
    }

    fn run(&self, id: &str) -> corral_core::Run {
        self.task(id).current_run().unwrap().clone()
    }

    fn sync(&self) {
        corral_ops::sync_at(&self.ctx, now()).unwrap();
    }

    fn full(&self) -> String {
        suite_command("full", &self.log)
    }

    fn quick(&self) -> String {
        suite_command("quick", &self.log)
    }

    /// Operating authority to dispatch, gate, review and land `ids` automatically.
    fn grant(&mut self, ids: &[&str]) {
        let preview = self
            .ctx
            .store
            .preview_operating_policy(&corral_core::OperatingSelection {
                tasks: ids.iter().map(|id| (*id).to_owned()).collect(),
                authority: corral_core::OperatingAuthority {
                    dispatch: true,
                    accept: true,
                    plan_accept: true,
                    aggregate_accept: false,
                },
                ..Default::default()
            })
            .unwrap();
        self.ctx.store.config = self
            .ctx
            .store
            .confirm_operating_policy(&preview, "fixture owner", now())
            .unwrap();
    }

    fn audit(&self, id: &str) {
        let task = self.task(id);
        let manifest: CriteriaManifest = serde_json::from_value(json!({
            "version": 1, "spec_digest": corral_ops::completion_spec_digest(&task).unwrap(),
            "reviewed_by": "owner", "audited": true, "require_gate": true,
            "criteria": [{"id": "C1", "required": true, "live": false,
                "scenario": "the change works", "expected": "pass"}],
            "owner_decisions": [],
        }))
        .unwrap();
        corral_ops::register_criteria(&self.ctx, id, &manifest).unwrap();
    }

    /// Lines of the fake suite log, as `(label, attempt, test, status)`.
    fn suite(&self) -> Vec<(String, String, String, String)> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(|line| {
                let f: Vec<&str> = line.split(' ').collect();
                (
                    f[0].to_owned(),
                    f[1].to_owned(),
                    format!("{} {}", f[2], f[3]),
                    f[4].to_owned(),
                )
            })
            .collect()
    }

    /// The tier label of each suite run, in order (one per gate).
    fn tiers_run(&self) -> Vec<String> {
        let mut runs: Vec<(String, String)> = Vec::new();
        for (label, attempt, _, _) in self.suite() {
            if runs.last() != Some(&(label.clone(), attempt.clone())) {
                runs.push((label, attempt));
            }
        }
        runs.into_iter().map(|(label, _)| label).collect()
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

    fn commit_on_main(&self, file: &str, text: &str) {
        std::fs::write(self.app.join(file), text).unwrap();
        git(&self.app, &["add", "-A", "--", file]);
        git(&self.app, &["commit", "-q", "-m", "main advanced"]);
    }
}

fn suite_command(tier: &str, log: &Path) -> String {
    let suite = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gate_tiers_e2e/suite.py");
    format!("python3 -I {} {tier} {}", suite.display(), log.display())
}

/// The one Gate contract section of `prompt`, up to the next section.
fn section<'a>(prompt: &'a str, heading: &str) -> &'a str {
    assert_eq!(prompt.matches(heading).count(), 1, "{prompt}");
    let start = prompt.find(heading).unwrap();
    let rest = &prompt[start + heading.len()..];
    let end = rest
        .find("\n## ")
        .map_or(rest.len(), |end| end + heading.len());
    &prompt[start..start + end]
}

/// The tier token of a Gate contract's `- Tier:` line.
fn parsed_tier(contract: &str) -> &str {
    let line = contract
        .lines()
        .find_map(|line| line.strip_prefix("- Tier: "))
        .unwrap_or_else(|| panic!("no tier line in:\n{contract}"));
    line.split([',', ' ']).next().unwrap_or_default()
}

fn spans(line: &str) -> Vec<&str> {
    line.split('`').skip(1).step_by(2).collect()
}

struct Round<'a> {
    files: &'a [(&'a str, &'a str)],
    summary: &'a str,
}

/// The scripted worker: reads the run's delivered prompt, checks it states only this tier, makes
/// and commits its change, then executes the Gate contract's order verbatim: the rebase onto its
/// base, the one receipted gate, evidence for the attempt it printed when audited, the completion
/// audit, and the Run contract's `done`.
fn work(fx: &Fx, id: &str, tier: &str, round: &Round) -> GateRecord {
    let run = fx.run(id);
    let prompt = std::fs::read_to_string(
        fx.ctx
            .store
            .prompts_dir()
            .join(format!("{id}-{}.md", run.id)),
    )
    .unwrap();
    let contract = section(&prompt, WORKER_SECTION);
    assert_eq!(parsed_tier(contract), tier, "{contract}");
    assert!(
        contract.contains(&format!("\n      {}\n", fx.quick())),
        "{contract}"
    );
    assert!(
        !prompt.contains(&fx.full()),
        "full command leaked:\n{prompt}"
    );
    assert!(
        !prompt.to_ascii_lowercase().contains("run the full gate"),
        "{prompt}"
    );
    let gate_line = contract
        .lines()
        .find(|line| line.starts_with("- Gate: "))
        .unwrap();
    let gate = spans(gate_line)[0];
    assert_eq!(gate, format!("{} gate {id} --wait", corral()));
    let order = contract
        .lines()
        .find(|line| line.starts_with("- Order: "))
        .unwrap();
    for (file, text) in round.files {
        let path = run.worktree.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    corral_git::commit_all(&run.worktree, &format!("{id}: {}", round.summary)).unwrap();
    let mut receipt = None;
    let mut done = false;
    for command in spans(order) {
        let argv = command.strip_prefix(&format!("{} ", corral()));
        let argv: Vec<&str> = argv.map_or_else(|| vec![command], |a| a.split(' ').collect());
        match argv.as_slice() {
            [base] if *base == run.base => {
                git(&run.worktree, &["rebase", "-q", &run.base]);
            }
            ["done"] => {
                corral_ops::done(&fx.ctx, id, round.summary).unwrap();
                done = true;
            }
            ["gate", task, "--wait"] if *task == id => {
                assert!(receipt.is_none(), "a second gate in one round");
                corral_ops::gate_start(&fx.ctx, id).unwrap();
                let gate = corral_ops::gate_wait(&fx.ctx, id).unwrap();
                assert_eq!(gate.result, GateResult::Pass, "{:?}", gate.failed_cmd);
                receipt = Some(gate);
            }
            ["evidence", "record", task] if *task == id => {
                if fx.task(id).completion.is_some() {
                    let gate = receipt.as_ref().expect("evidence before the gate");
                    observe(fx, id, &gate.identity.as_ref().unwrap().attempt);
                }
            }
            ["done", task, "--check"] if *task == id => {
                checkpoint(fx, id);
                let audit = corral_ops::completion_check(&fx.ctx, id, round.summary).unwrap();
                assert!(audit.passed, "{:?}", audit.failures);
            }
            other => panic!("unexpected Gate contract command {other:?} in {order}"),
        }
    }
    assert!(done, "the Gate contract order ends without done: {order}");
    receipt.expect("the Gate contract named no gate")
}

fn observe(fx: &Fx, id: &str, attempt: &str) {
    let proof = fx.tmp.path().join(format!("{id}-{attempt}-proof.txt"));
    std::fs::write(&proof, format!("pass on {attempt}\n")).unwrap();
    let artifact = corral_ops::add_artifact(
        &fx.ctx,
        id,
        &proof,
        Some(ArtifactKind::Evidence),
        Some("Scenario proof"),
    )
    .unwrap();
    let observation: EvidenceObservation = serde_json::from_value(json!({
        "version": 1, "key": format!("{id}-{attempt}"), "criterion": "C1", "run": fx.run(id).id,
        "attempt": attempt, "status": "pass", "action": "exercise the change",
        "environment": "fixture", "expected": "pass", "observed": "pass",
        "artifacts": [artifact.path], "captured": corral_core::clock::now(),
        "producer": "scripted worker", "live": false,
    }))
    .unwrap();
    corral_ops::record_evidence(&fx.ctx, id, &observation).unwrap();
}

fn checkpoint(fx: &Fx, id: &str) {
    let identity = corral_ops::progress_readback(&fx.ctx, id).unwrap();
    corral_ops::save_checkpoint(
        &fx.ctx,
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

/// The scripted reviewer: judges from the review prompt autopilot rendered. It requires one review
/// Gate contract that names `tier` and demands no full gate, and records each failure the worker
/// left under `Not verified:` as a non-blocking note, as that contract instructs. Returns the
/// recorded verdict.
fn review(fx: &Fx, id: &str, tier: &str, verdict: Verdict, request: &str) -> ReviewRecord {
    let run = fx.run(id);
    let pending = run.review.as_ref().expect("no review launched");
    assert_eq!(pending.verdict, None, "the review already has a verdict");
    let path = fx
        .ctx
        .store
        .prompts_dir()
        .join(format!("{id}-{}-review.md", run.id));
    let prompt = std::fs::read_to_string(path).unwrap();
    let contract = section(&prompt, REVIEW_SECTION);
    assert_eq!(parsed_tier(contract), tier, "{contract}");
    assert!(
        !prompt.contains(&fx.full()),
        "full command leaked:\n{prompt}"
    );
    let notes: Vec<String> = prompt
        .lines()
        .filter_map(|line| line.trim().strip_prefix("Not verified:"))
        .map(str::trim)
        .filter(|tail| *tail != "none")
        .map(|tail| format!("Non-blocking note: {tail}"))
        .collect();
    let notes = format!("{request}\n{}", notes.join("\n"));
    let record = corral_ops::verdict(&fx.ctx, id, verdict, notes.trim()).unwrap();
    assert_eq!(record.verdict, Some(verdict), "{:?}", record.notes);
    record
}
