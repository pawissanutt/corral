//! Real git repositories, an approved integration Plan and the autopilot's POSIX fake engine,
//! condensed from the autopilot suite. The fixed clock is the T-318 incident instant.
use std::path::{Path, PathBuf};

use corral_core::{
    Accept, AgentCmd, AutoStage, GateResult, ReviewOverride, Run, Status, Store, Task, Verdict,
};
use corral_herdr::Herdr;
use corral_ops::{Ctx, DispatchOpts, DispatchOutcome};
use jiff::{SignedDuration, Timestamp};

pub fn now() -> Timestamp {
    "2026-10-25T22:43:09Z".parse().unwrap()
}

pub fn at(seconds: i64) -> Timestamp {
    now() + SignedDuration::from_secs(seconds)
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = crate::git_support::command("git", dir)
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

pub struct GitFx {
    _tmp: tempfile::TempDir,
    pub ctx: Ctx,
    bin_dir: PathBuf,
    pub id: String,
}

impl GitFx {
    fn new(policy: impl FnOnce(&mut ReviewOverride)) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let bin_dir = tmp.path().join("fake-herdr");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let bin = bin_dir.join("herdr");
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
        let ctx = Ctx {
            resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
            store,
            herdr: Herdr::with_bin(bin, None),
        };
        Self {
            _tmp: tmp,
            ctx,
            bin_dir,
            id: String::new(),
        }
    }

    pub fn task(&self) -> Task {
        self.ctx.store.get(&self.id).unwrap()
    }

    pub fn run(&self) -> Run {
        self.task().current_run().unwrap().clone()
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

    fn change(&self, rel: &str, text: &str) {
        std::fs::write(self.run().worktree.join(rel), text).unwrap();
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

    pub fn verdict(&self, verdict: Verdict, notes: &str) {
        corral_ops::verdict(&self.ctx, &self.id, verdict, notes).unwrap();
    }

    pub fn notifications(&self) -> usize {
        std::fs::read_to_string(self.bin_dir.join("notify.log"))
            .unwrap_or_default()
            .lines()
            .count()
    }

    pub fn events(&self, kind: &str) -> usize {
        self.ctx
            .store
            .events()
            .unwrap()
            .iter()
            .filter(|event| event.task.as_deref() == Some(&self.id) && event.kind == kind)
            .count()
    }

    /// Write a marker into the Plan target checkout's own administrative directory.
    pub fn marker(&self, name: &str) -> PathBuf {
        let checkout = self.run().plan_target.unwrap().checkout;
        let admin = git(&checkout, &["rev-parse", "--absolute-git-dir"]);
        let path = Path::new(&admin).join(name);
        std::fs::write(&path, b"external git writer\n").unwrap();
        path
    }
}

/// A Plan child whose gate passed and whose live reviewer has not yet given a verdict.
pub fn reviewing() -> GitFx {
    let mut fx = GitFx::new(|review| {
        review.gate = Some(vec!["true".into()]);
        review.reviewer = Some("rev".into());
        review.accept = Some(Accept::Auto);
        review.remove_worktree = Some(true);
    });
    let source = fx.ctx.store.root().join("plan.md");
    std::fs::write(
        &source,
        r#"---
title: Integration Plan
project: app
approved_scope:
  goal: Ship A
  acceptance: [{scenario: Combine changes, expected: All included}]
  delivery_target: {kind: integration, branch: feature/plan}
tasks:
  - {key: a, title: A, profile: fake}
---
"#,
    )
    .unwrap();
    let imported =
        corral_ops::import_plan(&fx.ctx, &source, false, corral_core::Origin::Owner).unwrap();
    fx.grant(std::slice::from_ref(&imported.tracker.id));
    fx.id = imported.tasks[0].id.clone();
    assert!(matches!(
        corral_ops::dispatch(&fx.ctx, &fx.id, &DispatchOpts::default()).unwrap(),
        DispatchOutcome::Started { .. }
    ));
    fx.change("a.txt", "A\n");
    fx.done();
    corral_ops::sync_at(&fx.ctx, now()).unwrap();
    let run = fx.run();
    assert_eq!(run.autopilot.unwrap().stage, AutoStage::Reviewing);
    assert_eq!(run.gate.unwrap().result, GateResult::Pass);
    // Reviewer launch uses the real clock; align its canonical start with the fixture clock.
    fx.ctx
        .store
        .update(&fx.id, |task| {
            let run = task.current_run_mut().unwrap();
            run.review.as_mut().unwrap().at = now();
            run.idle_since = Some(now());
            let child = run.review_runs.last_mut().unwrap();
            child.started = now();
            child.review.as_mut().unwrap().at = now();
            child.idle_since = Some(now());
            child.agent_state = Some("idle".into());
            Ok(())
        })
        .unwrap();
    assert_eq!(fx.task().status, Status::Review);
    fx
}
