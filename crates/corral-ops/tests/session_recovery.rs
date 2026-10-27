//! Public request-changes recovery regressions, using only private file-backed herdr.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use std::sync::atomic::{AtomicUsize, Ordering};

use corral_core::{HerdrRef, RunOutcome, SessionPolicy, Status, Store, Task};
use corral_herdr::Herdr;
use corral_ops::Ctx;

use corral_ops::resources::{FixedResourceProbe, ResourceProbe, ResourceSnapshot};
use serde_json::Value;

use serde_json::json;

mod support;

/// Re-exec gives each test its own environment, including an unavailable OmO program.
/// No process-global environment mutation races with Rust's parallel test runner.
fn isolated(name: &str, body: impl FnOnce()) {
    if std::env::var("CORRAL_RECOVERY_CHILD").as_deref() == Ok(name) {
        body();
        return;
    }
    let private = tempfile::tempdir().unwrap();
    let output = support::command(std::env::current_exe().unwrap(), private.path())
        .args(["--exact", name, "--nocapture"])
        .env("CORRAL_RECOVERY_CHILD", name)
        .env("CORRAL_OMO_PROGRAM", private.path().join("unavailable-omo"))
        .env("HOME", private.path())
        .env("XDG_CONFIG_HOME", private.path().join("config"))
        .env("XDG_DATA_HOME", private.path().join("data"))
        .env("XDG_STATE_HOME", private.path().join("state"))
        .env("XDG_CACHE_HOME", private.path().join("cache"))
        .env("HERDR_SOCKET_PATH", private.path().join("fake.sock"))
        .env_remove("HERDR_SESSION")
        .env_remove("HERDR_CONFIG_PATH")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "corral recovery test")
        .env("GIT_AUTHOR_EMAIL", "recovery@test.invalid")
        .env("GIT_COMMITTER_NAME", "corral recovery test")
        .env("GIT_COMMITTER_EMAIL", "recovery@test.invalid")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "isolated {name} exited {:?}\n{}{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

struct Fixture {
    tmp: tempfile::TempDir,
    ctx: Ctx,
    id: String,
}

impl Fixture {
    fn new(live: bool, policy: SessionPolicy) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let app = root.join("app");
        std::fs::create_dir(&app).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["commit", "-q", "--allow-empty", "-m", "recovery fixture"],
        ] {
            let output = support::command("git", root)
                .arg("-C")
                .arg(&app)
                .args(args)
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
        }
        let mut store = Store::init(root).unwrap();
        store.config.review.worker_session = Some(policy);
        store.config.agents.insert(
            "probe".into(),
            corral_core::AgentCmd {
                command: "true {model_args} {prompt_ref}".into(),
            },
        );
        store.save_config().unwrap();
        let task = store
            .create(
                "Recover worker",
                Some("app"),
                Status::Review,
                "Recover context",
            )
            .unwrap();
        let directory = root.join("conversation");
        std::fs::create_dir(&directory).unwrap();
        let transcript = directory.join("exact.jsonl");
        std::fs::write(
            &transcript,
            format!("{}\n", json!({"type":"session", "id":"worker", "cwd":app})),
        )
        .unwrap();
        let engine = root.join("herdr");
        std::fs::write(&engine, include_str!("session_recovery_engine.py")).unwrap();
        std::fs::set_permissions(&engine, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(
            root.join("engine.json"),
            json!({
                "workspace":live, "next":0,
                "panes":if live { vec!["w2:p0"] } else { vec![] },
                "created":[], "launched":[], "commands":[], "closed":[]
            })
            .to_string(),
        )
        .unwrap();
        let herdr = Herdr::with_bin(engine, None);
        // Even a missing former workspace has a recorded launch-server identity.
        let reference = HerdrRef {
            workspace_id: if live { "w2" } else { "w1" }.into(),
            pane_id: if live { "w2:p0" } else { "w1:p0" }.into(),
            socket: Some(herdr.socket_path().unwrap()),
        };
        let run = serde_json::from_value(json!({
            "id":"R1", "agent":"probe", "harness":"probe", "worktree":app,
            "branch":"main", "base":"main", "base_sha":"HEAD",
            "started":"2026-10-26T00:00:00Z", "ended":"2026-10-26T01:00:00Z",
            "outcome":"done", "herdr":reference,
            "session":{"policy":"reuse", "id":"worker", "transcript":transcript,
                       "directory":directory, "harness":"probe", "adapter":"omo", "action":"new"}
        }))
        .unwrap();
        store
            .update(&task.id, |task| {
                task.runs.push(run);
                Ok(())
            })
            .unwrap();
        Self {
            tmp,
            ctx: Ctx {
                store,
                herdr,
                resource_probe: FixedResourceProbe::arc(),
            },
            id: task.id,
        }
    }

    fn engine(&self) -> Value {
        serde_json::from_slice(&std::fs::read(self.tmp.path().join("engine.json")).unwrap())
            .unwrap()
    }

    fn task(&self) -> Task {
        self.ctx.store.get(&self.id).unwrap()
    }

    fn receipt(&self) -> PathBuf {
        self.ctx
            .store
            .state_dir()
            .join("launch-exits")
            .join(format!("{}-R1.exited", self.id))
    }

    fn seed_receipt(&self) {
        std::fs::create_dir_all(self.receipt().parent().unwrap()).unwrap();
        std::fs::write(self.receipt(), b"37\n").unwrap();
    }

    fn assert_preserved(&self, before: &Task) {
        let engine = self.engine();
        let after = self.task();
        let receipt = std::fs::read(self.receipt());
        println!("refusal state: engine={engine}; receipt={receipt:?}");
        // One assertion reports all preservation failures, rather than hiding receipt loss
        // behind the first pane-close failure.
        assert!(
            engine["panes"]
                .as_array()
                .unwrap()
                .contains(&json!("w2:p0"))
                && !engine["closed"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("w2:p0"))
                && engine["launched"].as_array().unwrap().is_empty()
                && after == *before
                && receipt.as_deref().ok() == Some(b"37\n".as_slice()),
            "refused changes must preserve original live worker, saved session identity, \
             task state, and existing exit receipt; engine={engine}; \
             before={before:?}; after={after:?}; receipt={receipt:?}"
        );
    }
}

#[derive(Debug)]
struct AfterPreparation {
    marker: PathBuf,
    healthy_samples: AtomicUsize,
    held_samples: AtomicUsize,
}

impl ResourceProbe for AfterPreparation {
    fn sample(&self, _: &Store) -> anyhow::Result<ResourceSnapshot> {
        let mut snapshot = FixedResourceProbe::healthy().0;
        if self.marker.is_file() {
            self.held_samples.fetch_add(1, Ordering::SeqCst);
            snapshot.load_per_core = 100.0;
            snapshot.load_one = 100.0;
        } else {
            self.healthy_samples.fetch_add(1, Ordering::SeqCst);
        }
        Ok(snapshot)
    }
}

#[test]
fn legacy_replacement_workspace_worker_survives_immediate_round_cleanup() {
    isolated(
        "legacy_replacement_workspace_worker_survives_immediate_round_cleanup",
        || {
            // Given a completed legacy worker whose original workspace disappeared.
            let fx = Fixture::new(false, SessionPolicy::Reuse);
            let before = fx.task();
            // When the public changes API recovers its recorded conversation.
            let changed = corral_ops::request_changes(&fx.ctx, &fx.id, "repair").unwrap();
            // Then the launched replacement remains live after round cleanup.
            let run = changed.current_run().unwrap();
            let pane = &run.herdr.as_ref().unwrap().pane_id;
            let engine = fx.engine();
            println!(
                "legacy recovery: status={:?}; pane={pane}; engine={engine}",
                changed.status
            );
            assert_eq!(changed.status, Status::Running);
            assert_eq!(
                run.session.as_ref().unwrap().id,
                before.current_run().unwrap().session.as_ref().unwrap().id
            );
            assert_eq!(engine["launched"], json!([pane]));
            assert!(
                engine["panes"].as_array().unwrap().contains(&json!(pane))
                    && !engine["closed"].as_array().unwrap().contains(&json!(pane)),
                "round cleanup killed replacement worker {pane}: {engine}"
            );
        },
    );
}

#[test]
fn recovered_inbox_worker_survives_its_following_changes_round() {
    isolated(
        "recovered_inbox_worker_survives_its_following_changes_round",
        || {
            // Given an inbox worker recovered into a replacement workspace.
            let fx = Fixture::new(false, SessionPolicy::Reuse);
            fx.ctx
                .store
                .update(&fx.id, |task| {
                    task.current_run_mut().unwrap().delivery =
                        corral_core::inbox::DeliveryMode::Inbox;
                    Ok(())
                })
                .unwrap();
            let first = corral_ops::request_changes(&fx.ctx, &fx.id, "first repair").unwrap();
            let run = first.current_run().unwrap();
            let pane = run.herdr.as_ref().unwrap().pane_id.clone();
            let session = run.session.clone().unwrap();
            assert!(fx.ctx.herdr.pane_get(&pane).unwrap().is_some());
            assert_eq!(fx.engine()["closed"], json!([]));
            fx.ctx
                .store
                .update(&fx.id, |task| {
                    task.status = Status::Review;
                    task.current_run_mut().unwrap().outcome = Some(RunOutcome::Done);
                    Ok(())
                })
                .unwrap();
            // When its following changes round retains the recovered worker.
            let changed = corral_ops::request_changes(&fx.ctx, &fx.id, "second repair").unwrap();
            // Then historical replacement-workspace ownership cannot close it.
            let engine = fx.engine();
            println!(
                "following inbox round: status={:?}; pane={pane}; engine={engine}",
                changed.status
            );
            assert_eq!(changed.status, Status::Running);
            assert_eq!(
                changed
                    .current_run()
                    .unwrap()
                    .herdr
                    .as_ref()
                    .unwrap()
                    .pane_id,
                pane
            );
            assert_eq!(
                changed.current_run().unwrap().session.as_ref().unwrap().id,
                session.id
            );
            assert_eq!(engine["launched"], json!([pane]));
            assert!(
                engine["panes"].as_array().unwrap().contains(&json!(pane))
                    && !engine["closed"].as_array().unwrap().contains(&json!(pane)),
                "following round cleanup killed recovered inbox worker {pane}: {engine}"
            );
        },
    );
}

#[test]
fn explicit_fresh_pending_reply_refusal_preserves_original_worker_and_receipt() {
    isolated(
        "explicit_fresh_pending_reply_refusal_preserves_original_worker_and_receipt",
        || {
            // Given a live worker, saved identity, receipt, and an undelivered reply.
            let fx = Fixture::new(true, SessionPolicy::Fresh);
            fx.ctx
                .store
                .update(&fx.id, |task| {
                    task.current_run_mut().unwrap().pending_reply = Some("previous request".into());
                    Ok(())
                })
                .unwrap();
            fx.seed_receipt();
            let before = fx.task();
            // When explicit fresh mode refuses a different pending request.
            let error =
                corral_ops::request_changes(&fx.ctx, &fx.id, "different request").unwrap_err();
            println!("pending refusal: {error:#}");
            assert!(error.to_string().contains("resend"), "{error:#}");
            // Then refusal has not destroyed the original process or its recovery evidence.
            fx.assert_preserved(&before);
        },
    );
}

#[test]
fn late_resource_refusal_preserves_original_worker_and_receipt() {
    isolated(
        "late_resource_refusal_preserves_original_worker_and_receipt",
        || {
            // Given healthy initial admission, with pressure triggered by pane preparation.
            let mut fx = Fixture::new(true, SessionPolicy::Fresh);
            fx.seed_receipt();
            let before = fx.task();
            let probe = Arc::new(AfterPreparation {
                marker: fx.tmp.path().join("replacement-prepared"),
                healthy_samples: AtomicUsize::new(0),
                held_samples: AtomicUsize::new(0),
            });
            fx.ctx.resource_probe = probe.clone();
            // When capacity changes synchronously after replacement pane creation.
            let error = corral_ops::request_changes(&fx.ctx, &fx.id, "repair").unwrap_err();
            println!("late refusal: {error:#}");
            assert!(error.to_string().contains("machine busy"), "{error:#}");
            assert!(probe.marker.is_file(), "replacement pane was not prepared");
            assert!(probe.healthy_samples.load(Ordering::SeqCst) > 0);
            assert!(probe.held_samples.load(Ordering::SeqCst) > 0);
            assert_eq!(fx.engine()["created"].as_array().unwrap().len(), 1);
            // Then the later transaction's refusal leaves original worker state intact.
            fx.assert_preserved(&before);
        },
    );
}
