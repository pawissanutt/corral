use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use corral_core::{Status, Store, Task};
use corral_herdr::Herdr;
use corral_ops::Ctx;
use serde_json::Value;

#[path = "../resource_support/mod.rs"]
mod resource_support;
// A stateful protocol fixture, not a mocked dispatch service. Atomic JSON state
// preserves actual external side effects when the calling corral process dies.
pub struct Fixture {
    pub temp: tempfile::TempDir,
    pub ctx: Ctx,
    pub bin: PathBuf,
}

impl Fixture {
    pub fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        for project in ["app", "other"] {
            let repo = temp.path().join(project);
            std::fs::create_dir(&repo).unwrap();
            git(&repo, &["init", "-q", "-b", "main"]);
            git(&repo, &["config", "user.name", "queue fixture"]);
            git(&repo, &["config", "user.email", "queue@example.invalid"]);
            git(&repo, &["commit", "-q", "--allow-empty", "-m", "initial"]);
        }
        let bin = temp.path().join("herdr-fixture");
        std::os::unix::fs::symlink(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/queue_support/herdr.py"),
            &bin,
        )
        .unwrap();
        let mut store = Store::init(temp.path()).unwrap();
        resource_support::configure(&mut store.config);
        store.config.defaults.profile = Some("generic".into());
        store.config.limits.running = 1;
        store.config.review.autopilot = Some(false);
        store.save_config().unwrap();
        let ctx = Ctx {
            resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
            store,
            herdr: Herdr::with_bin(bin.clone(), Some("corral-test".into())),
        };
        Self { temp, ctx, bin }
    }

    pub fn task(&self, project: &str, status: Status) -> Task {
        self.ctx
            .store
            .create("Queue fixture", Some(project), status, "")
            .unwrap()
    }

    pub fn grant(&mut self, ids: &[&str]) {
        self.ctx.store.config.review.autopilot = None;
        self.ctx.store.save_config().unwrap();
        let preview = self
            .ctx
            .store
            .preview_operating_policy(&corral_core::OperatingSelection {
                tasks: ids.iter().map(|id| (*id).into()).collect(),
                authority: corral_core::OperatingAuthority {
                    dispatch: true,
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
        self.ctx.store.config = self
            .ctx
            .store
            .confirm_operating_policy(
                &preview,
                "fixture owner",
                "2026-10-18T03:00:00Z".parse().unwrap(),
            )
            .unwrap();
    }

    pub fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
        command
            .arg("--root")
            .arg(self.temp.path())
            .args(args)
            .env("CORRAL_ROOT", self.temp.path())
            .env_remove("CORRAL_GATE_ATTEMPT")
            .env_remove("CORRAL_CANDIDATE_HEAD")
            .env_remove("CORRAL_INTEGRATION_BASE")
            .env_remove("HERDR_PANE_ID")
            .env_remove("HERDR_WORKSPACE_ID")
            .env_remove("HERDR_TAB_ID")
            .env("HERDR_BIN_PATH", &self.bin)
            .env("CORRAL_HERDR_SESSION", "corral-test")
            .stdin(Stdio::null());
        command
    }

    pub fn cli(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    pub fn ok(&self, args: &[&str]) -> Output {
        if args.first() == Some(&"done") {
            let mut checkpoint = Command::new("bash");
            checkpoint
                .arg(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../../scripts/checkpoint-fixture.sh"
                ))
                .arg(env!("CARGO_BIN_EXE_corral"))
                .arg(args[1])
                .env("CORRAL_ROOT", self.temp.path())
                .env("HERDR_BIN_PATH", &self.bin)
                .env("CORRAL_HERDR_SESSION", "corral-test")
                .stdin(Stdio::null());
            for name in [
                "CORRAL_GATE_ATTEMPT",
                "CORRAL_CANDIDATE_HEAD",
                "CORRAL_INTEGRATION_BASE",
                "HERDR_PANE_ID",
                "HERDR_WORKSPACE_ID",
                "HERDR_TAB_ID",
            ] {
                checkpoint.env_remove(name);
            }
            let output = checkpoint.output().unwrap();
            assert!(output.status.success(), "{output:?}");
        }
        let out = self.cli(args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }

    pub fn state(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.temp.path().join("protocol.json")).unwrap())
            .unwrap()
    }

    pub fn fault(&self, point: &str) {
        std::fs::write(self.temp.path().join("fault"), point).unwrap();
    }
}

pub fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git")
        .env("CORRAL_ROOT", repo)
        .env_remove("CORRAL_GATE_ATTEMPT")
        .env_remove("CORRAL_CANDIDATE_HEAD")
        .env_remove("CORRAL_INTEGRATION_BASE")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_WORKSPACE_ID")
        .env_remove("HERDR_TAB_ID")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
