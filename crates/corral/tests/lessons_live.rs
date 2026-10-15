//! Real dispatch and review prompt delivery in the isolated corral-test engine.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use corral_core::{AgentCmd, Lesson, Status, Store};
use corral_herdr::Herdr;
use corral_ops::{Ctx, LessonProposal, LessonRequest};
use serde_json::{Value, json};

#[path = "resource_support/mod.rs"]
mod resource_support;

fn isolated(command: &mut Command) {
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("CORRAL_")
            || name.to_string_lossy().starts_with("HERDR_")
        {
            command.env_remove(name);
        }
    }
    command.stdin(Stdio::null());
}

fn git(root: &Path, args: &[&str]) {
    let mut command = Command::new("git");
    isolated(&mut command);
    let output = command.arg("-C").arg(root).args(args).output().unwrap();
    assert!(output.status.success(), "{args:?}: {output:?}");
}

fn cli(root: &Path, args: &[&str], log: &mut String) -> String {
    let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
    isolated(&mut command);
    let output = command
        .arg("--root")
        .arg(root)
        .args(args)
        .env("CORRAL_ROOT", root)
        .env("CORRAL_HERDR_SESSION", "corral-test")
        .current_dir(root)
        .output()
        .unwrap();
    let text = String::from_utf8(output.stdout).unwrap();
    log.push_str(&format!(
        "$ corral {args:?}\n{text}{}\nexit={}\n",
        String::from_utf8_lossy(&output.stderr),
        output.status
    ));
    assert!(output.status.success(), "{log}");
    text
}

fn packet(prompt: &str) -> Vec<Lesson> {
    serde_json::from_str(
        prompt
            .split("<corral-lessons>\n")
            .nth(1)
            .unwrap()
            .split("\n</corral-lessons>")
            .next()
            .unwrap(),
    )
    .unwrap()
}

struct Workspace {
    herdr: Herdr,
    id: String,
}

impl Drop for Workspace {
    fn drop(&mut self) {
        if let Err(error) = self.herdr.workspace_close(&self.id) {
            eprintln!("lesson live fixture cleanup: {error:#}");
        }
    }
}

#[test]
fn isolated_distillation_apply_dispatch_and_review_share_exact_lessons() {
    // Given a real isolated engine, finished failure history and a committed fixture repository.
    let herdr = Herdr::with_bin("herdr".into(), Some("corral-test".into()));
    if !herdr.available() {
        println!("SKIP: herdr session corral-test unavailable");
        return;
    }
    let temporary = tempfile::tempdir().unwrap();
    let root = std::env::var_os("CORRAL_LESSONS_LIVE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| temporary.path().to_path_buf());
    std::fs::create_dir_all(root.join("app")).unwrap();
    let app = root.join("app");
    git(&app, &["init", "-q", "-b", "main"]);
    git(&app, &["config", "user.name", "corral lessons fixture"]);
    git(&app, &["config", "user.email", "corral@test.invalid"]);
    git(&app, &["config", "commit.gpgsign", "false"]);
    std::fs::write(app.join("startup.rs"), "fn main() {}\n").unwrap();
    git(&app, &["add", "startup.rs"]);
    git(&app, &["commit", "-q", "-m", "initial"]);
    let mut store = Store::init(&root).unwrap();
    resource_support::configure(&mut store.config);
    store.config.herdr.session = Some("corral-test".into());
    store.config.lessons.profile = Some("generic".into());
    store.config.lessons.after_failures = Some(1);
    store.config.defaults.profile = Some("lesson-fixture".into());
    store.config.agents.insert(
        "lesson-fixture".into(),
        AgentCmd {
            command: "true".into(),
        },
    );
    store.save_config().unwrap();
    let history = store
        .create("Repair startup readiness", Some("app"), Status::Done, "")
        .unwrap();
    store
        .log_event(
            Some(&history.id),
            "changes_requested",
            json!({"text":"test startup::readiness ... FAILED"}),
        )
        .unwrap();
    let mut log = String::new();
    let spike: Value = serde_json::from_str(&cli(
        &root,
        &["lessons", "distill", "--project", "app", "--json"],
        &mut log,
    ))
    .unwrap();
    let request: LessonRequest = serde_json::from_str(
        &std::fs::read_to_string(
            store
                .state_dir()
                .join("lessons")
                .join(format!("{}.json", spike["id"].as_str().unwrap())),
        )
        .unwrap(),
    )
    .unwrap();
    let evidence = request.signals[0].evidence[0].clone();
    let proposal = LessonProposal {
        version: 1,
        project: "app".into(),
        task: request.task,
        source_digest: request.source_digest,
        lessons: vec![Lesson {
            id: "L-readiness".into(),
            rule: "Wait for process readiness before sending requests".into(),
            why: "A finished run failed its startup readiness test".into(),
            evidence: vec![evidence.clone()],
            scope: corral_core::LessonScope {
                files: vec!["startup.rs".into()],
                ..Default::default()
            },
            confidence: 0.8,
            pinned: false,
            contradicted: false,
            last_seen: evidence.ts,
        }],
    };
    let proposal_path = root.join("proposal.json");
    std::fs::write(
        &proposal_path,
        serde_json::to_vec_pretty(&proposal).unwrap(),
    )
    .unwrap();
    cli(
        &root,
        &[
            "lessons",
            "apply",
            "--project",
            "app",
            "--file",
            proposal_path.to_str().unwrap(),
        ],
        &mut log,
    );
    let task = cli(
        &root,
        &[
            "new",
            "Repair startup readiness",
            "-p",
            "app",
            "--ready",
            "--body",
            "Update startup.rs",
        ],
        &mut log,
    )
    .trim()
    .to_string();
    // When the real dispatch path submits to herdr and the owner later deletes the source lesson.
    cli(
        &root,
        &["dispatch", &task, "--profile", "lesson-fixture"],
        &mut log,
    );
    let dispatched = store.get(&task).unwrap();
    let run = dispatched.current_run().unwrap().clone();
    let workspace = Workspace {
        herdr,
        id: run.herdr.as_ref().unwrap().workspace_id.clone(),
    };
    let dispatch_path = store.prompts_dir().join(format!("{task}-{}.md", run.id));
    let dispatch_prompt = std::fs::read_to_string(&dispatch_path).unwrap();
    let preview: Vec<Lesson> = serde_json::from_str(&cli(
        &root,
        &["lessons", "--for", &task, "--applied", "--json"],
        &mut log,
    ))
    .unwrap();
    cli(
        &root,
        &["lessons", "delete", "L-readiness", "--project", "app"],
        &mut log,
    );
    store
        .update(&task, |t| {
            t.status = Status::Review;
            Ok(())
        })
        .unwrap();
    cli(
        &root,
        &["review-run", &task, "--reviewer", "lesson-fixture"],
        &mut log,
    );
    let review_path = store
        .prompts_dir()
        .join(format!("{task}-{}-review.md", run.id));
    let review_prompt = std::fs::read_to_string(&review_path).unwrap();
    // Then both actual launch files and the exposure CLI retain the same evidence-bound lesson.
    assert_eq!(packet(&dispatch_prompt), proposal.lessons);
    assert_eq!(packet(&review_prompt), preview);
    assert_eq!(preview, proposal.lessons);
    assert!(store.lessons("app").unwrap().is_empty());
    let ctx = Ctx::from_store(store.clone());
    let brief: Value =
        serde_json::from_str(&cli(&root, &["pm", "brief", "--json"], &mut log)).unwrap();
    assert!(brief["lessons"].is_object());
    let rows = corral_ops::stats(&store, corral_ops::StatsBy::Task, None).unwrap();
    let row = rows.iter().find(|r| r.group == task).unwrap();
    assert_eq!(row.lesson_effectiveness.exposed.runs, 1);
    assert_eq!(
        corral_ops::applied_lessons(&ctx, &dispatched, &run).unwrap(),
        Some(preview)
    );
    let reviewer = store
        .get(&task)
        .unwrap()
        .current_run()
        .unwrap()
        .review
        .clone()
        .unwrap();
    if let Some(pane) = reviewer.pane {
        workspace.herdr.pane_close(&pane).unwrap();
    }
    log.push_str(&format!(
        "PASS: request -> apply -> isolated dispatch -> independent review; task {task}, run {}\n",
        run.id
    ));
    if let Some(dir) = std::env::var_os("CORRAL_LESSONS_EVIDENCE_DIR") {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, text) in [
            ("live-check.log", log.clone()),
            ("dispatch-prompt.md", dispatch_prompt),
            ("review-prompt.md", review_prompt),
        ] {
            std::fs::write(dir.join(name), text).unwrap();
        }
        std::fs::write(
            dir.join("live-identity.json"),
            serde_json::to_vec_pretty(&json!({
                "root":root,"task":task,"run":run.id,"session":"corral-test",
                "workspace":workspace.id,"checks":"passed","cleanup":"workspace closed by guard"
            }))
            .unwrap(),
        )
        .unwrap();
    }
    println!("{log}");
}
