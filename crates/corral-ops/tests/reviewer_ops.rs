//! Review prompt and reviewer launch against a fake herdr (`pane run` executes synchronously, `agent prompt`
//! succeeds), so nothing waits on time.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use corral_core::{AgentCmd, Profile, Status, Store, Verdict};
use corral_ops::{Ctx, DispatchOpts, DispatchOutcome};
use serde_json::json;

const FAKE_HERDR: &str = r#"#!/usr/bin/env bash
if [ "$1" = --session ]; then shift 2; fi
ok() { printf '{"id":"fake","result":%s}\n' "$1"; }
case "$1 $2" in
  "workspace create") ok '{"workspace":{"workspace_id":"w1"},"root_pane":{"workspace_id":"w1","tab_id":"w1:t1","pane_id":"w1:p1"}}' ;;
  "tab create") ok '{"root_pane":{"workspace_id":"w1","tab_id":"w1:t2","pane_id":"w1:p2"}}' ;;
  "workspace list") ok '{"workspaces":[{"workspace_id":"w1","label":"fake"}]}' ;;
  "pane run") bash -c "$4" >/dev/null 2>&1; ok '{}' ;;
  "pane get") ok "{\"pane\":{\"pane_id\":\"$3\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t1\"}}" ;;
  *) ok '{}' ;;
esac
"#;

const SPEC: &str =
    "## Goal\nMake the widget spin.\n\n## Acceptance\n- The widget spins clockwise.\n";

fn fake_herdr() -> &'static PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("corral-fake-herdr-rv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("herdr");
        std::fs::write(&bin, FAKE_HERDR).unwrap();
        let st = Command::new("chmod").arg("+x").arg(&bin).status().unwrap();
        assert!(st.success());
        bin
    })
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
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

/// A task in Review: dispatched with a fake agent, one committed change, a passing gate.
fn task_in_review() -> (tempfile::TempDir, Ctx, String, PathBuf) {
    unsafe {
        std::env::set_var("HERDR_BIN_PATH", fake_herdr());
        std::env::remove_var("CORRAL_HERDR_SESSION");
    }
    let tmp = tempfile::tempdir().unwrap();
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
    store.config.review.gate = Some(vec!["echo gate-ok".into()]);
    store.save_config().unwrap();
    let ctx = Ctx::from_store(store);
    let task = ctx
        .store
        .create("Spin the widget", Some("app"), Status::Ready, SPEC)
        .unwrap();
    let opts = DispatchOpts {
        profile: Some("fake".into()),
        ..DispatchOpts::default()
    };
    let DispatchOutcome::Started { task, .. } =
        corral_ops::dispatch(&ctx, &task.id, &opts).unwrap()
    else {
        panic!("expected Started");
    };
    let worktree = task.current_run().unwrap().worktree.clone();
    std::fs::write(worktree.join("spin.txt"), "clockwise\n").unwrap();
    corral_ops::done(&ctx, &task.id, "widget spins").unwrap();
    corral_ops::gate_start(&ctx, &task.id).unwrap();
    corral_ops::gate_wait(&ctx, &task.id).unwrap();
    (tmp, ctx, task.id, worktree)
}

#[test]
fn review_prompt_has_spec_acceptance_gate_and_verdict_footer() {
    let (_tmp, ctx, id, _worktree) = task_in_review();
    let task = ctx.store.get(&id).unwrap();
    let run = task.current_run().unwrap();
    let prompt = corral_ops::review_prompt(&ctx, &task, run).unwrap();

    assert!(
        prompt.starts_with(&format!("# Review {id} Spin the widget (run R1)")),
        "{prompt}"
    );
    assert!(prompt.contains("Make the widget spin."), "{prompt}");
    assert!(prompt.contains("- The widget spins clockwise."), "{prompt}");
    assert!(prompt.contains("widget spins"), "summary missing: {prompt}");
    assert!(prompt.contains("Gate passed at"), "{prompt}");
    let log = corral_ops::gate_paths(&ctx, &id, "R1").log;
    assert!(
        prompt.contains(&format!("Gate log: {}", log.display())),
        "{prompt}"
    );
    assert!(prompt.contains(&format!(" review {id} --diff")), "{prompt}");
    assert!(
        prompt.contains("[diff] Final diff"),
        "artifacts missing: {prompt}"
    );
    assert!(prompt.contains("/AGENTS.md"), "{prompt}");
    assert!(
        prompt.contains("Do not edit files; finish with `corral verdict`"),
        "{prompt}"
    );
    let notes = corral_ops::review_notes_path(&ctx, &id, "R1");
    assert!(
        prompt.contains(&format!(
            " verdict {id} approve --notes-file {}",
            notes.display()
        )),
        "{prompt}"
    );
    assert!(!prompt.contains("{{"), "unfilled placeholder: {prompt}");
}

#[test]
fn review_prompt_template_is_overridable() {
    let (_tmp, ctx, id, _worktree) = task_in_review();
    std::fs::write(
        ctx.store.prompts_dir().join("review.md"),
        "Custom {{id}} / {{run}} / {{unknown}}\n",
    )
    .unwrap();
    let task = ctx.store.get(&id).unwrap();
    let prompt = corral_ops::review_prompt(&ctx, &task, task.current_run().unwrap()).unwrap();
    assert_eq!(prompt, format!("Custom {id} / R1 / {{{{unknown}}}}\n"));
}

#[test]
fn review_run_with_profile_opens_a_reviewer_pane_and_records_the_review() {
    let (_tmp, ctx, id, worktree) = task_in_review();
    let head = git(&worktree, &["rev-parse", "HEAD"]);

    let record = corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();
    assert_eq!(record.verdict, None);
    assert_eq!(record.reviewer, "rev");
    assert_eq!(record.sha, head);
    assert_eq!(
        record.pane.as_deref(),
        Some("w1:p2"),
        "a tab in the run workspace"
    );

    let task = ctx.store.get(&id).unwrap();
    assert_eq!(task.current_run().unwrap().review.as_ref(), Some(&record));
    let prompt_file = ctx.store.prompts_dir().join(format!("{id}-R1-review.md"));
    let text = std::fs::read_to_string(prompt_file).unwrap();
    assert!(text.contains("Make the widget spin."), "{text}");
    let events: Vec<String> = ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert!(events.iter().any(|k| k == "review_started"), "{events:?}");
}

#[test]
fn review_run_with_pm_prompts_the_pm_pane() {
    let (_tmp, mut ctx, id, _worktree) = task_in_review();
    reviewer_profile(&mut ctx, "true".into());
    ctx.store.config.pm.agent = "rev".into();
    std::fs::write(
        ctx.store.state_dir().join("pm.json"),
        r#"{"pane_id":"w9:p9"}"#,
    )
    .unwrap();
    let record = corral_ops::review_run(&ctx, &id, Some("pm")).unwrap();
    assert_eq!(record.reviewer, "pm");
    assert_eq!(record.pane.as_deref(), Some("w9:p9"));
    let task = ctx.store.get(&id).unwrap();
    let snapshot = &task.current_run().unwrap().review_runs[0];
    assert_eq!(snapshot.worktree, ctx.store.root());
    assert_eq!(snapshot.agent, "rev");
    assert_eq!(snapshot.harness.as_deref(), Some("omo"));
    assert_eq!(snapshot.model.as_deref(), Some("review-model"));
    assert_eq!(snapshot.effort.as_deref(), Some("high"));
}

#[test]
fn review_run_needs_a_reviewer() {
    let (_tmp, ctx, id, _worktree) = task_in_review();
    let err = corral_ops::review_run(&ctx, &id, None).unwrap_err();
    assert!(format!("{err:#}").contains("no reviewer"), "{err:#}");
    let err = corral_ops::review_run(&ctx, &id, Some("nobody")).unwrap_err();
    assert!(
        format!("{err:#}").contains("unknown reviewer profile"),
        "{err:#}"
    );
}

fn reviewer_profile(ctx: &mut Ctx, command: String) {
    ctx.store.config.harnesses.insert(
        "omo".into(),
        serde_json::from_value(json!({"command": format!(": {{model_args}}; {command}")})).unwrap(),
    );
    ctx.store.config.profiles.insert(
        "rev".into(),
        Profile {
            harness: "omo".into(),
            model: Some("review-model".into()),
            effort: Some("high".into()),
            args: Vec::new(),
        },
    );
    ctx.store.config.sync.omo_agent_dir = Some(ctx.store.root().join("agent"));
}

fn transcript_path(ctx: &Ctx, cwd: &Path) -> PathBuf {
    let slug = format!(
        "--{}--",
        cwd.to_string_lossy()
            .trim_start_matches('/')
            .replace('/', "-")
    );
    let dir = ctx.store.root().join("agent/sessions").join(slug);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("review.jsonl")
}

#[test]
fn verdict_collects_first_response_emitted_synchronously_during_launch() {
    // Given a reviewer that emits usage before pane_run returns, with no timers or polling.
    let (_tmp, mut ctx, id, worktree) = task_in_review();
    let path = transcript_path(&ctx, &worktree);
    let header = json!({"type":"session","cwd":worktree}).to_string();
    let command = format!(
        r#"printf '%s\n' '{header}' > '{path}'; printf '{{"type":"message","timestamp":"%s","message":{{"role":"assistant","usage":{{"input":13,"output":7}}}}}}\n' "$(date -u +%Y-%m-%dT%H:%M:%S.%NZ)" >> '{path}'"#,
        path = path.display()
    );
    reviewer_profile(&mut ctx, command);
    let parent = ctx.store.get(&id).unwrap().current_run().unwrap().clone();
    corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();

    // When the verdict closes the review window.
    let record = corral_ops::verdict(&ctx, &id, Verdict::Approve, "Checked.").unwrap();

    // Then the first response is counted and the dispatch remains the same run.
    let task = ctx.store.get(&id).unwrap();
    assert_eq!(task.runs.len(), 1);
    let run = task.current_run().unwrap();
    assert_eq!(run.id, parent.id);
    assert_eq!(run.ended, parent.ended);
    assert_eq!(run.outcome, parent.outcome);
    assert_eq!(run.usage, parent.usage);
    assert_eq!(run.review.as_ref(), Some(&record));
    let review = &run.review_runs[0];
    assert_eq!(review.id, "R1-review1");
    assert_eq!(review.worktree, worktree);
    assert_eq!(review.agent, "rev");
    assert_eq!(review.harness.as_deref(), Some("omo"));
    assert_eq!(review.model.as_deref(), Some("review-model"));
    assert_eq!(review.effort.as_deref(), Some("high"));
    assert_eq!(review.ended, Some(record.at));
    assert!(review.started <= record.at);
    assert_eq!(review.usage.as_ref().unwrap().input, 13);
    assert_eq!(review.usage.as_ref().unwrap().output, 7);
    assert_eq!(review.usage_note, None);
    assert_eq!(review.outcome, None);
    assert!(review.review_runs.is_empty());
}

#[test]
fn replacing_active_review_closes_old_window_and_preserves_retry_history() {
    // Given an active reviewer with a fixed transcript sample inside its window.
    let (_tmp, mut ctx, id, worktree) = task_in_review();
    reviewer_profile(&mut ctx, "true".into());
    corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();
    ctx.store
        .update(&id, |t| {
            t.current_run_mut().unwrap().review_runs[0].started =
                "2020-01-01T00:00:00Z".parse().unwrap();
            Ok(())
        })
        .unwrap();
    let path = transcript_path(&ctx, &worktree);
    std::fs::write(
        path,
        format!(
            "{}\n{}\n",
            json!({"type":"session","cwd":worktree}),
            json!({"type":"message","timestamp":"2020-01-01T00:00:01Z",
                "message":{"role":"assistant","usage":{"input":11,"output":3}}})
        ),
    )
    .unwrap();

    // When another reviewer replaces it without a verdict.
    let replacement = corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();

    // Then the old window stops exactly at the new start and retains collected usage.
    let task = ctx.store.get(&id).unwrap();
    let run = task.current_run().unwrap();
    assert_eq!(task.runs.len(), 1);
    assert_eq!(run.review_runs.len(), 2);
    let old = &run.review_runs[0];
    let new = &run.review_runs[1];
    assert_eq!(old.id, "R1-review1");
    assert_eq!(new.id, "R1-review2");
    assert_eq!(old.ended, Some(new.started));
    assert_eq!(new.started, replacement.at);
    assert_eq!(new.ended, None);
    assert_eq!(old.usage.as_ref().unwrap().input, 11);
    assert_eq!(new.usage, None);
    assert_eq!(run.review.as_ref(), Some(&replacement));
}

#[test]
fn replacing_completed_review_preserves_its_verdict_window_and_usage() {
    // Given a completed review whose unsupported harness records missing-usage evidence.
    let (_tmp, ctx, id, _worktree) = task_in_review();
    corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();
    corral_ops::verdict(&ctx, &id, Verdict::Changes, "Needs changes.").unwrap();
    let completed = ctx
        .store
        .get(&id)
        .unwrap()
        .current_run()
        .unwrap()
        .review_runs[0]
        .clone();

    // When the latest ReviewRecord is replaced.
    corral_ops::review_run(&ctx, &id, Some("rev")).unwrap();

    // Then history survives verbatim without reopening the completed accounting window.
    let task = ctx.store.get(&id).unwrap();
    let run = task.current_run().unwrap();
    assert_eq!(run.review_runs.len(), 2);
    assert_eq!(run.review_runs[0], completed);
    assert!(completed.ended.is_some());
    assert!(completed.usage.is_none());
    assert!(completed.usage_note.is_some());
    assert_eq!(run.review.as_ref().unwrap().verdict, None);
}
