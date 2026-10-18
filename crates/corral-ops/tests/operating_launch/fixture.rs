use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use corral_core::{AgentCmd, OperatingAuthority, OperatingSelection, Status, Store, Task};
use corral_herdr::Herdr;
use corral_ops::{Ctx, DispatchOpts, DispatchOutcome};
use jiff::Timestamp;

pub struct Fx {
    pub temp: tempfile::TempDir,
    pub ctx: Ctx,
    pub bin: PathBuf,
}

pub fn now() -> Timestamp {
    "2026-10-18T12:00:00Z".parse().unwrap()
}

pub fn git(repo: &Path, args: &[&str]) -> String {
    let result = Command::new("git")
        .env_remove("CORRAL_GATE_ATTEMPT")
        .env("CORRAL_ROOT", repo)
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().into()
}

impl Fx {
    pub fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("app");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.name", "Corral Test"]);
        git(&repo, &["config", "user.email", "fixture@example.invalid"]);
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
        let mut store = Store::init(temp.path()).unwrap();
        for agent in ["fake", "rev"] {
            store.config.agents.insert(
                agent.into(),
                AgentCmd {
                    command: "true".into(),
                },
            );
        }
        store
            .config
            .projects
            .get_mut("app")
            .unwrap()
            .default_profile = Some("fake".into());
        store.config.routing.rules.clear();
        store.config.limits.running = 8;
        store.config.limits.review = 2;
        store.config.review.gate = Some(vec!["true".into()]);
        store.config.review.reviewer = Some("rev".into());
        store.save_config().unwrap();
        let bin = temp.path().join("engine");
        std::fs::create_dir(&bin).unwrap();
        let executable = bin.join("herdr");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/operating_launch/herdr.py"),
            &executable,
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            temp,
            ctx: Ctx {
                store,
                herdr: Herdr::with_bin(executable, Some("corral-test".into())),
                resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
            },
            bin,
        }
    }

    pub fn task(&self, title: &str, status: Status) -> Task {
        self.ctx
            .store
            .create(title, Some("app"), status, "Finite fixture contract")
            .unwrap()
    }

    pub fn grant(&mut self, tasks: &[&str]) {
        let selection = OperatingSelection {
            tasks: tasks.iter().map(|id| (*id).to_owned()).collect(),
            authority: OperatingAuthority {
                dispatch: true,
                accept: true,
                plan_accept: true,
            },
            ..Default::default()
        };
        let preview = self.ctx.store.preview_operating_policy(&selection).unwrap();
        self.ctx.store.config = self
            .ctx
            .store
            .confirm_operating_policy(&preview, "fixture owner", now())
            .unwrap();
    }

    pub fn start(&self, task: &str) {
        assert!(matches!(
            corral_ops::dispatch(&self.ctx, task, &DispatchOpts::default()).unwrap(),
            DispatchOutcome::Started { .. }
        ));
    }

    pub fn done(&self, id: &str, file: &str) {
        let task = self.ctx.store.get(id).unwrap();
        let run = task.current_run().unwrap();
        std::fs::write(run.worktree.join(file), "verified fixture change\n").unwrap();
        git(&run.worktree, &["add", "-A"]);
        git(
            &run.worktree,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-qm",
                "fixture change",
            ],
        );
        let packet = corral_ops::progress_readback(&self.ctx, id).unwrap();
        corral_ops::save_checkpoint(
            &self.ctx,
            id,
            &corral_core::CheckpointUpdate {
                task: packet.task,
                run: packet.run,
                spec_revision: packet.spec_revision,
                expected_revision: packet
                    .checkpoints
                    .last()
                    .map_or(0, |entry| entry.checkpoint.revision),
                progress_revision: packet
                    .progress
                    .as_ref()
                    .map_or(0, |progress| progress.revision),
                head: packet.head,
                base: packet.base,
                tree: packet.tree,
                criterion_ids: vec![],
                evidence_refs: vec![],
                next_action: "Verify the candidate independently".into(),
                decisions: vec![],
                waits: vec![],
                resources: vec![],
            },
        )
        .unwrap();
        corral_ops::done(&self.ctx, id, "fixture implementation").unwrap();
    }

    pub fn records(&self, name: &str) -> Vec<String> {
        std::fs::read_to_string(self.bin.join(name))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    pub fn fresh(&self) -> Ctx {
        Ctx {
            store: Store::open(self.temp.path()).unwrap(),
            ..self.ctx.clone()
        }
    }
}
