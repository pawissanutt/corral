//! Gate runner against a fake herdr: `pane run` executes the command synchronously, so the gate result file
//! exists as soon as `gate_start` returns and no test waits on time.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use corral_core::{AgentCmd, ArtifactKind, GateResult, Status, Store};
use corral_ops::{Ctx, DispatchOpts, DispatchOutcome};

const FAKE_HERDR: &str = r#"#!/usr/bin/env bash
if [ "$1" = --session ]; then shift 2; fi
ok() { printf '{"id":"fake","result":%s}\n' "$1"; }
case "$1 $2" in
  "workspace create") ok '{"workspace":{"workspace_id":"w1"},"root_pane":{"workspace_id":"w1","tab_id":"w1:t1","pane_id":"w1:p1"}}' ;;
  "tab create") ok '{"root_pane":{"workspace_id":"w1","tab_id":"w1:t2","pane_id":"w1:p2"}}' ;;
  "workspace list") ok '{"workspaces":[{"workspace_id":"w1","label":"fake"}]}' ;;
  "pane run") bash -c "$4" >/dev/null 2>&1; ok '{}' ;;
  "pane get") ok "{\"pane\":{\"pane_id\":\"$3\",\"workspace_id\":\"w1\",\"tab_id\":\"w1:t1\"}}" ;;
  *) ok '{}' ;;
esac
"#;

fn fake_herdr() -> &'static PathBuf {
    static BIN: OnceLock<PathBuf> = OnceLock::new();
    BIN.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("corral-fake-herdr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("herdr");
        std::fs::write(&bin, FAKE_HERDR).unwrap();
        let st = Command::new("chmod").arg("+x").arg(&bin).status().unwrap();
        assert!(st.success());
        bin
    })
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
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
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A workspace with project "app", a fake agent, the given global gate, and one dispatched task.
fn running_task(gate: &[&str]) -> (tempfile::TempDir, Ctx, String, PathBuf) {
    unsafe {
        std::env::set_var("HERDR_BIN_PATH", fake_herdr());
        std::env::remove_var("CORRAL_HERDR_SESSION");
    }
    let tmp = tempfile::tempdir().unwrap();
    let app = tmp.path().join("app");
    std::fs::create_dir_all(&app).unwrap();
    git(&app, &["init", "-q", "-b", "main"]);
    git(&app, &["config", "user.name", "corral test"]);
    git(&app, &["config", "user.email", "corral@test.invalid"]);
    std::fs::write(app.join("README.md"), "hello\n").unwrap();
    git(&app, &["add", "-A"]);
    git(&app, &["commit", "-q", "-m", "init"]);
    let mut store = Store::init(tmp.path()).unwrap();
    store.config.agents.insert(
        "fake".into(),
        AgentCmd {
            command: "true".into(),
        },
    );
    store.config.review.gate = Some(gate.iter().map(|s| s.to_string()).collect());
    store.save_config().unwrap();
    let ctx = Ctx::from_store(store);
    let task = ctx
        .store
        .create("Gate me", Some("app"), Status::Ready, "")
        .unwrap();
    let opts = DispatchOpts {
        profile: Some("fake".into()),
        ..DispatchOpts::default()
    };
    let DispatchOutcome::Started { task, .. } =
        corral_ops::dispatch(&ctx, &task.id, &opts).unwrap()
    else {
        panic!("expected Started");
    };
    let worktree = task.current_run().unwrap().worktree.clone();
    (tmp, ctx, task.id, worktree)
}

fn gate_log(ctx: &Ctx, id: &str) -> (String, String) {
    let task = ctx.store.get(id).unwrap();
    let a = task
        .artifacts
        .iter()
        .rev()
        .find(|a| a.kind == ArtifactKind::Log)
        .expect("gate log artifact");
    let text = std::fs::read_to_string(ctx.store.dir().join(&a.path)).unwrap();
    (a.title.clone(), text)
}

#[test]
fn passing_gate_records_pass_with_sha_and_log() {
    let (_tmp, ctx, id, worktree) = running_task(&["echo hello-gate", "true"]);
    let head = git(&worktree, &["rev-parse", "HEAD"]);

    let started = corral_ops::gate_start(&ctx, &id).unwrap();
    assert_eq!(started.result, GateResult::Running);
    assert_eq!(
        started.pane.as_deref(),
        Some("w1:p2"),
        "gate opens a tab in the run workspace"
    );
    assert_eq!(started.sha, head);

    let record = corral_ops::gate_collect(&ctx, &id)
        .unwrap()
        .expect("finished");
    assert_eq!(record.result, GateResult::Pass);
    assert_eq!(record.failed_cmd, None);
    assert_eq!(record.sha, head);
    assert!(record.passed_at(&head));
    let task = ctx.store.get(&id).unwrap();
    assert_eq!(task.current_run().unwrap().gate.as_ref(), Some(&record));

    let (title, log) = gate_log(&ctx, &id);
    assert_eq!(title, "Gate: pass");
    assert!(log.contains("$ echo hello-gate\nhello-gate\n"), "{log}");
    assert!(log.contains("corral gate: pass"), "{log}");
    assert!(
        ctx.store
            .state_dir()
            .join(format!("gates/{id}-R1.json"))
            .is_file()
    );

    assert_eq!(corral_ops::gate_wait(&ctx, &id).unwrap(), record);
    let events: Vec<String> = ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert_eq!(
        events.iter().filter(|k| *k == "gate").count(),
        1,
        "{events:?}"
    );
}

#[test]
fn failing_gate_records_the_failed_command_and_stops() {
    let (_tmp, ctx, id, _worktree) =
        running_task(&["true", "echo boom; exit 3", "echo should-not-run"]);
    corral_ops::gate_start(&ctx, &id).unwrap();
    let record = corral_ops::gate_wait(&ctx, &id).unwrap();
    assert_eq!(record.result, GateResult::Fail);
    assert_eq!(record.failed_cmd.as_deref(), Some("echo boom; exit 3"));

    let (title, log) = gate_log(&ctx, &id);
    assert_eq!(title, "Gate: fail");
    assert!(log.contains("boom"), "{log}");
    assert!(!log.contains("should-not-run"), "{log}");
}

#[test]
fn gate_result_is_keyed_to_the_commit() {
    let (_tmp, ctx, id, worktree) = running_task(&["true"]);
    corral_ops::gate_start(&ctx, &id).unwrap();
    let first = corral_ops::gate_wait(&ctx, &id).unwrap();

    std::fs::write(worktree.join("new.txt"), "x\n").unwrap();
    let err = corral_ops::gate_start(&ctx, &id).unwrap_err();
    assert!(
        format!("{err:#}").contains("uncommitted changes"),
        "{err:#}"
    );

    git(&worktree, &["add", "-A"]);
    git(&worktree, &["commit", "-q", "-m", "more"]);
    let head = git(&worktree, &["rev-parse", "HEAD"]);
    assert!(
        !first.passed_at(&head),
        "a new commit invalidates the old pass"
    );

    corral_ops::gate_start(&ctx, &id).unwrap();
    let second = corral_ops::gate_wait(&ctx, &id).unwrap();
    assert_eq!(second.sha, head);
    assert!(second.passed_at(&head));
}

#[test]
fn gate_without_commands_is_an_error() {
    let (_tmp, ctx, id, _worktree) = running_task(&[]);
    let err = corral_ops::gate_start(&ctx, &id).unwrap_err();
    assert!(format!("{err:#}").contains("no gate commands"), "{err:#}");
    assert!(
        ctx.store
            .get(&id)
            .unwrap()
            .current_run()
            .unwrap()
            .gate
            .is_none()
    );
}
