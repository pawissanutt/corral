//! cowork-landing (T-544 C9, `review required` at finish and finish into an existing task)
//! through the ops API, against a fake terminal server whose panes run gates synchronously.
#![cfg(unix)] // Fixtures execute the symlinked POSIX fake terminal server.

use std::path::{Path, PathBuf};

mod support;

use corral_core::assistant::AssistantScratch;
use corral_core::{
    Accept, AgentCmd, Cowork, CoworkReview, GateResult, Origin, Reviewer, Run, RunOutcome, Status,
};
use corral_herdr::Herdr;
use corral_ops::assistant::scratch::{self, Finish};
use corral_ops::{ApproveStrategy, Ctx, DispatchOpts, DispatchOutcome};
use serde_json::json;

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
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

struct Fx {
    _tmp: tempfile::TempDir,
    app: PathBuf,
    ctx: Ctx,
}

impl Fx {
    /// Project `app` with a passing gate, reviewer `reviewer` when given, fake agent and
    /// reviewer profiles, and assistant session A-1.
    fn new(reviewer: Option<&str>) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("herdr");
        std::os::unix::fs::symlink(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/reviewer_ops/herdr.sh"),
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
        let mut store = corral_core::Store::init(tmp.path()).unwrap();
        for name in ["fake", "rev"] {
            let command = "true".into();
            store
                .config
                .agents
                .insert(name.into(), AgentCmd { command });
        }
        store.config.review.gate = Some(vec!["echo gate-ok".into()]);
        store.config.review.reviewer = reviewer.map(str::to_owned);
        let project = serde_json::from_value(json!({"path": "app"})).unwrap();
        store.config.projects.insert("app".into(), project);
        store.save_config().unwrap();
        store.create_assistant("pi", None, None, None).unwrap();
        let ctx = Ctx {
            resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
            store,
            herdr: Herdr::with_bin(bin, None),
        };
        let app = app.canonicalize().unwrap();
        Self {
            _tmp: tmp,
            app,
            ctx,
        }
    }

    fn scratch(&self, slug: &str) -> AssistantScratch {
        scratch::create(&self.ctx, "A-1", "app", Some(slug), None).unwrap()
    }

    fn commit(&self, dir: &Path, file: &str) -> String {
        std::fs::write(dir.join(file), format!("{file}\n")).unwrap();
        git(dir, &["add", file]);
        git(dir, &["commit", "-q", "-m", &format!("add {file}")]);
        git(dir, &["rev-parse", "HEAD"])
    }

    fn kind(&self, slug: &str) -> Option<String> {
        let session = self.ctx.store.get_assistant("A-1").unwrap();
        let entry = session.scratch.into_iter().find(|s| s.slug == slug);
        entry.map(|s| s.kind)
    }

    /// Binds A-1's generation 1 to a Pi transcript with responses before, at and after `at`.
    fn usage_around(&self, at: jiff::Timestamp) {
        let transcript = self.ctx.store.dir().join("A-1-session.jsonl");
        let line = |id: &str, ts: String, input: u64| {
            format!(
                "{{\"type\":\"message\",\"id\":\"{id}\",\"timestamp\":\"{ts}\",\"message\":{{\"role\":\"assistant\",\"usage\":{{\"input\":{input},\"output\":1}}}}}}\n"
            )
        };
        let text = [
            "{\"type\":\"session\",\"id\":\"sid\"}\n".to_owned(),
            line("before", "2020-01-01T00:00:00Z".into(), 100),
            line("during", at.to_string(), 10),
            line("after", "2100-01-01T00:00:00Z".into(), 1000),
        ]
        .concat();
        std::fs::write(&transcript, text).unwrap();
        let store = &self.ctx.store;
        store
            .update_assistant_container("A-1", |container| {
                container.state.generations.push(serde_json::from_value(json!({
                    "n": 1, "bind_nonce_sha256": "nonce", "pane": "pane", "created": at,
                    "bound": {"host_cap_sha256": "host", "ctl_cap_sha256": "ctl", "session_id": "sid",
                        "session_file": transcript, "pid": 1, "start_time": "start",
                        "request_id": "hello", "at": at}
                }))?);
                container.state.binding.generation = 1;
                Ok(())
            })
            .unwrap();
    }
}

fn kinds(ctx: &Ctx, task: &str) -> Vec<String> {
    let events = ctx.store.events_shared().unwrap();
    let mine = events.iter().filter(|e| e.task.as_deref() == Some(task));
    mine.map(|event| event.kind.clone()).collect()
}

#[test]
fn c9_finish_materializes_a_gated_review_none_task_that_lands_through_approve() {
    // Given A-1's scratch cache-fix with one commit, while main moved on, in a project whose
    // policy names reviewer rev; A-1's transcript has usage before, during and after the work.
    let fx = Fx::new(Some("rev"));
    let made = fx.scratch("cache-fix");
    fx.commit(&made.path, "cache.txt");
    let moved = fx.commit(&fx.app, "main.txt");
    fx.usage_around(made.created);
    // When the owner finishes it as a fix for the cache, with no review.
    let opts = Finish {
        title: Some("Fix the cache"),
        ..Finish::default()
    };
    let finished = scratch::finish(&fx.ctx, "A-1", "cache-fix", &opts).unwrap();
    // Then a Review task exists with origin assistant, label cowork and run R1 on the scratch.
    assert_eq!(
        (finished.gate.as_str(), finished.status, &finished.reviewer),
        ("pass", Status::Review, &None)
    );
    let task = fx.ctx.store.get(&finished.task).unwrap();
    assert_eq!(task.title, "Fix the cache");
    assert_eq!(task.origin, Some(Origin::Assistant));
    assert_eq!(task.labels, ["cowork"]);
    let cowork = Cowork {
        session: "A-1".into(),
        scratch: "cache-fix".into(),
        run: "R1".into(),
        review: CoworkReview::None,
    };
    assert_eq!(task.cowork.as_ref(), Some(&cowork));
    let run = task.current_run().unwrap();
    assert_eq!((run.id.as_str(), &run.worktree), ("R1", &made.path));
    assert_eq!(
        (run.branch.as_str(), run.base.as_str()),
        (made.branch.as_str(), "main")
    );
    assert_eq!(run.outcome, Some(RunOutcome::Done));
    assert!(run.herdr.is_none());
    assert_eq!(run.gate.as_ref().map(|g| g.result), Some(GateResult::Pass));
    assert_eq!(run.gate.as_ref().unwrap().sha, finished.head);
    // The scratch was rebased onto the moved main before the gate.
    assert_eq!(git(&made.path, &["rev-parse", "HEAD^"]), moved);
    // Only the usage between scratch creation and finish is on the run.
    let usage = run.usage.as_ref().expect("A-1's usage on the run");
    assert_eq!((usage.input, usage.entries.len()), (10, 1));
    // The review choice resolves reviewer none and owner acceptance for this run.
    let policy = fx.ctx.store.config.task_review_policy(&task);
    assert_eq!(
        (policy.reviewer, policy.accept),
        (Reviewer::None, Accept::Owner)
    );
    // The scratch now belongs to the task: kind cowork, and the session cannot discard it.
    assert_eq!(fx.kind("cache-fix").as_deref(), Some("cowork"));
    let held = scratch::discard(&fx.ctx, "A-1", "cache-fix").unwrap_err();
    assert!(format!("{held:#}").contains("scratch_attached"), "{held:#}");

    // When the owner approves it, it lands through the normal approve path.
    let line =
        corral_ops::approve(&fx.ctx, &task.id, ApproveStrategy::FastForward, true, false).unwrap();
    assert!(line.contains("worktree removed"), "{line}");
    let task = fx.ctx.store.get(&task.id).unwrap();
    assert_eq!(task.status, Status::Done);
    assert_eq!(
        task.landed.as_ref().map(|l| l.sha.as_str()),
        Some(finished.head.as_str())
    );
    assert_eq!(git(&fx.app, &["rev-parse", "main"]), finished.head);
    assert!(!made.path.exists());
    // Then the session lists the landed task and commits and forgets the scratch.
    let session = fx.ctx.store.get_assistant("A-1").unwrap();
    assert!(session.scratch.is_empty());
    let landed = &session.landed[0];
    assert_eq!(
        (landed.task.as_str(), landed.run.as_str()),
        (task.id.as_str(), "R1")
    );
    assert_eq!(
        (landed.sha.as_str(), landed.base.as_str()),
        (finished.head.as_str(), "main")
    );
    assert_eq!(landed.commits, std::slice::from_ref(&finished.head));
    let seen = kinds(&fx.ctx, &task.id);
    for kind in [
        "created",
        "cowork_finished",
        "gate",
        "approved",
        "cowork_landed",
    ] {
        assert!(
            seen.iter().any(|k| k == kind),
            "{kind} missing from {seen:?}"
        );
    }
    // And stats count the finish, its review choice and the landing.
    let role = corral_ops::assistant_role_stats(&fx.ctx.store, None).unwrap();
    let counts = (
        role.cowork.finished,
        role.cowork.review_none,
        role.cowork.landed,
    );
    assert_eq!(counts, (1, 1, 1));
    assert_eq!(role.sessions[0].landed, 1);
}

#[test]
fn review_required_runs_the_normal_reviewer_after_the_gate() {
    // Given A-1's scratch with a commit in a project reviewed by rev.
    let fx = Fx::new(Some("rev"));
    let made = fx.scratch("spinner");
    fx.commit(&made.path, "spin.txt");
    // When the owner finishes it and asks for review.
    let opts = Finish {
        title: Some("Spin the widget"),
        review: Some(CoworkReview::Required),
        ..Finish::default()
    };
    let finished = scratch::finish(&fx.ctx, "A-1", "spinner", &opts).unwrap();
    // Then the gate passed and the project's reviewer started on the run, while landing still
    // waits for the owner.
    assert_eq!(finished.gate, "pass");
    assert_eq!(finished.reviewer.as_deref(), Some("rev"));
    let task = fx.ctx.store.get(&finished.task).unwrap();
    let run = task.current_run().unwrap();
    assert_eq!(
        run.review.as_ref().map(|r| r.reviewer.as_str()),
        Some("rev")
    );
    assert_eq!(run.review_runs.len(), 1);
    let policy = fx.ctx.store.config.task_review_policy(&task);
    assert_eq!(policy.reviewer, Reviewer::parse("rev"));
    assert_eq!(policy.accept, Accept::Owner);
}

#[test]
fn reviewer_none_applies_only_to_the_cowork_run() {
    // Given a project whose policy names reviewer rev and accepts automatically.
    let mut fx = Fx::new(Some("rev"));
    fx.ctx.store.config.review.accept = Some(Accept::Auto);
    let task = fx
        .ctx
        .store
        .create("Plain", Some("app"), Status::Review, "")
        .unwrap();
    let run = |id: &str| -> Run {
        serde_json::from_value(json!({"id": id, "agent": "fake", "worktree": "/tmp/w",
            "branch": "b", "base": "main", "base_sha": "0", "started": task.created}))
        .unwrap()
    };
    let policy = |task: &corral_core::Task| {
        let policy = fx.ctx.store.config.task_review_policy(task);
        (policy.reviewer, policy.accept)
    };
    let rev = Reviewer::parse("rev");
    // An ordinary task keeps the project's reviewer and acceptance.
    let mut plain = task.clone();
    plain.runs.push(run("R1"));
    assert_eq!(policy(&plain), (rev.clone(), Accept::Auto));
    // A co-work run with review none resolves no reviewer, landing by the owner only.
    let mut cowork = plain.clone();
    cowork.runs.push(run("R2"));
    cowork.cowork = Some(Cowork {
        session: "A-1".into(),
        scratch: "s".into(),
        run: "R2".into(),
        review: CoworkReview::None,
    });
    assert_eq!(policy(&cowork), (Reviewer::None, Accept::Owner));
    // A later ordinary run of the same task is reviewed and accepted as the project says.
    let mut later = cowork.clone();
    later.runs.push(run("R3"));
    assert_eq!(policy(&later), (rev, Accept::Auto));
}

#[test]
fn finish_into_an_existing_task_adds_a_run_and_refuses_task_busy_while_one_is_live() {
    // Given T-001 dispatched with the fake agent, so its run R1 is live, and A-1's scratch.
    let fx = Fx::new(None);
    let created = fx
        .ctx
        .store
        .create("Cache layer", Some("app"), Status::Ready, "")
        .unwrap();
    let opts = DispatchOpts {
        profile: Some("fake".into()),
        ..DispatchOpts::default()
    };
    let started = corral_ops::dispatch(&fx.ctx, &created.id, &opts).unwrap();
    assert!(matches!(started, DispatchOutcome::Started { .. }));
    let made = fx.scratch("cache-fix");
    fx.commit(&made.path, "cache.txt");
    let into = Finish {
        into: Some(&created.id),
        ..Finish::default()
    };
    // When the owner finishes the scratch into T-001 while R1 is live, it refuses task_busy.
    let before = fx.ctx.store.get(&created.id).unwrap();
    let busy = scratch::finish(&fx.ctx, "A-1", "cache-fix", &into).unwrap_err();
    assert!(format!("{busy:#}").contains("task_busy"), "{busy:#}");
    assert_eq!(fx.ctx.store.get(&created.id).unwrap(), before);
    assert_eq!(fx.kind("cache-fix").as_deref(), Some("spike"));
    // When R1 has ended, the same finish adds run R2 on the scratch to T-001.
    fx.ctx
        .store
        .update(&created.id, |task| {
            let run = task.current_run_mut().unwrap();
            run.outcome = Some(RunOutcome::Done);
            run.ended = Some(jiff::Timestamp::now());
            task.status = Status::Review;
            Ok(())
        })
        .unwrap();
    let finished = scratch::finish(&fx.ctx, "A-1", "cache-fix", &into).unwrap();
    assert_eq!(
        (finished.task.as_str(), finished.run.as_str()),
        (created.id.as_str(), "R2")
    );
    let task = fx.ctx.store.get(&created.id).unwrap();
    assert_eq!(
        (task.title.as_str(), task.runs.len(), task.status),
        ("Cache layer", 2, Status::Review)
    );
    assert_eq!(task.runs[1].branch, made.branch);
    assert_eq!(task.cowork.as_ref().map(|c| c.run.as_str()), Some("R2"));
    assert!(task.labels.iter().any(|label| label == "cowork"));
    assert_eq!(fx.kind("cache-fix").as_deref(), Some("cowork"));
}

#[test]
fn finish_refuses_empty_dirty_and_finished_scratch_without_filing() {
    let fx = Fx::new(None);
    let opts = Finish {
        title: Some("Nothing yet"),
        ..Finish::default()
    };
    // An empty scratch and one with uncommitted work file nothing.
    fx.scratch("empty");
    let empty = scratch::finish(&fx.ctx, "A-1", "empty", &opts).unwrap_err();
    assert!(format!("{empty:#}").contains("scratch_empty"), "{empty:#}");
    let dirty = fx.scratch("dirty");
    fx.commit(&dirty.path, "kept.txt");
    std::fs::write(dirty.path.join("loose.txt"), "loose\n").unwrap();
    let refused = scratch::finish(&fx.ctx, "A-1", "dirty", &opts).unwrap_err();
    assert!(
        format!("{refused:#}").contains("scratch_dirty"),
        "{refused:#}"
    );
    assert!(fx.ctx.store.list().unwrap().is_empty());
    // A scratch finishes once; a second finish names the task that holds it.
    std::fs::remove_file(dirty.path.join("loose.txt")).unwrap();
    let finished = scratch::finish(&fx.ctx, "A-1", "dirty", &opts).unwrap();
    let again = scratch::finish(&fx.ctx, "A-1", "dirty", &opts).unwrap_err();
    assert!(
        format!("{again:#}").contains("scratch_attached"),
        "{again:#}"
    );
    assert_eq!(fx.ctx.store.list().unwrap().len(), 1);
    assert_eq!(finished.task, fx.ctx.store.list().unwrap()[0].id);
    // Discarding the co-work task removes its worktree, and the session forgets the scratch.
    corral_ops::discard(&fx.ctx, &finished.task).unwrap();
    assert!(!dirty.path.exists());
    assert_eq!(fx.kind("dirty"), None);
}
