use super::*;
use corral_core::{AgentCmd, Artifact, ArtifactKind, RunOutcome, Store};
use corral_herdr::Herdr;
use std::{path::Path, process::Stdio};

struct Fixture {
    _tmp: tempfile::TempDir,
    ctx: Ctx,
    id: String,
}

fn at(secs: i64) -> Timestamp {
    Timestamp::from_second(1_800_000_000 + secs).unwrap()
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("app");
        std::fs::create_dir(&repo).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.name", "Corral Test"],
            vec!["config", "user.email", "test@example.invalid"],
            vec![
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-qm",
                "init",
            ],
        ] {
            let result = crate::test_support::command("git", tmp.path())
                .arg("-C")
                .arg(&repo)
                .args(args)
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let mut store = Store::init(tmp.path()).unwrap();
        store.config.agents.insert(
            "fake".into(),
            AgentCmd {
                command: "true".into(),
            },
        );
        store
            .config
            .projects
            .get_mut("app")
            .unwrap()
            .default_profile = Some("fake".into());
        store.config.routing.rules.clear();
        store.config.review.autopilot = Some(false);
        store.save_config().unwrap();
        let engine = tmp.path().join("engine");
        std::fs::create_dir(&engine).unwrap();
        let bin = engine.join("herdr");
        std::os::unix::fs::symlink(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/dispatch_startup/herdr.sh"),
            &bin,
        )
        .unwrap();
        let task = store
            .create(
                "Dashboard",
                Some("app"),
                Status::Backlog,
                "Regenerate report.html and register it with title Dashboard.",
            )
            .unwrap();
        Self {
            _tmp: tmp,
            id: task.id,
            ctx: Ctx {
                store,
                herdr: Herdr::with_bin(bin, Some("corral-test".into())),
                resource_probe: crate::resources::FixedResourceProbe::arc(),
            },
        }
    }

    fn configure(&self) {
        configure(&self.ctx, &self.id, 60, at(0), true).unwrap();
    }

    fn task(&self) -> Task {
        self.ctx.store.get(&self.id).unwrap()
    }

    fn artifact(&self, run: &str, stamp: &str) {
        let path = format!("artifacts/{}/{run}/{stamp}-report.html", self.id);
        let abs = self.ctx.store.dir().join(&path);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(abs, stamp).unwrap();
        self.ctx
            .store
            .update(&self.id, |task| {
                task.artifacts.push(Artifact {
                    run: run.into(),
                    kind: ArtifactKind::Report,
                    title: "Dashboard".into(),
                    path: path.clone(),
                    source: "/stable/report.html".into(),
                    sha256: stamp.into(),
                    bytes: 8,
                    added: at(0),
                    tier_override: None,
                    compressed_bytes: None,
                    purged_at: None,
                });
                Ok(())
            })
            .unwrap();
    }
}

#[test]
fn dashboard_requires_authorization_and_positive_interval() {
    let fx = Fixture::new();
    assert!(configure(&fx.ctx, &fx.id, 60, at(0), false).is_err());
    assert!(configure(&fx.ctx, &fx.id, 0, at(0), true).is_err());
    assert!(fx.task().dashboard.is_none());
    fx.configure();
    assert!(fx.task().dispatch_hold);
    assert!(!crate::queue::authorized_ready(&fx.ctx.store, &fx.task()));
    assert!(queue_due(&fx.ctx, &fx.id, at(-1)).unwrap().is_none());
    assert!(queue_due(&fx.ctx, &fx.id, at(0)).unwrap().is_some());
    assert!(fx.task().queued);
    assert_eq!(fx.task().dashboard.unwrap().next_due, at(60));
    assert!(queue_due(&fx.ctx, &fx.id, at(0)).unwrap().is_none());
    assert!(queue_due(&fx.ctx, &fx.id, at(600)).unwrap().is_none());
}

#[test]
fn dashboard_pause_remove_and_review_states_never_recur() {
    let fx = Fixture::new();
    fx.configure();
    disable(&fx.ctx, &fx.id, false).unwrap();
    assert!(queue_due(&fx.ctx, &fx.id, at(0)).unwrap().is_none());
    fx.configure();
    for status in [
        Status::Running,
        Status::NeedsMe,
        Status::Review,
        Status::Dropped,
    ] {
        fx.ctx
            .store
            .update(&fx.id, |task| {
                task.status = status;
                Ok(())
            })
            .unwrap();
        assert!(queue_due(&fx.ctx, &fx.id, at(600)).unwrap().is_none());
    }
    disable(&fx.ctx, &fx.id, true).unwrap();
    assert!(fx.task().dashboard.is_none());
}

#[test]
fn dashboard_launch_lock_and_bad_dependencies_leave_due_unchanged() {
    let fx = Fixture::new();
    fx.configure();
    let lock = fx
        .ctx
        .store
        .try_lock(&format!("launch-{}", fx.id))
        .unwrap()
        .unwrap();
    assert!(queue_due(&fx.ctx, &fx.id, at(0)).unwrap().is_none());
    drop(lock);
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.depends_on.push("T-999".into());
            Ok(())
        })
        .unwrap();
    assert!(queue_due(&fx.ctx, &fx.id, at(0)).is_err());
    assert_eq!(fx.task().dashboard.unwrap().next_due, at(0));
    assert!(!fx.task().queued);
}

#[test]
fn dashboard_sync_runs_again_only_after_done_and_pin_follows() {
    let fx = Fixture::new();
    fx.configure();
    crate::sync_at(&fx.ctx, at(-1)).unwrap();
    assert!(fx.task().runs.is_empty());
    crate::sync_at(&fx.ctx, at(0)).unwrap();
    assert_eq!(fx.task().status, Status::Running);
    assert_eq!(fx.task().runs.len(), 1);
    crate::sync_at(&fx.ctx, at(600)).unwrap();
    assert_eq!(fx.task().runs.len(), 1);
    fx.artifact("R1", "11111111");
    let (_, pin) = fx
        .ctx
        .store
        .pin(&format!("{}:R1:report.html", fx.id), None, true)
        .unwrap();
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.status = Status::Review;
            task.current_run_mut().unwrap().outcome = Some(RunOutcome::Done);
            Ok(())
        })
        .unwrap();
    crate::sync_at(&fx.ctx, at(600)).unwrap();
    assert_eq!(fx.task().runs.len(), 1);
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.status = Status::Done;
            Ok(())
        })
        .unwrap();
    crate::sync_at(&fx.ctx, at(600)).unwrap();
    assert_eq!(fx.task().runs.len(), 2);
    assert_eq!(fx.task().dashboard.unwrap().next_due, at(660));
    crate::sync_at(&fx.ctx, at(600)).unwrap();
    assert_eq!(fx.task().runs.len(), 2);
    fx.artifact("R2", "22222222");
    let resolved = corral_core::pins::resolve_pin(&pin, &[fx.task()], fx.ctx.store.dir());
    assert!(matches!(resolved, corral_core::pins::PinTarget::Artifact {
        artifact, followed: true, ..
    } if artifact.run == "R2"));
}
