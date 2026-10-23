//! T-260: prompts larger than Linux's 128 KiB per-argument limit still launch, and a launch whose
//! agent returns to its shell ends the attempt with the shell's error. The fake engine executes
//! each `pane run` command with bash, so exec limits and shell errors are real.
#![cfg(unix)] // Fixtures execute POSIX-shebang scripts.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use corral_core::{AgentCmd, Profile, RunOutcome, Status, Store, Verdict};
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
  "pane run")
    printf '%s\n' "$4" >> "$dir/runs.log"
    if [ -f "$dir/fish" ]; then fish --no-config -c "$4"; else bash -c "$4"; fi > "$dir/output" 2>&1 </dev/null
    ok '{}' ;;
  "pane read") cat "$dir/output" ;;
  "pane process-info") ok '{"process_info":{"shell_pid":10,"foreground_processes":[]}}' ;;
  *) ok '{}' ;;
esac
"#;

const AGENT: &str = "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$(dirname \"$0\")/argv\"\n";
const STDIN_AGENT: &str = "#!/bin/sh\nwc -c > \"$(dirname \"$0\")/stdin-bytes\"\n";

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
    executable(&engine.join("stdin-agent"), STDIN_AGENT);
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

fn dispatch_and_sync(fx: &Fx) -> corral_core::Task {
    fx.dispatch();
    corral_ops::sync(&fx.ctx).unwrap();
    fx.ctx.store.get(&fx.id).unwrap()
}

#[test]
fn no_template_spelling_passes_an_over_limit_prompt_as_an_argument() {
    for (spelling, fish) in [
        ("\"$(< '{prompt_file}')\"", false),
        ("\"$( cat '{prompt_file}')\"", false),
        ("\"$(/bin/cat '{prompt_file}')\"", false),
        ("\"` cat '{prompt_file}'`\"", false),
        ("\"$(cat \"{prompt_file}\")\"", false),
        ("\"you're a reviewer\" \"$( cat '{prompt_file}')\"", false),
        ("\"$(printf ')'; cat '{prompt_file}')\"", false),
        (
            "\"$(IFS= read -r p < '{prompt_file}'; cat '{prompt_file}')\"",
            false,
        ),
        ("(cat '{prompt_file}')", true),
    ] {
        // Given an over-limit prompt and a template inlining its file, in a bash or fish pane.
        let fx = fixture(&format!("/bin/echo {spelling}"), false);
        if fish {
            std::fs::write(fx.engine.join("fish"), "").unwrap();
        }
        // When the run is dispatched and its shell returns.
        let task = dispatch_and_sync(&fx);
        // Then no argument carried the prompt, and the returned launch ended its attempt.
        let output = fx.shell_output();
        assert!(
            !output.contains("too long") && !output.contains("exceeds"),
            "{spelling}: {output}"
        );
        assert!(
            output.len() < 4_096,
            "{spelling}: inlined {} bytes",
            output.len()
        );
        let run = task.current_run().unwrap();
        assert_eq!(
            run.outcome,
            Some(RunOutcome::Failed),
            "{spelling}: {output}"
        );
    }
}

#[test]
fn worker_that_returns_to_its_shell_ends_its_attempt_and_frees_its_slot() {
    for command in [
        "'{bin}/missing-agent' {model_args} {prompt_ref}",
        "'{bin}/agent' {model_args} {prompt_ref}",
        "cat '{prompt_file}' | '{bin}/stdin-agent'",
        "cat '{prompt_file}' | '{bin}/missing-agent'",
        "'{bin}/agent' {prompt_ref};",
        "'{bin}/missing-agent' {prompt_ref};",
        "'{bin}/agent' {prompt_ref} # trailing note",
        "'{bin}/agent' {prompt_ref} &",
    ] {
        // Given one run slot and a worker that never starts or returns without reporting.
        let fx = fixture(command, false);
        let mut store = fx.ctx.store.clone();
        store.config.limits.running = 1;
        store.save_config().unwrap();
        let mut ctx = fx.ctx.clone();
        ctx.store = store;
        let opts = DispatchOpts {
            profile: Some("agent".into()),
            ..DispatchOpts::default()
        };
        corral_ops::dispatch(&ctx, &fx.id, &opts).unwrap();
        // When sync observes the pane back at its shell.
        corral_ops::sync(&ctx).unwrap();
        // Then the attempt is ended as failed with the shell's output, and escalated.
        let task = ctx.store.get(&fx.id).unwrap();
        let run = task.current_run().unwrap();
        assert_eq!(task.status, Status::NeedsMe, "{command}");
        assert_eq!(run.outcome, Some(RunOutcome::Failed), "{command}");
        assert!(run.ended.is_some());
        assert_eq!(run.agent_state.as_deref(), Some("exited"));
        if command.contains("missing-agent") {
            let excerpt = run.blocked_excerpt.as_deref().unwrap_or_default();
            assert!(excerpt.contains("missing-agent"), "{command}: {excerpt}");
        }
        if command.contains("stdin-agent") {
            let bytes = std::fs::read_to_string(fx.engine.join("stdin-bytes")).unwrap();
            assert!(
                bytes.trim().parse::<usize>().unwrap() < 4_096,
                "stdin carried {bytes}"
            );
        }
        assert!(
            task.artifacts
                .iter()
                .any(|a| a.title == "Agent launch failure")
        );
        assert!(
            ctx.store
                .events()
                .unwrap()
                .iter()
                .any(|e| e.kind == "run_failed")
        );
        // And its slot is released: the next task is admitted.
        assert_eq!(corral_ops::slot_usage(&ctx).unwrap().tasks, 0, "{command}");
        let next = ctx
            .store
            .create("Next", Some("app"), Status::Ready, "## Goal\nNext.\n")
            .unwrap();
        let outcome = corral_ops::dispatch(&ctx, &next.id, &opts).unwrap();
        assert!(
            matches!(outcome, DispatchOutcome::Started { .. }),
            "{command}"
        );
    }
}

#[test]
fn the_generic_display_is_the_only_unarmed_launch() {
    // Given the generic harness, which shows the prompt and hands the pane to a person.
    let fx = fixture("cat '{prompt_file}'", false);
    // When it returns to the shell.
    let task = dispatch_and_sync(&fx);
    // Then the run stays open for that person rather than failing.
    assert_eq!(task.status, Status::Running);
    assert_eq!(task.current_run().unwrap().outcome, None);
}

#[test]
fn plan_review_whose_reviewer_returns_to_its_shell_ends_the_attempt() {
    for agent in ["missing-agent", "agent"] {
        // Given a plan reviewer whose executable is missing, or exits without a verdict.
        let fx = fixture(
            &format!("'{{bin}}/{agent}' {{model_args}} {{prompt_ref}}"),
            true,
        );
        let review = fx.plan_review();
        assert_eq!(corral_ops::slot_usage(&fx.ctx).unwrap().reviews, 1);
        // When sync observes the reviewer's shell.
        corral_ops::sync(&fx.ctx).unwrap();
        // Then the attempt is escalated with the reason, its slot is released and the owner is asked.
        let task = fx.ctx.store.get(&fx.id).unwrap();
        let ended = task.plans[0].reviews.last().unwrap();
        assert_eq!(ended.id, review.id);
        assert_eq!(ended.verdict, Some(Verdict::Escalate), "{agent}");
        let notes = ended.notes.as_deref().unwrap_or_default();
        assert!(notes.contains("exited to its shell"), "{notes}");
        if agent == "missing-agent" {
            assert!(notes.contains("missing-agent"), "{notes}");
        }
        assert_eq!(task.status, Status::NeedsMe);
        assert_eq!(corral_ops::slot_usage(&fx.ctx).unwrap().reviews, 0);
    }
}

#[test]
fn pm_shell_without_its_agent_is_not_reused_as_the_pm() {
    // Given a PM harness whose executable does not exist.
    let fx = fixture("'{bin}/missing-agent' {model_args} {prompt_ref}", false);
    let mut store = fx.ctx.store.clone();
    store.config.pm.agent = "agent".into();
    store.save_config().unwrap();
    let mut ctx = fx.ctx.clone();
    ctx.store = store;
    corral_ops::open_pm(&ctx).unwrap();
    // When the PM is opened again over the shell the failed agent left.
    corral_ops::open_pm(&ctx).unwrap();
    // Then the exit is reported and a fresh PM is launched instead of reusing the shell.
    let events = ctx.store.events().unwrap();
    let exited = events.iter().find(|e| e.kind == "pm_exited").unwrap();
    let exited = serde_json::to_string(exited).unwrap();
    assert!(exited.contains("missing-agent"), "{exited}");
    let runs = std::fs::read_to_string(fx.engine.join("runs.log")).unwrap();
    assert_eq!(
        runs.lines().filter(|l| l.contains("CORRAL_PM=1")).count(),
        2,
        "{runs}"
    );
}
