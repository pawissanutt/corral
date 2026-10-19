//! Review autopilot against a fake herdr (`pane run` executes synchronously, so a gate finishes inside the sync
//! that starts it) and real git repos. Verdicts are injected with `corral_ops::verdict`; the clock is fixed.
#![cfg(unix)] // Fixtures execute the symlinked POSIX fake engine.

#[path = "autopilot_ops/authority.rs"]
mod authority;
#[path = "autopilot_ops/config_identity.rs"]
mod config_identity;
#[path = "autopilot_ops/diagnostics.rs"]
mod diagnostics;
#[path = "autopilot_ops/empty_gates.rs"]
mod empty_gates;
#[path = "autopilot_ops/environment.rs"]
mod environment;
#[path = "autopilot_ops/escalation.rs"]
mod escalation;
#[path = "autopilot_ops/freshness.rs"]
mod freshness;
#[path = "autopilot_ops/gate_admission.rs"]
mod gate_admission;
#[path = "autopilot_ops/guardrails.rs"]
mod guardrails;
#[path = "autopilot_ops/integration.rs"]
mod integration;
#[path = "autopilot_ops/manual_approval.rs"]
mod manual_approval;
#[path = "autopilot_ops/plan_targets.rs"]
mod plan_targets;
#[path = "autopilot_ops/resources.rs"]
mod resources;
#[path = "autopilot_ops/rework.rs"]
mod rework;
#[path = "autopilot_ops/routing.rs"]
mod routing;
#[path = "autopilot_ops/send_back.rs"]
mod send_back;
#[path = "autopilot_ops/verdict_changes.rs"]
mod verdict_changes;

use std::path::{Path, PathBuf};
mod support;

use corral_core::{
    Accept, AgentCmd, AutoStage, GateResult, ReviewOverride, Run, RunOutcome, Status, Store, Task,
    Verdict,
};
use corral_herdr::Herdr;
use corral_ops::{ApproveStrategy, Ctx, DispatchOpts, DispatchOutcome};
use jiff::Timestamp;

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

fn gate(cmds: &[&str]) -> Option<Vec<String>> {
    Some(cmds.iter().map(|c| c.to_string()).collect())
}

/// Project "app" (a git repo on main), agents "fake" and "rev" (both run `true`), the given `[review]` policy,
/// and T-001 dispatched with "fake".
struct Fx {
    _tmp: tempfile::TempDir,
    ctx: Ctx,
    bin_dir: PathBuf,
    app: PathBuf,
    id: String,
}

impl Fx {
    fn new(policy: impl FnOnce(&mut ReviewOverride)) -> Fx {
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
        git(&app, &["add", "-A"]);
        git(&app, &["commit", "-q", "-m", "init"]);
        let mut store = Store::init(tmp.path()).unwrap();
        for name in ["fake", "rev"] {
            store.config.agents.insert(
                name.into(),
                AgentCmd {
                    command: "true".into(),
                },
            );
        }
        policy(&mut store.config.review);
        store.save_config().unwrap();
        let mut ctx = Ctx {
            resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
            store,
            herdr: Herdr::with_bin(bin, None),
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
        let opts = DispatchOpts {
            profile: Some("fake".into()),
            ..DispatchOpts::default()
        };
        let DispatchOutcome::Started { .. } = corral_ops::dispatch(&ctx, &task.id, &opts).unwrap()
        else {
            panic!("expected Started");
        };
        let preview = ctx
            .store
            .preview_operating_policy(&corral_core::OperatingSelection {
                tasks: [task.id.clone()].into(),
                authority: corral_core::OperatingAuthority {
                    dispatch: true,
                    accept: true,
                    plan_accept: true,
                },
                ..Default::default()
            })
            .unwrap();
        ctx.store.config = ctx
            .store
            .confirm_operating_policy(&preview, "fixture owner", now())
            .unwrap();
        Fx {
            _tmp: tmp,
            ctx,
            bin_dir,
            app,
            id: task.id,
        }
    }

    fn task(&self) -> Task {
        self.ctx.store.get(&self.id).unwrap()
    }

    fn grant(&mut self, ids: &[String]) {
        let preview = self
            .ctx
            .store
            .preview_operating_policy(&corral_core::OperatingSelection {
                tasks: ids.iter().cloned().collect(),
                authority: corral_core::OperatingAuthority {
                    dispatch: true,
                    accept: true,
                    plan_accept: true,
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

    fn run(&self) -> Run {
        self.task().current_run().unwrap().clone()
    }

    fn stage(&self) -> Option<AutoStage> {
        self.run().autopilot.map(|a| a.stage)
    }

    fn reason(&self) -> String {
        self.run()
            .autopilot
            .and_then(|a| a.reason)
            .unwrap_or_default()
    }

    fn change(&self, rel: &str, text: &str) {
        let file = self.run().worktree.join(rel);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }

    fn done(&self) {
        let task = self.task();
        let run = task.current_run().unwrap();
        corral_git::commit_all(
            &run.worktree,
            &format!("{}: {} (corral run {})", task.id, task.title, run.id),
        )
        .unwrap();
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
                next_action: "Review the candidate".into(),
                decisions: vec![],
                waits: vec![],
                resources: vec![],
            },
        )
        .unwrap();
        corral_ops::done(&self.ctx, &self.id, "made the change").unwrap();
    }

    fn sync(&self) -> Task {
        corral_ops::sync_at(&self.ctx, now()).unwrap();
        self.task()
    }

    fn verdict(&self, verdict: Verdict, notes: &str) {
        corral_ops::verdict(&self.ctx, &self.id, verdict, notes).unwrap();
    }

    fn herdr_log(&self, name: &str) -> String {
        std::fs::read_to_string(self.bin_dir.join(name)).unwrap_or_default()
    }

    /// The `to` stage of every "autopilot" event, in order.
    fn transitions(&self) -> Vec<String> {
        self.ctx
            .store
            .events()
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == "autopilot")
            .map(|e| e.detail["to"].as_str().unwrap_or("?").to_string())
            .collect()
    }

    fn events(&self, kind: &str) -> usize {
        let events = self.ctx.store.events().unwrap();
        events.iter().filter(|e| e.kind == kind).count()
    }

    fn artifact_text(&self, title: &str) -> String {
        let task = self.task();
        let a = task
            .artifacts
            .iter()
            .rev()
            .find(|a| a.title == title)
            .unwrap_or_else(|| panic!("no artifact {title:?}"));
        std::fs::read_to_string(self.ctx.store.dir().join(&a.path)).unwrap()
    }

    fn main_subjects(&self) -> String {
        git(&self.app, &["log", "--format=%s", "main"])
    }

    fn commit_on_main(&self, file: &str, text: &str, subject: &str) {
        std::fs::write(self.app.join(file), text).unwrap();
        git(&self.app, &["add", "-A", "--", file]);
        git(&self.app, &["commit", "-q", "-m", subject]);
    }
}

#[test]
fn gate_pass_and_reviewer_approval_land_with_an_acceptance_record() {
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.reviewer = Some("rev".into());
        r.accept = Some(Accept::Auto);
    });
    fx.change("widget.txt", "spins\n");
    fx.done();

    let t = fx.sync();
    assert_eq!(t.status, Status::Review);
    assert_eq!(fx.stage(), Some(AutoStage::Reviewing));
    let run = fx.run();
    assert_eq!(run.gate.map(|g| g.result), Some(GateResult::Pass));
    assert_eq!(run.review.map(|r| r.reviewer), Some("rev".to_string()));
    assert_eq!(fx.sync().status, Status::Review, "waits for the verdict");

    fx.verdict(Verdict::Approve, "Looks right.");
    let t = fx.sync();
    assert_eq!(t.status, Status::Done);
    let run = fx.run();
    let main = git(&fx.app, &["rev-parse", "main"]);
    let state = run.autopilot.clone().unwrap();
    assert_eq!(state.stage, AutoStage::Done);
    assert_eq!(state.sha.as_deref(), Some(main.as_str()));
    assert!(fx.main_subjects().contains("T-001: Ship the widget"));
    assert!(fx.app.join("widget.txt").is_file());
    assert!(!run.worktree.exists(), "worktree removed");
    assert_eq!(
        git(&fx.app, &["branch", "--list", "corral/T-001"]),
        "",
        "merged branch deleted"
    );
    assert_eq!(fx.transitions(), ["gating", "reviewing", "landing", "done"]);
    assert_eq!(t.rounds, 0);

    let record = fx.artifact_text("Acceptance record");
    for part in [
        main.as_str(),
        "- Gate: pass at",
        "Gate: pass",
        "- Review: approve by rev",
        "- Rounds: 0 change round(s) of max_rounds 2",
        "accept = \"auto\"",
        "gate = [\"true\"]",
        "Looks right.",
    ] {
        assert!(record.contains(part), "missing {part:?} in\n{record}");
    }
}

#[test]
fn gate_failure_sends_the_run_back_and_the_second_done_lands() {
    let fx = Fx::new(|r| {
        r.gate = gate(&["test -f fixed.txt"]);
        r.reviewer = Some("rev".into());
        r.accept = Some(Accept::Auto);
    });
    fx.change("widget.txt", "v1\n");
    fx.done();

    let t = fx.sync();
    assert_eq!(t.status, Status::Running);
    assert_eq!(t.rounds, 1);
    let run = fx.run();
    assert_eq!(run.id, "R1", "the same run continues");
    assert_eq!(run.outcome, None);
    assert_eq!(fx.stage(), Some(AutoStage::Changes));
    let events = fx.ctx.store.events().unwrap();
    let delivered = events
        .iter()
        .find(|e| e.kind == "changes_requested")
        .unwrap();
    assert_eq!(delivered.detail["by"], "autopilot");
    assert_eq!(delivered.detail["via"], "agent_prompt");
    assert_eq!(delivered.detail["submitted"], true);
    assert_eq!(delivered.detail["attempts"], 1);
    assert!(run.pending_reply.is_none());
    let prompts = fx.herdr_log("prompts.log");
    assert!(
        prompts.contains("round 1 of 2") && prompts.contains("`test -f fixed.txt`"),
        "{prompts}"
    );
    let request = fx.artifact_text("Autopilot changes (round 1)");
    assert!(
        request.contains("$ test -f fixed.txt") && request.contains("corral gate: fail"),
        "the request quotes the gate log tail:\n{request}"
    );

    fx.change("fixed.txt", "ok\n");
    fx.done();
    fx.sync();
    fx.verdict(Verdict::Approve, "The repair passed independent review.");
    let t = fx.sync();
    assert_eq!(t.status, Status::Done);
    assert_eq!(t.rounds, 1);
    assert!(fx.app.join("fixed.txt").is_file());
    assert_eq!(
        fx.transitions(),
        [
            "gating",
            "changes",
            "gating",
            "reviewing",
            "landing",
            "done"
        ]
    );
}

#[test]
fn exhausted_rounds_escalate_to_needs_me_and_the_owner_can_still_approve() {
    let fx = Fx::new(|r| {
        r.gate = gate(&["false"]);
        r.accept = Some(Accept::Auto);
        r.max_rounds = Some(1);
    });
    fx.change("a.txt", "a\n");
    fx.done();
    assert_eq!(fx.sync().status, Status::Running);
    fx.change("b.txt", "b\n");
    fx.done();

    let t = fx.sync();
    assert_eq!(t.status, Status::NeedsMe);
    assert!(t.escalated());
    assert_eq!(fx.stage(), Some(AutoStage::Escalated));
    assert!(
        fx.reason().contains("the gate failed") && fx.reason().contains("all 1 change rounds used"),
        "{}",
        fx.reason()
    );
    assert_eq!(fx.herdr_log("notify.log"), "▲ T-001 needs you\n");
    assert_eq!(fx.sync().status, Status::NeedsMe, "waits for the owner");
    assert_eq!(fx.events("autopilot"), 4);

    corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::Rebase, true, false).unwrap();
    assert_eq!(fx.task().status, Status::Done);
}

#[test]
fn touching_owner_paths_escalates() {
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.accept = Some(Accept::Auto);
        r.owner_paths = Some(vec!["secrets/**".into()]);
    });
    fx.change("secrets/key.txt", "k\n");
    fx.change("widget.txt", "w\n");
    fx.done();

    let t = fx.sync();
    assert_eq!(t.status, Status::NeedsMe);
    assert_eq!(
        fx.reason(),
        "the change touches owner_paths: secrets/key.txt"
    );
    assert!(!fx.main_subjects().contains("Ship the widget"));
}

#[test]
fn a_diff_over_max_diff_lines_escalates() {
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.accept = Some(Accept::Auto);
        r.max_diff_lines = Some(2);
    });
    fx.change("widget.txt", "1\n2\n3\n");
    fx.done();
    assert_eq!(fx.sync().status, Status::NeedsMe);
    assert_eq!(
        fx.reason(),
        "the diff has 3 changed lines, over max_diff_lines 2"
    );
}

#[test]
fn task_review_owner_escalates_under_an_auto_policy() {
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.accept = Some(Accept::Auto);
    });
    fx.ctx
        .store
        .update(&fx.id, |t| {
            t.review = Some(Accept::Owner);
            Ok(())
        })
        .unwrap();
    fx.change("widget.txt", "w\n");
    fx.done();
    assert_eq!(fx.sync().status, Status::NeedsMe);
    assert!(fx.reason().contains("review: owner"), "{}", fx.reason());
}

#[test]
fn dirty_main_checkout_blocks_until_it_is_clean() {
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.accept = Some(Accept::Auto);
        r.reviewer = Some("rev".into());
    });
    fx.change("widget.txt", "w\n");
    fx.done();
    std::fs::write(fx.app.join("README.md"), "local edit\n").unwrap();

    fx.sync();
    fx.verdict(Verdict::Approve, "Independent review passed.");
    let t = fx.sync();
    assert_eq!(t.status, Status::Review);
    assert_eq!(fx.stage(), Some(AutoStage::Blocked));
    assert_eq!(fx.reason(), "main checkout has uncommitted changes");
    fx.sync();
    assert_eq!(fx.stage(), Some(AutoStage::Blocked));
    assert_eq!(
        fx.herdr_log("notify.log"),
        "⏸ T-001 cannot land\n",
        "notified once"
    );

    git(&fx.app, &["checkout", "--", "README.md"]);
    let t = fx.sync();
    assert_eq!(t.status, Status::Done);
    assert!(fx.app.join("widget.txt").is_file());
    assert_eq!(
        fx.transitions(),
        [
            "gating",
            "reviewing",
            "landing",
            "blocked",
            "landing",
            "done"
        ]
    );
}

#[test]
fn rebase_conflict_requests_changes() {
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.accept = Some(Accept::Auto);
    });
    fx.change("README.md", "agent version\n");
    fx.done();
    let worktree = fx.run().worktree;
    let head = git(&worktree, &["rev-parse", "HEAD"]);
    fx.commit_on_main("README.md", "main version\n", "main moved");

    let t = fx.sync();
    assert_eq!(t.status, Status::Running);
    assert_eq!(t.rounds, 1);
    assert_eq!(fx.stage(), Some(AutoStage::Changes));
    let prompts = fx.herdr_log("prompts.log");
    assert!(
        prompts.contains("rebase onto `main` and resolve conflicts"),
        "{prompts}"
    );
    assert_eq!(
        git(&worktree, &["rev-parse", "HEAD"]),
        head,
        "rebase aborted"
    );
    assert_eq!(git(&worktree, &["status", "--porcelain"]), "");
}

#[test]
fn a_moved_base_is_rebased_before_the_first_gate() {
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.accept = Some(Accept::Auto);
        r.reviewer = Some("rev".into());
    });
    fx.change("widget.txt", "w\n");
    fx.done();
    fx.commit_on_main("other.txt", "o\n", "main moved");

    fx.sync();
    assert_eq!(
        fx.events("gate_started"),
        1,
        "only the integrated candidate is gated"
    );
    fx.verdict(Verdict::Approve, "Independent integrated review passed.");
    let t = fx.sync();
    assert_eq!(t.status, Status::Done);
    let subjects = fx.main_subjects();
    assert!(
        subjects.contains("main moved") && subjects.contains("T-001: Ship the widget"),
        "{subjects}"
    );
    let main = git(&fx.app, &["rev-parse", "main"]);
    let gate = fx.run().gate.unwrap();
    assert_eq!(gate.sha, main, "the last gate checked the rebased commit");
    assert_eq!(gate.result, GateResult::Pass);
    assert_eq!(fx.events("gate_started"), 1);
}

#[test]
fn accept_owner_stops_in_review_marked_reviewed() {
    let fx = Fx::new(|r| r.gate = gate(&["true"]));
    fx.ctx
        .store
        .set_attribution(corral_core::Actor::Owner, "tui")
        .unwrap();
    fx.change("widget.txt", "w\n");
    fx.done();

    let t = fx.sync();
    assert_eq!(t.status, Status::Review);
    assert_eq!(fx.stage(), Some(AutoStage::Reviewed));
    fx.sync();
    assert_eq!(fx.transitions(), ["gating", "reviewed"]);
    assert!(!fx.main_subjects().contains("Ship the widget"));
    let events = fx.ctx.store.events().unwrap();
    let transitions: Vec<_> = events.iter().filter(|e| e.kind == "autopilot").collect();
    assert_eq!(transitions.len(), 2);
    for event in transitions {
        assert_eq!(event.actor, Some(corral_core::Actor::Autopilot));
        assert_eq!(event.via.as_deref(), Some("tui"));
    }
    corral_ops::comment(
        &fx.ctx,
        &fx.id,
        "Owner follow-up",
        corral_core::CommentAuthor::Owner,
        false,
    )
    .unwrap();
    let events = fx.ctx.store.events().unwrap();
    let comment = events.iter().find(|e| e.kind == "commented").unwrap();
    assert_eq!(comment.actor, Some(corral_core::Actor::Owner));
    assert_eq!(comment.via.as_deref(), Some("tui"));
}

#[test]
fn autopilot_off_makes_no_transitions_and_manual_commands_still_work() {
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.accept = Some(Accept::Auto);
    });
    // Another process flips the kill switch; this context still holds the config it loaded with it on.
    let mut other = Store::open(fx.ctx.store.root()).unwrap();
    other.set_autopilot(false).unwrap();
    fx.change("widget.txt", "w\n");
    fx.done();

    let t = fx.sync();
    fx.sync();
    assert_eq!(t.status, Status::Review);
    let run = fx.run();
    assert_eq!(run.autopilot, None);
    assert_eq!(run.gate, None);
    assert!(fx.transitions().is_empty());

    corral_ops::gate_start(&fx.ctx, &fx.id).unwrap();
    assert_eq!(
        corral_ops::gate_wait(&fx.ctx, &fx.id).unwrap().result,
        GateResult::Pass
    );
    corral_ops::approve(&fx.ctx, &fx.id, ApproveStrategy::Rebase, true, false).unwrap();
    assert_eq!(fx.task().status, Status::Done);
    assert!(fx.transitions().is_empty());
}

#[test]
fn turning_the_autopilot_back_on_resumes() {
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.accept = Some(Accept::Auto);
        r.reviewer = Some("rev".into());
    });
    let mut other = Store::open(fx.ctx.store.root()).unwrap();
    other.set_autopilot(false).unwrap();
    fx.change("widget.txt", "w\n");
    fx.done();
    assert_eq!(fx.sync().status, Status::Review);
    other.set_autopilot(true).unwrap();
    assert_eq!(fx.sync().status, Status::Review);
    fx.verdict(Verdict::Approve, "Independent review after resume.");
    assert_eq!(fx.sync().status, Status::Done);
}

#[test]
fn reviewer_changes_loop_back_and_an_escalation_goes_to_needs_me() {
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.reviewer = Some("rev".into());
        r.accept = Some(Accept::Auto);
    });
    fx.change("widget.txt", "spins\n");
    fx.done();
    fx.sync();
    fx.verdict(Verdict::Changes, "Rename spin() to rotate().\nAdd a test.");

    let t = fx.sync();
    assert_eq!(t.status, Status::Running);
    assert_eq!(t.rounds, 1);
    let prompts = fx.herdr_log("prompts.log");
    assert!(
        prompts.contains("the reviewer (rev) requested changes: Rename spin() to rotate()"),
        "{prompts}"
    );
    assert!(
        fx.artifact_text("Autopilot changes (round 1)")
            .contains("Add a test.")
    );

    fx.change("widget.txt", "rotates\n");
    fx.done();
    fx.sync();
    assert_eq!(fx.stage(), Some(AutoStage::Reviewing), "a fresh review");
    fx.verdict(Verdict::Escalate, "Needs a product call on the API name.");
    let t = fx.sync();
    assert_eq!(t.status, Status::NeedsMe);
    assert_eq!(
        fx.reason(),
        "the reviewer (rev) escalated: Needs a product call on the API name."
    );
}

#[test]
fn a_gone_agent_pane_gets_a_retry_run_carrying_the_request() {
    let fx = Fx::new(|r| {
        r.gate = gate(&["test -f fixed.txt"]);
        r.accept = Some(Accept::Auto);
    });
    fx.change("widget.txt", "w\n");
    fx.done();
    std::fs::write(fx.bin_dir.join("gone"), "w1:p1\n").unwrap();

    let t = fx.sync();
    assert_eq!(t.status, Status::Running);
    assert_eq!(t.rounds, 1);
    assert_eq!(t.runs.len(), 2);
    assert_eq!(t.runs[0].outcome, Some(RunOutcome::Superseded));
    let run = fx.run();
    assert_eq!(run.id, "R2");
    assert_eq!(run.agent, "fake", "same profile");
    assert_eq!(run.autopilot.map(|a| a.stage), Some(AutoStage::Changes));
    let prompt = std::fs::read_to_string(fx.ctx.store.prompts_dir().join("T-001-R2.md")).unwrap();
    assert!(
        prompt.contains("## Changes requested")
            && prompt.contains("`test -f fixed.txt`")
            && prompt.contains("corral/T-001"),
        "{prompt}"
    );
    assert_eq!(fx.herdr_log("prompts.log"), "", "no agent to prompt");
}

#[test]
fn without_a_review_section_nothing_lands_or_merges() {
    let fx = Fx::new(|_| {});
    let toml = std::fs::read_to_string(fx.ctx.store.config_path()).unwrap();
    assert!(
        !toml.contains("[review]") && fx.ctx.store.config.review == ReviewOverride::default(),
        "the workspace has no [review] section:\n{toml}"
    );
    let main = git(&fx.app, &["rev-parse", "main"]);
    fx.change("widget.txt", "w\n");
    fx.done();
    let task_file = std::fs::read_to_string(&fx.task().path).unwrap();
    let worktree = fx.run().worktree;
    let head = git(&worktree, &["rev-parse", "HEAD"]);

    for _ in 0..3 {
        assert_eq!(
            fx.sync().status,
            Status::Review,
            "stays in Review for the owner"
        );
    }
    assert_eq!(
        std::fs::read_to_string(&fx.task().path).unwrap(),
        task_file,
        "the task is untouched"
    );
    let run = fx.run();
    assert_eq!(run.autopilot, None);
    assert_eq!(run.gate, None);
    assert_eq!(run.review, None);
    assert_eq!(
        git(&fx.app, &["rev-parse", "main"]),
        main,
        "main never moves"
    );
    assert!(
        !fx.app.join("widget.txt").exists(),
        "nothing merged into main"
    );
    assert_eq!(
        git(&worktree, &["rev-parse", "HEAD"]),
        head,
        "branch not rebased"
    );
    assert!(worktree.is_dir(), "worktree kept");
    for kind in [
        "autopilot",
        "approved",
        "gate_started",
        "review_started",
        "changes_requested",
    ] {
        assert_eq!(fx.events(kind), 0, "{kind}");
    }
    assert_eq!(fx.herdr_log("prompts.log"), "");
    assert_eq!(fx.herdr_log("notify.log"), "");
}
