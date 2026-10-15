use std::path::{Path, PathBuf};
use std::process::Command;

use corral_core::{AgentCmd, Profile, Run, RunMode, Status, Store};
use corral_herdr::AgentStatus;
use corral_ops::{Ctx, DispatchOpts, DispatchOutcome};

const SESSION: &str = "corral-test";

fn git(dir: &Path, args: &[&str]) {
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
}

fn workspace() -> (tempfile::TempDir, Ctx) {
    unsafe { std::env::set_var("CORRAL_HERDR_SESSION", SESSION) };
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
    store.config.agents.insert(
        "fake".into(),
        AgentCmd {
            command: "echo corral-fake {task}".into(),
        },
    );
    store.config.herdr.session = Some(SESSION.into());
    store.save_config().unwrap();
    (tmp, Ctx::from_store(store))
}

fn sample_run(worktree: PathBuf) -> Run {
    Run {
        id: "R1".into(),
        agent: "fake".into(),
        profile: None,
        harness: None,
        model: None,
        effort: None,
        route: None,
        vendor: None,
        failures: 0,
        switches: vec![],
        retry: None,
        mode: RunMode::Interactive,
        worktree,
        branch: "corral/T-001".into(),
        base: "main".into(),
        base_sha: "0".repeat(40),
        herdr: None,
        started: jiff::Timestamp::now(),
        ended: None,
        outcome: None,
        summary: None,
        blocked_excerpt: None,
        agent_state: None,
        seen_working: false,
        gate: None,
        idle_since: None,
        idle_no_report: false,
        herdr_idle_since: None,
        stuck: None,
        pending_reply: None,
        obligations: Default::default(),
        review: None,
        review_runs: Vec::new(),
        usage: None,
        usage_note: None,
        autopilot: None,
    }
}

fn fake_opts() -> DispatchOpts {
    DispatchOpts {
        profile: Some("fake".into()),
        ..DispatchOpts::default()
    }
}

fn herdr_up() -> bool {
    let ok = Command::new("herdr")
        .args(["--session", SESSION, "workspace", "list"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        println!("SKIP: herdr session {SESSION} unavailable");
    }
    ok
}

#[test]
fn task_prompt_has_done_and_artifact_instructions() {
    let (tmp, ctx) = workspace();
    let task = ctx
        .store
        .create("Add login", Some("app"), Status::Ready, "Spec body here.\n")
        .unwrap();
    let run = sample_run(tmp.path().join("app/.worktrees/T-001"));
    let prompt = corral_ops::task_prompt(&ctx, &task, &run).unwrap();
    let corral = std::env::current_exe().unwrap().display().to_string();
    assert!(prompt.starts_with(&format!("# {} Add login", task.id)));
    assert!(prompt.contains("Spec body here."));
    assert!(prompt.contains("branch corral/T-001 from main"));
    assert!(prompt.contains(&format!("{corral} done {} --summary \"", task.id)));
    assert!(prompt.contains(&format!(
        "{corral} artifact add {} <path> --kind <report|plan|evidence|screenshot|log>",
        task.id
    )));
    assert!(prompt.contains("## Run contract"));
    assert!(!prompt.contains("<corral>"));
    assert!(!prompt.contains("{{"), "{prompt}");
}

#[test]
fn render_command_escapes_single_quotes() {
    let (tmp, mut ctx) = workspace();
    ctx.store.config.agents.insert(
        "quoted".into(),
        AgentCmd {
            command: "run -n '{name}' '{prompt_file}'".into(),
        },
    );
    let task = ctx
        .store
        .create("It's broken", Some("app"), Status::Ready, "")
        .unwrap();
    let run = sample_run(tmp.path().join("app/.worktrees/T-001"));
    let cmd =
        corral_ops::render_command(&ctx, "quoted", &task, &run, Path::new("/p/x.md")).unwrap();
    assert_eq!(
        cmd,
        format!("run -n '{} It'\\''s broken' '/p/x.md'", task.id)
    );
    let fake = corral_ops::render_command(&ctx, "fake", &task, &run, Path::new("/p/x.md")).unwrap();
    assert_eq!(fake, format!("echo corral-fake {}", task.id));
    let err =
        corral_ops::render_command(&ctx, "nope", &task, &run, Path::new("/p/x.md")).unwrap_err();
    assert!(format!("{err:#}").contains("fake"), "{err:#}");
}

#[test]
fn render_command_fills_model_args_from_profile_and_run() {
    let (tmp, mut ctx) = workspace();
    ctx.store.config.profiles.insert(
        "deep".into(),
        Profile {
            harness: "claude".into(),
            model: Some("opus".into()),
            effort: Some("high".into()),
            args: vec!["--append-system-prompt".into(), "be terse".into()],
            vendor: None,
            escalate_to: None,
        },
    );
    let task = ctx
        .store
        .create("Deep work", Some("app"), Status::Ready, "")
        .unwrap();
    let mut run = sample_run(tmp.path().join("app/.worktrees/T-001"));
    let p = Path::new("/p/x.md");
    assert_eq!(
        corral_ops::render_command(&ctx, "claude", &task, &run, p).unwrap(),
        "claude \"$(cat '/p/x.md')\""
    );
    assert_eq!(
        corral_ops::render_command(&ctx, "deep", &task, &run, p).unwrap(),
        "claude --model opus --effort high --append-system-prompt 'be terse' \"$(cat '/p/x.md')\""
    );
    run.model = Some("sonnet".into());
    run.effort = Some("max".into());
    assert_eq!(
        corral_ops::render_command(&ctx, "deep", &task, &run, p).unwrap(),
        "claude --model sonnet --effort max --append-system-prompt 'be terse' \"$(cat '/p/x.md')\""
    );
}

#[test]
fn dispatch_rejects_bad_effort_before_touching_git() {
    let (tmp, ctx) = workspace();
    let task = ctx
        .store
        .create("Bad effort", Some("app"), Status::Ready, "")
        .unwrap();
    let opts = DispatchOpts {
        profile: Some("claude".into()),
        effort: Some("off".into()),
        ..DispatchOpts::default()
    };
    let err = corral_ops::dispatch(&ctx, &task.id, &opts).unwrap_err();
    assert!(
        format!("{err:#}").contains("valid: low, medium, high, xhigh, max"),
        "{err:#}"
    );
    assert!(!tmp.path().join("app/.worktrees/T-001").exists());
}

#[test]
fn dispatch_without_slots_queues_or_errors() {
    let (_tmp, mut ctx) = workspace();
    ctx.store.config.limits.running = 0;
    let task = ctx
        .store
        .create("Blocked", Some("app"), Status::Ready, "")
        .unwrap();
    let err = corral_ops::dispatch(&ctx, &task.id, &fake_opts()).unwrap_err();
    assert!(format!("{err:#}").contains("slot"), "{err:#}");
    assert!(!ctx.store.get(&task.id).unwrap().queued);
    let opts = DispatchOpts {
        queue_if_blocked: true,
        ..fake_opts()
    };
    match corral_ops::dispatch(&ctx, &task.id, &opts).unwrap() {
        DispatchOutcome::Queued { task, reason } => {
            assert!(task.queued);
            assert_eq!(task.status, Status::Ready);
            assert!(reason.contains("slot"), "{reason}");
        }
        other => panic!("expected Queued, got {other:?}"),
    }
}

#[test]
fn dispatch_with_unmet_dependency_queues() {
    let (_tmp, ctx) = workspace();
    let dep = ctx
        .store
        .create("First", Some("app"), Status::Ready, "")
        .unwrap();
    let task = ctx
        .store
        .create("Second", Some("app"), Status::Ready, "")
        .unwrap();
    ctx.store
        .update(&task.id, |t| {
            t.depends_on = vec![dep.id.clone()];
            Ok(())
        })
        .unwrap();
    let opts = DispatchOpts {
        queue_if_blocked: true,
        ..fake_opts()
    };
    match corral_ops::dispatch(&ctx, &task.id, &opts).unwrap() {
        DispatchOutcome::Queued { task, reason } => {
            assert!(task.queued);
            assert!(task.runs.is_empty());
            assert!(reason.contains(&dep.id), "{reason}");
        }
        other => panic!("expected Queued, got {other:?}"),
    }
}

#[test]
fn herdr_dispatch_sync_reply_jump_retry() {
    if !herdr_up() {
        return;
    }
    let (tmp, mut ctx) = workspace();
    ctx.store.config.profiles.insert(
        "fake-profile".into(),
        Profile {
            harness: "fake".into(),
            model: None,
            effort: None,
            args: vec![],
            vendor: None,
            escalate_to: None,
        },
    );
    let task = ctx
        .store
        .create("Fake agent run", Some("app"), Status::Ready, "Do it.\n")
        .unwrap();
    let mut panes: Vec<String> = Vec::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let DispatchOutcome::Started {
            task: t,
            run_id,
            pane_id,
        } = corral_ops::dispatch(
            &ctx,
            &task.id,
            &DispatchOpts {
                profile: Some("fake-profile".into()),
                ..DispatchOpts::default()
            },
        )
        .unwrap()
        else {
            panic!("expected Started");
        };
        panes.push(pane_id.clone());
        println!("HERDR dispatched {} {run_id} pane {pane_id}", t.id);
        assert_eq!(run_id, "R1");
        assert_eq!(t.status, Status::Running);
        let run = t.current_run().unwrap();
        assert_eq!(run.agent, "fake-profile");
        assert_eq!(run.harness.as_deref(), Some("fake"));
        assert_eq!(run.herdr.as_ref().unwrap().pane_id, pane_id);
        assert_eq!(run.worktree, tmp.path().join("app/.worktrees/T-001"));
        assert!(run.worktree.join("README.md").exists());
        assert!(ctx.store.prompts_dir().join("T-001-R1.md").exists());

        ctx.herdr
            .pane_report_agent(
                &pane_id,
                "fake",
                AgentStatus::Blocked,
                Some("need a decision"),
            )
            .unwrap();
        let changes = corral_ops::sync(&ctx).unwrap();
        println!("HERDR sync changes {changes:?}");
        let t = ctx.store.get(&task.id).unwrap();
        assert_eq!(t.status, Status::NeedsMe);
        assert!(t.current_run().unwrap().blocked_excerpt.is_some());
        assert!(
            changes
                .iter()
                .any(|c| c.task == t.id && c.to == Status::NeedsMe)
        );

        let t = corral_ops::reply(&ctx, &task.id, "go ahead").unwrap();
        println!("HERDR reply -> {:?}", t.status);
        assert_eq!(t.status, Status::Running);
        assert!(t.current_run().unwrap().blocked_excerpt.is_none());

        corral_ops::jump(&ctx, &task.id).unwrap();
        println!("HERDR jump ok");

        let DispatchOutcome::Started {
            task: t,
            run_id,
            pane_id: pane2,
        } = corral_ops::retry(&ctx, &task.id, &corral_core::ProfileChoice::default()).unwrap()
        else {
            panic!("expected Started on retry");
        };
        panes.push(pane2.clone());
        println!(
            "HERDR retry {run_id} pane {pane2} branch {}",
            t.current_run().unwrap().branch
        );
        assert_eq!(run_id, "R2");
        assert_eq!(t.current_run().unwrap().agent, "fake-profile");
        assert_eq!(t.current_run().unwrap().harness.as_deref(), Some("fake"));
        assert_eq!(t.runs.len(), 2);
        assert_eq!(t.runs[0].outcome, Some(corral_core::RunOutcome::Superseded));
        assert_eq!(t.current_run().unwrap().branch, "corral/T-001-r2");
        assert_eq!(t.status, Status::Running);
    }));
    for pane in panes {
        if ctx.herdr.pane_get(&pane).ok().flatten().is_some() {
            ctx.herdr.pane_close(&pane).unwrap();
        }
    }
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
