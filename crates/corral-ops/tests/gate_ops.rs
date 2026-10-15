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
    assert!(corral_ops::gate_paths(&ctx, &id, "R1").result.is_file());
    let identity = record.identity.as_ref().unwrap();
    assert_eq!(identity.task, id);
    assert_eq!(identity.run, "R1");
    assert_eq!(identity.head, head);
    assert_eq!(identity.tree, git(&worktree, &["rev-parse", "HEAD^{tree}"]));
    assert_eq!(record.commands.len(), 2);
    assert!(
        record
            .commands
            .iter()
            .all(|r| r.exit_code == 0 && r.log_end > r.log_start)
    );
    assert!(record.log_sha256.is_some());

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

#[test]
fn same_head_policy_edit_invalidates_previous_pass() {
    // Given a completed gate on the unchanged candidate.
    let (_tmp, ctx, id, worktree) = running_task(&["true"]);
    corral_ops::gate_start(&ctx, &id).unwrap();
    let first = corral_ops::gate_wait(&ctx, &id).unwrap();
    // When the effective recipe changes, even this already-open context must reload it.
    let mut store = Store::open(ctx.store.root()).unwrap();
    store.config.review.gate = Some(vec!["false".into()]);
    store.save_config().unwrap();
    let collected = corral_ops::gate_collect(&ctx, &id).unwrap().unwrap();
    // Then the old PASS cannot stand for the new policy.
    assert_eq!(git(&worktree, &["rev-parse", "HEAD"]), first.sha);
    assert_eq!(collected.result, GateResult::Fail);
    assert!(collected.failed_cmd.unwrap().starts_with("stale_result:"));
}

#[test]
fn stale_result_file_cannot_complete_a_new_attempt() {
    let (_tmp, ctx, id, _) = running_task(&["true"]);
    corral_ops::gate_start(&ctx, &id).unwrap();
    let old_path = corral_ops::gate_paths(&ctx, &id, "R1").result;
    let first = corral_ops::gate_wait(&ctx, &id).unwrap();
    corral_ops::gate_start(&ctx, &id).unwrap();
    let current_path = corral_ops::gate_paths(&ctx, &id, "R1").result;
    assert_ne!(old_path, current_path);
    let mut old: serde_json::Value =
        serde_json::from_slice(&std::fs::read(old_path).unwrap()).unwrap();
    let current: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&current_path).unwrap()).unwrap();
    // Isolate identity validation from the independent ownership check.
    old["owner_token"] = current["owner_token"].clone();
    std::fs::write(current_path, serde_json::to_vec(&old).unwrap()).unwrap();
    let result = corral_ops::gate_wait(&ctx, &id).unwrap();
    assert_ne!(
        first.identity.unwrap().attempt,
        result.identity.unwrap().attempt
    );
    assert_eq!(result.result, GateResult::Fail);
    assert!(result.failed_cmd.unwrap().starts_with("stale_result:"));
}

#[test]
fn changed_task_criterion_invalidates_inflight_result() {
    let (_tmp, ctx, id, _) = running_task(&["true"]);
    corral_ops::gate_start(&ctx, &id).unwrap();
    ctx.store
        .update(&id, |task| {
            task.body.push_str("\n- Additional required criterion\n");
            Ok(())
        })
        .unwrap();
    let result = corral_ops::gate_wait(&ctx, &id).unwrap();
    assert_eq!(result.result, GateResult::Fail);
    assert!(result.failed_cmd.unwrap().starts_with("stale_result:"));
}

#[test]
fn pipeline_failure_is_not_hidden_by_successful_last_command() {
    let (_tmp, ctx, id, _) = running_task(&["false | true"]);
    corral_ops::gate_start(&ctx, &id).unwrap();
    let result = corral_ops::gate_wait(&ctx, &id).unwrap();
    assert_eq!(result.result, GateResult::Fail);
    assert_eq!(result.commands[0].exit_code, 1);
}

#[test]
fn legacy_pass_is_removed_before_new_attempt_launch() {
    let (_tmp, ctx, id, _) = running_task(&["true"]);
    let legacy = ctx
        .store
        .state_dir()
        .join("gates")
        .join(format!("{id}-R1.json"));
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::write(&legacy, r#"{"result":"pass","exit_code":0}"#).unwrap();
    let mut store = Store::open(ctx.store.root()).unwrap();
    store.config.review.gate = Some(vec![format!("test ! -e '{}'", legacy.display())]);
    store.save_config().unwrap();

    corral_ops::gate_start(&ctx, &id).unwrap();

    // Pre-T075 collectors read only <task>-<run>.json. Even before their Task
    // decoder rejects the parent's v2 envelope, that path cannot supply a pass.
    let old_result = std::fs::read_to_string(&legacy)
        .ok()
        .map(|text| serde_json::from_str::<serde_json::Value>(&text).unwrap());
    assert!(old_result.is_none());
    assert_eq!(
        corral_ops::gate_wait(&ctx, &id).unwrap().result,
        GateResult::Pass
    );
}

#[test]
fn result_without_ownership_token_cannot_complete_attempt() {
    let (_tmp, ctx, id, _) = running_task(&["true"]);
    corral_ops::gate_start(&ctx, &id).unwrap();
    let path = corral_ops::gate_paths(&ctx, &id, "R1").result;
    let mut result: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    result.as_object_mut().unwrap().remove("owner_token");
    std::fs::write(&path, serde_json::to_vec(&result).unwrap()).unwrap();

    assert!(corral_ops::gate_collect(&ctx, &id).is_err());
    assert_eq!(
        ctx.store
            .get(&id)
            .unwrap()
            .current_run()
            .unwrap()
            .gate
            .as_ref()
            .unwrap()
            .result,
        GateResult::Running
    );
}

#[test]
fn wrong_ownership_token_cannot_complete_attempt() {
    let (_tmp, ctx, id, _) = running_task(&["true"]);
    corral_ops::gate_start(&ctx, &id).unwrap();
    let path = corral_ops::gate_paths(&ctx, &id, "R1").result;
    let mut result: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    // The public attempt ID, also inherited by nested tests, is not ownership.
    result["owner_token"] = result["identity"]["attempt"].clone();
    std::fs::write(&path, serde_json::to_vec(&result).unwrap()).unwrap();

    let error = corral_ops::gate_collect(&ctx, &id).unwrap_err();
    assert!(format!("{error:#}").contains("ownership token"));
    assert_eq!(
        ctx.store
            .get(&id)
            .unwrap()
            .current_run()
            .unwrap()
            .gate
            .as_ref()
            .unwrap()
            .result,
        GateResult::Running
    );
}

#[test]
fn valid_owner_uses_private_noninherited_per_attempt_secret() {
    use std::os::unix::fs::PermissionsExt;

    let (_tmp, ctx, id, _) = running_task(&[
        "test -n \"$CORRAL_GATE_ATTEMPT\"; test -z \"${owner_token+x}\"; test -z \"${CORRAL_GATE_OWNER_TOKEN+x}\"",
    ]);
    corral_ops::gate_start(&ctx, &id).unwrap();
    let path = corral_ops::gate_paths(&ctx, &id, "R1")
        .result
        .with_extension("owner");
    let token = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(token.len(), 64);
    let first = corral_ops::gate_wait(&ctx, &id).unwrap();
    assert_eq!(first.result, GateResult::Pass);
    assert_ne!(token, first.identity.unwrap().attempt);

    corral_ops::gate_start(&ctx, &id).unwrap();
    let next = corral_ops::gate_paths(&ctx, &id, "R1")
        .result
        .with_extension("owner");
    assert_ne!(std::fs::read_to_string(next).unwrap(), token);
    assert_eq!(
        corral_ops::gate_wait(&ctx, &id).unwrap().result,
        GateResult::Pass
    );
}

#[test]
fn old_format_result_at_attempt_path_is_refused() {
    let (_tmp, ctx, id, _) = running_task(&["true"]);
    corral_ops::gate_start(&ctx, &id).unwrap();
    let path = corral_ops::gate_paths(&ctx, &id, "R1").result;
    std::fs::write(path, r#"{"result":"pass","exit_code":0}"#).unwrap();

    assert!(corral_ops::gate_collect(&ctx, &id).is_err());
    assert_eq!(
        ctx.store
            .get(&id)
            .unwrap()
            .current_run()
            .unwrap()
            .gate
            .as_ref()
            .unwrap()
            .result,
        GateResult::Running
    );
}

#[test]
fn new_gate_can_replace_a_running_legacy_attempt_without_collecting_its_pass() {
    // Given: an upgraded store still has the old board's running, unbound gate.
    let (_tmp, ctx, id, _) = running_task(&["true"]);
    corral_ops::gate_start(&ctx, &id).unwrap();
    ctx.store
        .update(&id, |task| {
            task.current_run_mut()
                .unwrap()
                .gate
                .as_mut()
                .unwrap()
                .identity = None;
            Ok(())
        })
        .unwrap();
    let legacy = ctx
        .store
        .state_dir()
        .join("gates")
        .join(format!("{id}-R1.json"));
    std::fs::write(&legacy, r#"{"result":"pass","exit_code":0}"#).unwrap();
    // When
    let fresh = corral_ops::gate_start(&ctx, &id).unwrap();
    // Then
    assert!(fresh.identity.is_some());
    assert!(!legacy.exists());
    assert!(!ctx.store.events().unwrap().iter().any(|e| e.kind == "gate"));
    assert_eq!(
        corral_ops::gate_wait(&ctx, &id).unwrap().result,
        GateResult::Pass
    );
}

#[test]
fn inherited_attempt_cannot_start_or_forge_completion() {
    if let Some(root) = std::env::var_os("CORRAL_GATE_TEST_ROOT") {
        let ctx = Ctx::from_store(Store::open(Path::new(&root)).unwrap());
        let id = std::env::var("CORRAL_GATE_TEST_TASK").unwrap();
        let task = ctx.store.get(&id).unwrap();
        let before = task.current_run().unwrap().gate.clone();
        let error = corral_ops::gate_start(&ctx, &id).unwrap_err();
        assert!(format!("{error:#}").contains("nested gate"));
        let error = corral_ops::gate_collect(&ctx, &id).unwrap_err();
        assert!(format!("{error:#}").contains("ownership token"));
        assert_eq!(
            ctx.store.get(&id).unwrap().current_run().unwrap().gate,
            before
        );
        return;
    }
    let (_tmp, ctx, id, _) = running_task(&["true"]);
    let started = corral_ops::gate_start(&ctx, &id).unwrap();
    let path = corral_ops::gate_paths(&ctx, &id, "R1").result;
    let mut result: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let attempt = &started.identity.as_ref().unwrap().attempt;
    result["owner_token"] = serde_json::json!(attempt);
    std::fs::write(&path, serde_json::to_vec(&result).unwrap()).unwrap();
    let before = ctx.store.events().unwrap().len();

    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "inherited_attempt_cannot_start_or_forge_completion",
            "--nocapture",
        ])
        .env("CORRAL_GATE_TEST_ROOT", ctx.store.root())
        .env("CORRAL_GATE_TEST_TASK", &id)
        .env("CORRAL_GATE_ATTEMPT", attempt)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(ctx.store.events().unwrap().len(), before);
    assert_eq!(
        ctx.store.get(&id).unwrap().current_run().unwrap().gate,
        Some(started)
    );
}

#[test]
fn runtime_skew_refuses_gate_start_before_creating_gate_files() {
    let (_tmp, ctx, id, _) = running_task(&["true"]);
    std::fs::write(ctx.store.state_dir().join("schema-version"), "999\n").unwrap();

    let error = corral_ops::gate_start(&ctx, &id).unwrap_err();

    assert_eq!(error.to_string(), corral_core::UPDATE_NOTICE);
    assert!(!ctx.store.state_dir().join("gates").exists());
    let events = ctx.store.events().unwrap();
    assert_eq!(events.last().unwrap().kind, "version_skew");
    assert!(!events.iter().any(|event| event.kind == "gate_started"));
}

#[test]
fn runtime_skew_refuses_collecting_an_owned_result() {
    let (_tmp, ctx, id, _) = running_task(&["true"]);
    corral_ops::gate_start(&ctx, &id).unwrap();
    let task = ctx.store.get(&id).unwrap();
    let before = std::fs::read(&task.path).unwrap();
    std::fs::write(ctx.store.state_dir().join("schema-version"), "999\n").unwrap();

    let error = corral_ops::gate_collect(&ctx, &id).unwrap_err();

    assert_eq!(error.to_string(), corral_core::UPDATE_NOTICE);
    assert_eq!(std::fs::read(&task.path).unwrap(), before);
    let events = ctx.store.events().unwrap();
    assert_eq!(events.last().unwrap().kind, "version_skew");
    assert!(!events.iter().any(|event| event.kind == "gate"));
}

#[test]
fn passing_cargo_tests_do_not_override_all_sloc_violations() {
    let guard = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/check-candidate.py");
    let guard = format!("python3 '{}'", guard.display());
    let (_tmp, ctx, id, worktree) = running_task(&["RUSTC_WRAPPER= cargo test --offline", &guard]);
    std::fs::create_dir(worktree.join("src")).unwrap();
    std::fs::write(
        worktree.join("Cargo.toml"),
        "[package]\nname = \"sloc-fixture\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    std::fs::write(worktree.join(".gitignore"), "/target\n/Cargo.lock\n").unwrap();
    std::fs::write(worktree.join("src/lib.rs"), "pub mod a;\npub mod b;\n").unwrap();
    for (path, count) in [("a.rs", 801), ("b.rs", 802)] {
        let code = format!(
            "pub fn example() {{\n{}}}\n",
            "    let _ = 1;\n".repeat(count - 2)
        );
        std::fs::write(worktree.join("src").join(path), code).unwrap();
    }
    git(&worktree, &["add", "-A"]);
    git(
        &worktree,
        &["commit", "-q", "-m", "compilable oversized fixtures"],
    );
    corral_ops::gate_start(&ctx, &id).unwrap();
    let result = corral_ops::gate_wait(&ctx, &id).unwrap();
    assert_eq!(result.commands[0].exit_code, 0, "cargo tests pass");
    assert_eq!(result.result, GateResult::Fail);
    assert_eq!(result.failed_cmd.as_deref(), Some(guard.as_str()));
    let (_, log) = gate_log(&ctx, &id);
    let report: serde_json::Value = log
        .lines()
        .find_map(|line| serde_json::from_str(line).ok())
        .unwrap();
    let offenders = report["violations"].as_array().unwrap();
    assert_eq!(offenders.len(), 2, "{report}");
    assert_eq!(offenders[0]["path"], "src/a.rs");
    assert_eq!(offenders[0]["detail"], "801 > 800");
    assert_eq!(offenders[1]["path"], "src/b.rs");
    assert_eq!(offenders[1]["detail"], "802 > 800");
    println!(
        "SLOC_REJECTION_RECEIPT {}",
        serde_json::json!({"gate":result,"guard":report})
    );
}
