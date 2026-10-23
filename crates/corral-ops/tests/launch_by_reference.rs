//! T-260: prompts larger than Linux's 128 KiB per-argument limit still launch, and a launch whose
//! agent returns to its shell ends the attempt with the shell's error. The fake engine executes
//! each `pane run` command with bash, so exec limits and shell errors are real.
#![cfg(unix)] // Fixtures execute POSIX-shebang scripts.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use corral_core::{AgentCmd, Profile, Status, Store, Verdict};
use corral_herdr::Herdr;
use corral_ops::{Ctx, DispatchOpts, DispatchOutcome};

/// Linux MAX_ARG_STRLEN: one argument longer than this fails exec with E2BIG.
const MAX_ARG_STRLEN: usize = 131_072;

const FAKE_HERDR: &str = r#"#!/usr/bin/env bash
if [ "${1:-}" = --session ]; then shift 2; fi
dir="$(dirname "$0")"
ok() { printf '{"id":"fake","result":%s}\n' "$1"; }
case "$1 $2" in
  "workspace create") ok '{"workspace":{"workspace_id":"w1"},"root_pane":{"workspace_id":"w1","tab_id":"w1:t1","pane_id":"w1:p1"}}' ;;
  "workspace list") ok '{"workspaces":[{"workspace_id":"w1","label":"fake"}]}' ;;
  "pane list") ok '{"panes":[{"pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1","agent_status":"idle"}]}' ;;
  "pane get") ok '{"pane":{"pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1","agent_status":"idle"}}' ;;
  "pane run") bash -c "$4" > "$dir/output" 2>&1 </dev/null; ok '{}' ;;
  "pane read") cat "$dir/output" ;;
  "pane process-info") ok '{"process_info":{"shell_pid":10,"foreground_processes":[]}}' ;;
  *) ok '{}' ;;
esac
"#;

const AGENT: &str = "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$(dirname \"$0\")/argv\"\n";

const PLAN: &str = "---\ngoal: Make the widget spin\nnon_goals: []\ndecisions: []\naffected_paths: [widget.rs]\ncontracts: []\ndependencies: []\nacceptance:\n  - criterion: C1\n    scenario: widget --spin\n    expected: clockwise\nverification: [cargo test]\nowner_decisions: []\n---\nImplementation details.\n";

struct Fx {
    tmp: tempfile::TempDir,
    ctx: Ctx,
    engine: PathBuf,
    id: String,
}

fn executable(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}");
}

/// A task whose body alone exceeds the argument limit, so every rendered prompt does too.
fn fixture(command: &str, plan_required: bool) -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let app = tmp.path().join("app");
    std::fs::create_dir_all(&app).unwrap();
    git(&app, &["init", "-q", "-b", "main"]);
    git(&app, &["config", "user.name", "corral test"]);
    git(&app, &["config", "user.email", "corral@test.invalid"]);
    std::fs::write(app.join("README.md"), "hello\n").unwrap();
    git(&app, &["add", "-A"]);
    git(&app, &["commit", "-q", "-m", "init"]);
    let engine = tmp.path().join("engine");
    std::fs::create_dir_all(&engine).unwrap();
    executable(&engine.join("herdr"), FAKE_HERDR);
    executable(&engine.join("agent"), AGENT);
    let command = command.replace("{bin}", &engine.display().to_string());
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.harnesses.insert(
        "fixture".into(),
        serde_json::from_value(serde_json::json!({ "command": command })).unwrap(),
    );
    store.config.profiles.insert(
        "agent".into(),
        Profile {
            harness: "fixture".into(),
            model: None,
            effort: None,
            args: Vec::new(),
            vendor: None,
            escalate_to: None,
            metadata: None,
        },
    );
    store
        .config
        .agents
        .insert("reviewer".into(), AgentCmd { command });
    store.config.review.reviewer = Some("reviewer".into());
    store.config.review.plan_required = Some(plan_required);
    store.save_config().unwrap();
    let body = format!(
        "## Goal\nSpin clockwise.\n\n{}\n",
        "x".repeat(MAX_ARG_STRLEN + 8_192)
    );
    let task = store
        .create("Spin widget", Some("app"), Status::Ready, &body)
        .unwrap();
    let mut ctx = corral_ops::resources::FixedResourceProbe::ctx(store);
    ctx.herdr = Herdr::with_bin(engine.join("herdr"), Some("corral-test".into()));
    Fx {
        tmp,
        ctx,
        engine,
        id: task.id,
    }
}

impl Fx {
    fn dispatch(&self) {
        let opts = DispatchOpts {
            profile: Some("agent".into()),
            ..DispatchOpts::default()
        };
        let outcome = corral_ops::dispatch(&self.ctx, &self.id, &opts).unwrap();
        assert!(matches!(outcome, DispatchOutcome::Started { .. }));
    }

    fn plan_review(&self) -> corral_core::PlanReview {
        let path = self.tmp.path().join("plan.md");
        std::fs::write(&path, PLAN).unwrap();
        corral_ops::plan_submit(&self.ctx, &self.id, &path).unwrap();
        corral_ops::plan_review(&self.ctx, &self.id, None).unwrap()
    }

    fn shell_output(&self) -> String {
        std::fs::read_to_string(self.engine.join("output")).unwrap_or_default()
    }

    /// The agent ran and received only a short pointer at a prompt file over the limit.
    fn assert_received_by_reference(&self, prompt: &Path) {
        let argv = std::fs::read_to_string(self.engine.join("argv"))
            .unwrap_or_else(|_| panic!("agent never started: {}", self.shell_output()));
        let size = std::fs::metadata(prompt).unwrap().len();
        assert!(size > MAX_ARG_STRLEN as u64, "prompt is only {size} bytes");
        assert!(
            argv.len() < 4_096,
            "prompt was inlined ({} bytes)",
            argv.len()
        );
        assert!(argv.contains(&prompt.display().to_string()), "{argv}");
        assert!(
            !self.shell_output().contains("too long"),
            "{}",
            self.shell_output()
        );
    }
}

#[test]
fn implementation_run_launches_a_prompt_over_the_argument_limit() {
    // Given a user template still inlining the prompt with the pre-T-260 cat idiom.
    let fx = fixture(
        "'{bin}/agent' {model_args} \"$(cat '{prompt_file}')\"",
        false,
    );
    // When a run whose prompt exceeds MAX_ARG_STRLEN is dispatched through it.
    fx.dispatch();
    // Then the agent starts with the prompt by reference and the run stays live.
    let prompt = fx.ctx.store.prompts_dir().join(format!("{}-R1.md", fx.id));
    fx.assert_received_by_reference(&prompt);
    assert_eq!(fx.ctx.store.get(&fx.id).unwrap().status, Status::Running);
}

#[test]
fn plan_review_launches_a_prompt_over_the_argument_limit() {
    // Given the shipped reference form.
    let fx = fixture("'{bin}/agent' {model_args} {prompt_ref}", true);
    // When a plan review whose prompt exceeds MAX_ARG_STRLEN is launched.
    let review = fx.plan_review();
    // Then the reviewer starts with the prompt by reference.
    let plan = &fx.ctx.store.get(&fx.id).unwrap().plans[0].id;
    let prompt = fx
        .ctx
        .store
        .prompts_dir()
        .join(format!("{}-{plan}-{}-plan-review.md", fx.id, review.id));
    fx.assert_received_by_reference(&prompt);
    assert_eq!(review.verdict, None);
}

#[test]
fn other_inline_prompt_substitutions_are_refused_before_launch() {
    // Given a template that inlines the prompt in a form corral cannot rewrite.
    let fx = fixture("'{bin}/agent' \"$(< '{prompt_file}')\"", false);
    // When a run is dispatched.
    let opts = DispatchOpts {
        profile: Some("agent".into()),
        ..DispatchOpts::default()
    };
    let error = corral_ops::dispatch(&fx.ctx, &fx.id, &opts).unwrap_err();
    // Then it is refused with the reason and nothing is executed.
    assert!(
        format!("{error:#}").contains("per-argument limit"),
        "{error:#}"
    );
    assert!(!fx.engine.join("output").exists());
}

#[test]
fn worker_whose_agent_never_starts_moves_to_needs_me_with_the_error() {
    // Given a harness whose executable does not exist.
    let fx = fixture("'{bin}/missing-agent' {model_args} {prompt_ref}", false);
    fx.dispatch();
    // When sync observes the pane back at its shell.
    corral_ops::sync(&fx.ctx).unwrap();
    // Then the run is flagged with the shell's error and its evidence is attached.
    let task = fx.ctx.store.get(&fx.id).unwrap();
    let run = task.current_run().unwrap();
    assert_eq!(task.status, Status::NeedsMe);
    assert_eq!(run.agent_state.as_deref(), Some("exited"));
    let excerpt = run.blocked_excerpt.as_deref().unwrap_or_default();
    assert!(excerpt.contains("missing-agent"), "{excerpt}");
    assert!(
        task.artifacts
            .iter()
            .any(|a| a.title == "Agent launch failure")
    );
}

#[test]
fn plan_review_whose_reviewer_never_starts_ends_the_attempt() {
    // Given a plan reviewer whose executable does not exist.
    let fx = fixture("'{bin}/missing-agent' {model_args} {prompt_ref}", true);
    let review = fx.plan_review();
    assert_eq!(corral_ops::slot_usage(&fx.ctx).unwrap().reviews, 1);
    // When sync observes the reviewer's shell.
    corral_ops::sync(&fx.ctx).unwrap();
    // Then the attempt is escalated with the error, its slot is released and the owner is asked.
    let task = fx.ctx.store.get(&fx.id).unwrap();
    let ended = task.plans[0].reviews.last().unwrap();
    assert_eq!(ended.id, review.id);
    assert_eq!(ended.verdict, Some(Verdict::Escalate));
    let notes = ended.notes.as_deref().unwrap_or_default();
    assert!(notes.contains("missing-agent"), "{notes}");
    assert_eq!(task.status, Status::NeedsMe);
    assert_eq!(corral_ops::slot_usage(&fx.ctx).unwrap().reviews, 0);
}
