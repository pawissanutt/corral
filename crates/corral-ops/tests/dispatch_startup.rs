//! Real dispatch and sync against an isolated fake terminal engine.

use std::path::{Path, PathBuf};
use std::process::Stdio;

mod support;

use corral_core::{AgentCmd, Status, Store, Task};
use corral_herdr::Herdr;
use corral_ops::{Ctx, DispatchOpts};
use jiff::{SignedDuration, Timestamp};

struct Fx {
    _tmp: tempfile::TempDir,
    ctx: Ctx,
    bin_dir: PathBuf,
    id: String,
    started: Timestamp,
}

fn git(repo: &Path, args: &[&str]) {
    let output = support::command("git", repo)
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

impl Fx {
    fn dispatch() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("app");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.name", "corral test"]);
        git(&repo, &["config", "user.email", "corral@test.invalid"]);
        git(
            &repo,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-qm",
                "init",
            ],
        );
        let mut store = Store::init(tmp.path()).unwrap();
        store.config.agents.insert(
            "fake".into(),
            AgentCmd {
                command: "true".into(),
            },
        );
        store.save_config().unwrap();
        let bin_dir = tmp.path().join("engine");
        std::fs::create_dir(&bin_dir).unwrap();
        let bin = bin_dir.join("herdr");
        std::os::unix::fs::symlink(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/dispatch_startup/herdr.sh"),
            &bin,
        )
        .unwrap();
        let ctx = Ctx {
            store,
            herdr: Herdr::with_bin(bin, Some("corral-test".into())),
        };
        let task = ctx
            .store
            .create("Startup race", Some("app"), Status::Ready, "")
            .unwrap();
        corral_ops::dispatch(
            &ctx,
            &task.id,
            &DispatchOpts {
                profile: Some("fake".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let started = ctx
            .store
            .get(&task.id)
            .unwrap()
            .current_run()
            .unwrap()
            .started;
        Self {
            _tmp: tmp,
            ctx,
            bin_dir,
            id: task.id,
            started,
        }
    }

    fn sync(&self, millis: i64) -> Task {
        // Reopen each time to exercise persisted observation rather than process-local state.
        let ctx = Ctx {
            store: Store::open(self.ctx.store.root()).unwrap(),
            herdr: self.ctx.herdr.clone(),
        };
        corral_ops::sync_at(&ctx, self.started + SignedDuration::from_millis(millis)).unwrap();
        ctx.store.get(&self.id).unwrap()
    }
}

#[test]
fn immediate_sync_keeps_unlisted_dispatched_pane_running() {
    // Given a dispatched agent whose engine has not listed its pane yet.
    let fx = Fx::dispatch();
    // When sync samples it 350 ms after dispatch, and again just before the deadline.
    for millis in [350, 29_999] {
        let task = fx.sync(millis);
        // Then startup does not create a false owner alert.
        assert_eq!(task.status, Status::Running);
        assert_eq!(task.current_run().unwrap().blocked_excerpt, None);
    }
    assert!(!fx.bin_dir.join("notifications").exists());
    assert!(
        !fx.ctx
            .store
            .events()
            .unwrap()
            .iter()
            .any(|e| e.kind == "needs_me")
    );
}

#[test]
fn appearing_pane_stays_running_then_disappearance_is_immediate() {
    // Given a dispatched pane that first appears idle, not working.
    let fx = Fx::dispatch();
    assert_eq!(fx.sync(350).status, Status::Running);
    std::fs::write(fx.bin_dir.join("visible"), "").unwrap();
    assert_eq!(fx.sync(400).status, Status::Running);
    // When the already observed pane disappears within the startup window.
    std::fs::remove_file(fx.bin_dir.join("visible")).unwrap();
    let task = fx.sync(500);
    // Then the disappearance is actionable without waiting for the startup deadline.
    assert_eq!(task.status, Status::NeedsMe);
}

#[test]
fn unlisted_pane_needs_owner_at_startup_deadline() {
    // Given a dispatched pane that never appears.
    let fx = Fx::dispatch();
    // When the bounded default startup window expires.
    let task = fx.sync(30_000);
    // Then it is gone, with one alert even after subsequent syncs.
    assert_eq!(task.status, Status::NeedsMe);
    assert_eq!(
        task.current_run().unwrap().agent_state.as_deref(),
        Some("gone")
    );
    fx.sync(31_000);
    assert_eq!(
        std::fs::read_to_string(fx.bin_dir.join("notifications"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[test]
fn saved_startup_window_controls_the_missing_pane_deadline() {
    // Given a dispatched pane and a non-default window saved through the config API.
    let fx = Fx::dispatch();
    fx.ctx
        .store
        .edit_config(|doc| doc.set_str("sync.startup_grace_secs", "5"))
        .unwrap();
    // When separate sync processes sample just before and at that deadline.
    assert_eq!(fx.sync(4_999).status, Status::Running);
    // Then the configured deadline, not the default or last sync time, governs expiry.
    assert_eq!(fx.sync(5_000).status, Status::NeedsMe);
}

#[test]
fn legacy_observed_idle_pane_does_not_get_another_startup_window() {
    // Given a pre-upgrade run whose prior idle sighting predates the seen_pane field.
    let fx = Fx::dispatch();
    fx.ctx
        .store
        .update(&fx.id, |task| {
            let run = task.current_run_mut().unwrap();
            run.agent_state = Some("idle".into());
            let mut legacy = serde_json::to_value(&*run)?;
            legacy.as_object_mut().unwrap().remove("seen_pane");
            *run = serde_json::from_value(legacy)?;
            Ok(())
        })
        .unwrap();
    // When its pane is missing well within the startup window.
    let task = fx.sync(350);
    // Then an existing sighting still makes its disappearance immediate.
    assert_eq!(task.status, Status::NeedsMe);
}
