//! `corral migrate` through the binary: JSON reports, and `kill -9` at named barriers (the
//! debug build stops at `CORRAL_MIGRATE_BARRIER` and announces it on stderr) followed by
//! `--status` and a rerun, including the composed B20 sequence (plan T-253 section 12).

#[path = "../../corral-core/tests/migrate_support/mod.rs"]
mod support;

use std::io::{BufRead, BufReader};
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use support::{FENCE, Legacy};

struct Env {
    home: tempfile::TempDir,
}

impl Env {
    fn new() -> Env {
        Env {
            home: tempfile::tempdir().unwrap(),
        }
    }

    fn cmd(&self, legacy: &Legacy) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_corral"));
        let home = self.home.path();
        cmd.arg("--root")
            .arg(&legacy.root)
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_CACHE_HOME", home.join("cache"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("XDG_STATE_HOME", home.join("state"))
            .env_remove("CORRAL_ROOT")
            .env_remove("CORRAL_MIGRATE_BARRIER")
            .stdin(Stdio::null());
        cmd
    }

    fn run(&self, legacy: &Legacy, args: &[&str]) -> Output {
        self.cmd(legacy).args(args).output().unwrap()
    }

    fn ok(&self, legacy: &Legacy, args: &[&str]) -> String {
        let out = self.run(legacy, args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn status(&self, legacy: &Legacy) -> Value {
        serde_json::from_str(&self.ok(legacy, &["migrate", "--status", "--json"])).unwrap()
    }

    /// Runs `args` until the process announces barrier `at`, then kills it with SIGKILL.
    fn kill_at(&self, legacy: &Legacy, args: &[&str], at: &str) {
        let mut child = self
            .cmd(legacy)
            .args(args)
            .env("CORRAL_MIGRATE_BARRIER", at)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stderr = BufReader::new(child.stderr.take().unwrap());
        let wanted = format!("corral-migrate-barrier {at}");
        let mut seen = String::new();
        loop {
            let mut line = String::new();
            let n = stderr.read_line(&mut line).unwrap();
            assert!(n > 0, "{args:?} exited before barrier {at}: {seen}");
            if line.trim_end() == wanted {
                break;
            }
            seen.push_str(&line);
        }
        child.kill().unwrap();
        let status = child.wait().unwrap();
        assert!(!status.success(), "{at}: the process was not killed");
    }
}

#[test]
fn dry_run_json_lists_blockers_and_refuses() {
    // Given a legacy board with an open coding run.
    let env = Env::new();
    let legacy = Legacy::new();
    std::fs::write(
        legacy.corral.join("tasks/T-009-open.md"),
        "---\nschema_version: 2\ntask:\n  id: T-009\n  title: Open\n  status: running\n  priority: P2\n  created: 2026-10-04T00:00:00Z\n  updated: 2026-10-04T00:00:00Z\n  runs:\n  - id: R1\n    agent: claude\n    worktree: /tmp/w\n    branch: corral/T-009\n    base: main\n    base_sha: '00'\n    started: 2026-10-04T00:00:00Z\n    herdr:\n      workspace_id: w5\n      pane_id: w5:p1\n---\nBody\n",
    )
    .unwrap();
    // When the dry run reports in JSON.
    let out = env.run(&legacy, &["migrate", "--dry-run", "--json"]);
    // Then it exits non-zero listing the blocker with task, run and pane; nothing is created.
    assert!(!out.status.success());
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    let blocker = &report["blockers"][0];
    assert_eq!(blocker["family"], "open_coding_run");
    assert_eq!(
        (&blocker["task"], &blocker["run"], &blocker["pane"]),
        (
            &Value::from("T-009"),
            &Value::from("R1"),
            &Value::from("w5:p1")
        )
    );
    assert_eq!(report["history"][0]["family"], "process_lost_gate");
    assert!(legacy.journal().is_none());
    assert_eq!(legacy.fence().as_deref(), Some(FENCE));
}

#[test]
fn kill_9_at_forward_barriers_then_status_and_rerun_publish() {
    for at in ["B2", "B3", "B5"] {
        // Given an apply killed with SIGKILL at the barrier.
        let env = Env::new();
        let legacy = Legacy::new();
        let before = legacy.legacy_bytes();
        env.kill_at(&legacy, &["migrate", "--apply"], at);
        // Then --status names the state and changes nothing; the invariant holds.
        let status = env.status(&legacy);
        assert_eq!(env.status(&legacy), status, "{at}");
        assert_eq!(status["problems"], Value::Array(Vec::new()), "{at}");
        legacy.invariant();
        assert_eq!(legacy.legacy_bytes(), before, "{at}");
        // When the apply reruns, it publishes and storage check passes.
        env.ok(&legacy, &["migrate", "--apply"]);
        assert_eq!(env.status(&legacy)["state"], "published", "{at}");
        env.ok(&legacy, &["storage", "check"]);
        legacy.invariant();
    }
}

#[test]
fn kill_9_inside_published_rollback_then_rerun_restores_the_legacy_store() {
    for at in ["B10", "B14"] {
        // Given a published store whose rollback is killed with SIGKILL at the barrier.
        let env = Env::new();
        let legacy = Legacy::new();
        let before = legacy.legacy_bytes();
        env.ok(&legacy, &["migrate", "--apply"]);
        env.kill_at(&legacy, &["migrate", "--rollback"], at);
        // Then --status names rolling_back and its step, and the fence is whole.
        let status = env.status(&legacy);
        assert_eq!(status["state"], "rolling_back", "{at}");
        assert_eq!(legacy.fence().as_deref(), Some("4\n"), "{at}");
        legacy.invariant();
        // When it reruns, the legacy store is restored with the database archived.
        env.ok(&legacy, &["migrate", "--rollback"]);
        assert_eq!(legacy.fence().as_deref(), Some(FENCE), "{at}");
        assert!(legacy.journal().is_none());
        assert_eq!(legacy.legacy_bytes(), before);
        assert_eq!(env.status(&legacy)["state"], "legacy");
    }
}

#[test]
fn composed_b20_kill_after_the_rename_rollback_write_and_reapply() {
    // Given an apply killed with SIGKILL after A6's rename (B6).
    let env = Env::new();
    let legacy = Legacy::new();
    let before = legacy.legacy_bytes();
    env.kill_at(&legacy, &["migrate", "--apply"], "B6");
    // Then --status reports verified with the candidate at db and both next commands.
    let status = env.status(&legacy);
    assert_eq!(status["state"], "verified");
    assert_eq!(status["candidate"], "db");
    let next = status["next"].to_string();
    assert!(
        next.contains("--apply") && next.contains("--rollback"),
        "{next}"
    );
    // When it is rolled back (killed once more inside the rollback, B16, and rerun).
    env.kill_at(&legacy, &["migrate", "--rollback"], "B16");
    env.ok(&legacy, &["migrate", "--rollback"]);
    // Then db, db.migrating and db.discard are absent, the fence is original, no journal.
    for name in ["db", "db.migrating", "db.discard"] {
        assert!(!legacy.exists(name), "{name}");
    }
    assert_eq!(legacy.fence().as_deref(), Some(FENCE));
    assert!(legacy.journal().is_none());
    assert_eq!(legacy.legacy_bytes(), before);
    // And a 0.3.x write followed by a re-apply publishes a database holding that write.
    legacy.legacy_comment("T-002-backlog.md", "after the rollback");
    env.ok(&legacy, &["migrate", "--apply"]);
    env.ok(&legacy, &["storage", "check"]);
    let shown: Value =
        serde_json::from_str(&env.ok(&legacy, &["show", "T-002", "--json"])).unwrap();
    assert_eq!(shown["comments"][0]["text"], "after the rollback");
}

#[test]
fn finalize_requires_yes_and_an_action_is_required() {
    // Given a published store.
    let env = Env::new();
    let legacy = Legacy::new();
    env.ok(&legacy, &["migrate", "--apply"]);
    // When finalize runs without --yes, or migrate runs without an action, they refuse.
    let out = env.run(&legacy, &["migrate", "--finalize"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--yes"));
    assert!(!env.run(&legacy, &["migrate"]).status.success());
    // Then the store stays published.
    assert_eq!(env.status(&legacy)["state"], "published");
}
