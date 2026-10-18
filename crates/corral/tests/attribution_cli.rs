use std::path::Path;
use std::process::{Command, Stdio};

use corral_core::{Actor, Event, Run, Status, Store};
use serde_json::json;

struct Fixture {
    dir: tempfile::TempDir,
    store: Store,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::init(dir.path()).unwrap();
        store.create("Move me", None, Status::Backlog, "").unwrap();
        Self { dir, store }
    }

    fn command(&self, cwd: &Path, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_corral"));
        for (key, _) in std::env::vars_os() {
            let name = key.to_string_lossy();
            if name.starts_with("CORRAL_") || name.starts_with("HERDR_") {
                cmd.env_remove(key);
            }
        }
        cmd.env("CORRAL_ROOT", self.dir.path())
            .env("HERDR_BIN_PATH", self.dir.path().join("no-herdr"))
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null());
        cmd
    }

    fn run(&self, cwd: &Path, args: &[&str]) {
        let output = self.command(cwd, args).output().unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn event(&self, kind: &str) -> Event {
        self.store
            .events()
            .unwrap()
            .into_iter()
            .rev()
            .find(|event| event.kind == kind)
            .unwrap()
    }

    fn pending_config_edit(&self, key: &str, value: &str) {
        let path = self.store.dir().join("corral.toml");
        let mut doc = corral_core::ConfigDoc::load(&path).unwrap();
        doc.set_str(key, value).unwrap();
        std::fs::write(path, doc.to_string()).unwrap();
    }

    fn run_worktree(&self, run_id: &str) -> std::path::PathBuf {
        let repo = self.dir.path().join("app");
        std::fs::create_dir(&repo).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@test.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "Initial",
            ],
            vec![
                "worktree",
                "add",
                "-q",
                "-b",
                "corral/T-001",
                ".worktrees/T-001",
            ],
        ] {
            let output = Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let worktree = repo.join(".worktrees/T-001");
        let run: Run = serde_json::from_value(json!({
            "id": run_id, "agent": "generic", "worktree": worktree,
            "branch": "corral/T-001", "base": "main", "base_sha": "fixture",
            "started": "2026-10-18T00:00:00Z"
        }))
        .unwrap();
        self.store
            .update("1", |task| {
                task.runs.push(run);
                Ok(())
            })
            .unwrap();
        worktree
    }
}

#[test]
fn move_from_shell_stamps_owner_cli() {
    // Given a task and a shell outside any run.
    let fx = Fixture::new();
    // When the owner moves it through the CLI.
    fx.run(fx.dir.path(), &["move", "1", "ready"]);
    // Then the event identifies the owner and CLI.
    let event = fx.event("moved");
    assert_eq!(event.actor, Some(Actor::Owner));
    assert_eq!(event.via.as_deref(), Some("cli"));
}

#[test]
fn move_with_explicit_author_stamps_pm_cli() {
    // Given a task in a shell.
    let fx = Fixture::new();
    // When the command explicitly identifies the PM.
    fx.run(fx.dir.path(), &["move", "1", "ready", "--author", "pm"]);
    // Then the author changes independently of the delivery surface.
    let event = fx.event("moved");
    assert_eq!(event.actor, Some(Actor::Pm));
    assert_eq!(event.via.as_deref(), Some("cli"));
}

#[test]
fn new_with_explicit_origin_stamps_pm_cli() {
    // Given a shell filing work for the PM.
    let fx = Fixture::new();
    // When the filing origin is explicit.
    fx.run(fx.dir.path(), &["new", "PM work", "--origin", "pm"]);
    // Then creation uses that origin as its actor.
    let event = fx.event("created");
    assert_eq!(event.actor, Some(Actor::Pm));
    assert_eq!(event.via.as_deref(), Some("cli"));
}

#[test]
fn move_from_run_worktree_stamps_agent_run_r1() {
    // Given a persisted R1 and a real git worktree, nested below its root.
    let fx = Fixture::new();
    let cwd = fx.run_worktree("R1").join("nested");
    std::fs::create_dir(&cwd).unwrap();
    // When an agent invokes the CLI without an author override.
    fx.run(&cwd, &["move", "1", "ready"]);
    // Then both the decision actor and its run surface are stamped.
    let event = fx.event("moved");
    assert_eq!(event.actor, Some(Actor::Agent));
    assert_eq!(event.via.as_deref(), Some("run:R1"));
}

#[test]
fn explicit_author_in_retry_worktree_preserves_actual_run_id() {
    // Given a later run using the task's worktree.
    let fx = Fixture::new();
    let cwd = fx.run_worktree("R2");
    // When an explicit owner decision is made from that run.
    fx.run(&cwd, &["move", "1", "ready", "--author", "owner"]);
    // Then explicit authors win without guessing R1 from the directory name.
    let event = fx.event("moved");
    assert_eq!(event.actor, Some(Actor::Owner));
    assert_eq!(event.via.as_deref(), Some("run:R2"));
}

#[test]
fn pm_pane_cli_move_stamps_pm_pm() {
    // Given a PM pane's inherited launch environment.
    let fx = Fixture::new();
    let mut cmd = fx.command(fx.dir.path(), &["move", "1", "ready"]);
    cmd.env("CORRAL_PM", "1");
    // When it invokes an ordinary CLI command.
    let output = cmd.output().unwrap();
    // Then the pane's actions identify the PM and PM surface.
    assert!(output.status.success(), "{output:?}");
    let event = fx.event("moved");
    assert_eq!(event.actor, Some(Actor::Pm));
    assert_eq!(event.via.as_deref(), Some("pm"));
}

#[test]
fn wait_pending_config_edit_stamps_owner_cli() {
    // Given a file edit which no command has observed yet.
    let fx = Fixture::new();
    fx.pending_config_edit("ui.mouse", "false");
    // When wait observes it at startup, before its immediate timeout.
    let output = fx
        .command(fx.dir.path(), &["wait", "--timeout", "0ms"])
        .output()
        .unwrap();
    // Then the newly produced config event identifies the shell invocation.
    assert_eq!(output.status.code(), Some(124), "{output:?}");
    let event = fx.event("config_changed");
    assert_eq!(event.detail["source"], "file_edit");
    assert_eq!(event.actor, Some(Actor::Owner));
    assert_eq!(event.via.as_deref(), Some("cli"));
}

#[test]
fn wait_pending_invalid_config_stamps_owner_cli() {
    // Given a parseable edit with an invalid running limit.
    let fx = Fixture::new();
    fx.pending_config_edit("limits.running", "0");
    // When wait observes the rejected configuration at startup.
    let output = fx
        .command(fx.dir.path(), &["wait", "--timeout", "0ms"])
        .output()
        .unwrap();
    // Then even the rejection event identifies the shell invocation.
    let event = fx.event("config_invalid");
    assert_eq!(event.detail["source"], "file_edit");
    assert_eq!(event.actor, Some(Actor::Owner));
    assert_eq!(event.via.as_deref(), Some("cli"));
    assert_eq!(output.status.code(), Some(124), "{output:?}");
}

#[test]
fn wait_pending_config_edit_from_pm_pane_stamps_pm_pm() {
    // Given an unobserved edit and the PM pane's launch environment.
    let fx = Fixture::new();
    fx.pending_config_edit("ui.mouse", "false");
    let mut cmd = fx.command(fx.dir.path(), &["wait", "--timeout", "0ms"]);
    cmd.env("CORRAL_PM", "1");
    // When the PM invokes wait.
    let output = cmd.output().unwrap();
    // Then its startup event retains the PM actor and surface.
    assert_eq!(output.status.code(), Some(124), "{output:?}");
    let event = fx.event("config_changed");
    assert_eq!(event.detail["source"], "file_edit");
    assert_eq!(event.actor, Some(Actor::Pm));
    assert_eq!(event.via.as_deref(), Some("pm"));
}

#[test]
fn wait_pending_config_edit_from_run_stamps_agent_run_r1() {
    // Given an unobserved edit and a persisted run with a real worktree.
    let fx = Fixture::new();
    let cwd = fx.run_worktree("R1");
    fx.pending_config_edit("ui.mouse", "false");
    // When the agent invokes wait from that worktree.
    let output = fx
        .command(&cwd, &["wait", "--timeout", "0ms"])
        .output()
        .unwrap();
    // Then the startup event retains the actual run identity.
    assert_eq!(output.status.code(), Some(124), "{output:?}");
    let event = fx.event("config_changed");
    assert_eq!(event.detail["source"], "file_edit");
    assert_eq!(event.actor, Some(Actor::Agent));
    assert_eq!(event.via.as_deref(), Some("run:R1"));
}
