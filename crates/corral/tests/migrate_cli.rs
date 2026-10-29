//! `corral migrate` through the binary: JSON reports, and `kill -9` at named barriers (the
//! debug build stops at `CORRAL_MIGRATE_BARRIER` and announces it on stderr) followed by
//! `--status` and a rerun, including the composed B20 sequence (plan T-253 section 12).

#[path = "../../corral-core/tests/migrate_support/mod.rs"]
mod support;

use std::io::{BufRead, BufReader};
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use support::{FENCE, Legacy, NewArtifact};

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

/// `list` through the v2 binary: its refusal names `expected`, or it succeeds when `None`.
fn opener(env: &Env, legacy: &Legacy, expected: Option<&str>, at: &str) {
    let out = env.run(legacy, &["list"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    match expected {
        None => assert!(out.status.success(), "{at}: {stderr}"),
        Some(text) => assert!(
            !out.status.success() && stderr.contains(text),
            "{at}: expected a refusal naming {text:?}: {stderr}"
        ),
    }
}

/// The legacy store is whole again: original fence, no journal or database directory, the
/// legacy bytes of `before`, and the v2 binary refusing it in favour of `corral migrate`.
fn restored(
    env: &Env,
    legacy: &Legacy,
    before: &std::collections::BTreeMap<String, Vec<u8>>,
    at: &str,
) {
    for name in ["db", "db.migrating", "db.discard"] {
        assert!(!legacy.exists(name), "{at}: {name} remains");
    }
    assert_eq!(legacy.fence().as_deref(), Some(FENCE), "{at}");
    assert!(legacy.journal().is_none(), "{at}");
    assert_eq!(&legacy.legacy_bytes(), before, "{at}");
    assert_eq!(env.status(legacy)["state"], "legacy", "{at}");
    opener(env, legacy, Some("run `corral migrate`"), at);
    legacy.invariant();
}

#[test]
fn kill_9_at_forward_barriers_then_status_and_rerun_publish() {
    for at in ["B1", "B2", "B3", "B4", "B5", "B6", "B7"] {
        // Given an apply killed with SIGKILL at the barrier.
        let env = Env::new();
        let legacy = Legacy::new();
        let before = legacy.legacy_bytes();
        env.kill_at(&legacy, &["migrate", "--apply"], at);
        // Then --status names the state and changes nothing; the invariant holds, the legacy
        // files are unchanged and the v2 binary refuses the store mid-migration.
        let status = env.status(&legacy);
        assert_eq!(env.status(&legacy), status, "{at}");
        assert_eq!(status["problems"], Value::Array(Vec::new()), "{at}");
        legacy.invariant();
        assert_eq!(legacy.legacy_bytes(), before, "{at}");
        if at != "B7" {
            opener(&env, &legacy, Some("corral migrate"), at);
        }
        // When the apply reruns, it publishes and storage check passes.
        env.ok(&legacy, &["migrate", "--apply"]);
        assert_eq!(env.status(&legacy)["state"], "published", "{at}");
        env.ok(&legacy, &["storage", "check"]);
        opener(&env, &legacy, None, at);
        legacy.invariant();
    }
}

#[test]
fn kill_9_inside_published_rollback_then_rerun_restores_the_legacy_store() {
    // (barrier, state after the kill, fence after the kill)
    for (at, state, fence) in [
        ("B8", "published", "4\n"),
        ("B9", "rolling_back", "4\n"),
        ("B10", "rolling_back", "4\n"),
        ("B11", "rolling_back", "4\n"),
        ("B14", "rolling_back", "4\n"),
        ("B12", "rolling_back", FENCE),
        ("B13", "rolling_back", FENCE),
    ] {
        // Given a published store whose rollback is killed with SIGKILL at the barrier.
        let env = Env::new();
        let legacy = Legacy::new();
        let before = legacy.legacy_bytes();
        env.ok(&legacy, &["migrate", "--apply"]);
        env.kill_at(&legacy, &["migrate", "--rollback"], at);
        // Then --status names the state and step, the fence is whole and as tabled, and the
        // v2 binary refuses every rolling_back step naming --rollback.
        let status = env.status(&legacy);
        assert_eq!(status["state"], state, "{at}");
        assert_eq!(status["problems"], Value::Array(Vec::new()), "{at}");
        assert_eq!(legacy.fence().as_deref(), Some(fence), "{at}");
        legacy.invariant();
        if state == "rolling_back" {
            opener(&env, &legacy, Some("corral migrate --rollback"), at);
        }
        // When it reruns, the legacy store is restored with the database archived.
        env.ok(&legacy, &["migrate", "--rollback"]);
        restored(&env, &legacy, &before, at);
    }
}

#[test]
fn kill_9_inside_pre_publish_rollback_then_rerun_restores_the_legacy_store() {
    for at in ["B15", "B16", "B17", "B24"] {
        // Given an apply killed while importing, whose rollback is killed at the barrier.
        let env = Env::new();
        let legacy = Legacy::new();
        let before = legacy.legacy_bytes();
        env.kill_at(&legacy, &["migrate", "--apply"], "B3");
        env.kill_at(&legacy, &["migrate", "--rollback"], at);
        // Then --status names rolling_back with no problems and the v2 binary refuses it.
        let status = env.status(&legacy);
        assert_eq!(status["state"], "rolling_back", "{at}");
        assert_eq!(status["problems"], Value::Array(Vec::new()), "{at}");
        legacy.invariant();
        opener(&env, &legacy, Some("corral migrate --rollback"), at);
        // When it reruns, the partial import is discarded and the legacy store restored.
        env.ok(&legacy, &["migrate", "--rollback"]);
        restored(&env, &legacy, &before, at);
    }
}

#[test]
fn composed_sequences_b20_to_b26_kill_rollback_write_and_reapply() {
    // (name, apply barrier, candidate after it, rollback barrier killed again)
    for (name, first, candidate, again) in [
        ("B20", "B6", "db", None),
        ("B21", "B6", "db", Some("B15")),
        ("B22", "B6", "db", Some("B16")),
        ("B23", "B6", "db", Some("B17")),
        ("B24", "B6", "db", Some("B24")),
        ("B25", "B6", "db", Some("B12")),
        ("B26", "B5", "db.migrating", None),
    ] {
        // Given an apply killed with SIGKILL after verification.
        let env = Env::new();
        let legacy = Legacy::new();
        let before = legacy.legacy_bytes();
        env.kill_at(&legacy, &["migrate", "--apply"], first);
        // Then --status reports verified with the candidate's location and both next commands,
        // and the v2 binary refuses the store naming both.
        let status = env.status(&legacy);
        assert_eq!(status["state"], "verified", "{name}");
        assert_eq!(status["candidate"], candidate, "{name}");
        let next = status["next"].to_string();
        assert!(
            next.contains("--apply") && next.contains("--rollback"),
            "{name}: {next}"
        );
        opener(&env, &legacy, Some("corral migrate --rollback"), name);
        // When it is rolled back (killed once more inside the rollback, then rerun).
        if let Some(again) = again {
            env.kill_at(&legacy, &["migrate", "--rollback"], again);
            assert_eq!(env.status(&legacy)["state"], "rolling_back", "{name}");
            legacy.invariant();
            opener(&env, &legacy, Some("corral migrate --rollback"), name);
        }
        env.ok(&legacy, &["migrate", "--rollback"]);
        // Then the legacy store is whole, with db, db.migrating and db.discard absent.
        restored(&env, &legacy, &before, name);
        // And a 0.3.x write followed by a re-apply publishes a database holding that write.
        legacy.legacy_comment("T-002-backlog.md", "after the rollback");
        env.ok(&legacy, &["migrate", "--apply"]);
        env.ok(&legacy, &["storage", "check"]);
        let shown: Value =
            serde_json::from_str(&env.ok(&legacy, &["show", "T-002", "--json"])).unwrap();
        assert_eq!(shown["comments"][0]["text"], "after the rollback", "{name}");
    }
}

#[test]
fn kill_9_inside_finalize_then_rerun_finalizes() {
    for at in ["B18", "B19"] {
        // Given a published store whose finalize is killed with SIGKILL at the barrier (B19
        // holds the storage-gc claim and has deleted the first legacy path).
        let env = Env::new();
        let legacy = Legacy::new();
        env.ok(&legacy, &["migrate", "--apply"]);
        env.kill_at(&legacy, &["migrate", "--finalize", "--yes"], at);
        // Then --status reports finalizing with no problems and names finalize as next.
        let status = env.status(&legacy);
        assert_eq!(status["state"], "finalizing", "{at}");
        assert_eq!(status["problems"], Value::Array(Vec::new()), "{at}");
        assert!(status["next"].to_string().contains("--finalize"), "{at}");
        legacy.invariant();
        // When finalize reruns, it takes over the dead process's claim and finishes.
        env.ok(&legacy, &["migrate", "--finalize", "--yes"]);
        assert_eq!(env.status(&legacy)["state"], "finalized", "{at}");
        assert!(
            !legacy.exists("tasks") && !legacy.exists("events.jsonl"),
            "{at}"
        );
        assert_eq!(legacy.fence().as_deref(), Some("4\n"), "{at}");
        env.ok(&legacy, &["storage", "check"]);
        legacy.invariant();
    }
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

/// `(refs, unreferenced)` of the blob holding `content` in the published database.
fn blob_refs(legacy: &Legacy, content: &[u8]) -> Option<(u64, bool)> {
    use corral_core::backend::{BlobQuery, PageReq, StoreBackend};
    use sha2::{Digest, Sha256};
    let hash =
        corral_core::blob_ref::BlobHash::parse(&format!("{:x}", Sha256::digest(content))).unwrap();
    let backend = corral_core::sqlite::SqliteBackend::open(
        &legacy.corral,
        corral_core::sqlite::SqliteOptions::default(),
    )
    .unwrap();
    let query = BlobQuery {
        hash: Some(hash),
        state: None,
    };
    let page = backend
        .view()
        .unwrap()
        .blobs(&query, PageReq::first(4))
        .unwrap();
    page.items
        .first()
        .map(|row| (row.refs, row.unreferenced_since.is_some()))
}

#[test]
fn transcript_retention_after_migration_and_finalize_frees_every_copy() {
    // Given a Done task's transcript artifact with a legacy review copy, under a one-day
    // transcript retention.
    let env = Env::new();
    let legacy = Legacy::new();
    let transcript = b"agent transcript line\n".repeat(32);
    legacy.register_artifact(NewArtifact {
        task_file: "T-001-finished-work.md",
        kind: "log",
        file: "agent.transcript.log",
        content: &transcript,
        stored: Some(&transcript),
        compressed: false,
        purged: false,
    });
    legacy.put(
        "state/review-watch/T-001-R1/agent.transcript.log",
        &transcript,
    );
    let config = legacy.corral.join("corral.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    let text = if text.contains("[storage]\n") {
        text.replacen(
            "[storage]\n",
            "[storage]\ntranscript_retention_days = 1\n",
            1,
        )
    } else {
        text + "\n[storage]\ntranscript_retention_days = 1\n"
    };
    std::fs::write(&config, text).unwrap();
    // When it is migrated and finalized, and artifact retention runs.
    env.ok(&legacy, &["migrate", "--apply"]);
    env.ok(&legacy, &["migrate", "--finalize", "--yes"]);
    assert_eq!(blob_refs(&legacy, &transcript), Some((1, false)));
    let gc = env.ok(&legacy, &["artifact", "gc", "--yes", "--json"]);
    assert!(gc.contains("\"transcript\""), "{gc}");
    // Then nothing references the transcript's content any more (its grace clock runs), while
    // the report artifact keeps its reference.
    assert_eq!(blob_refs(&legacy, &transcript), Some((0, true)));
    assert_eq!(blob_refs(&legacy, support::REPORT), Some((1, false)));
}
