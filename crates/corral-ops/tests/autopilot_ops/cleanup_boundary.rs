//! Actual retry -> durable intent -> external inventory -> truthful outcome, in private children.
use super::*;
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;

const TEST: &str =
    "cleanup_boundary::retirement_observes_durable_intent_and_records_only_real_failure";
const ROOT: &str = "CORRAL_RETRY_CLEANUP_BOUNDARY";

#[test]
fn retirement_observes_durable_intent_and_records_only_real_failure() {
    let Some(boundary) = std::env::var_os(ROOT).map(PathBuf::from) else {
        for fail in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let podman = dir.path().join("podman");
            std::fs::write(&podman, r#"#!/usr/bin/python3
import json, os, pathlib, subprocess, sys
root = pathlib.Path(__file__).parent
context = root / 'context.json'
if sys.argv[1] != 'ps' or not context.exists(): sys.exit(0)
env = dict(os.environ, CORRAL_RETRY_CLEANUP_OBSERVE='1')
result = subprocess.run([os.environ['CORRAL_RETRY_CLEANUP_EXE'], '--exact', os.environ['CORRAL_RETRY_CLEANUP_TEST'], '--nocapture'], env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
if result.returncode:
    sys.stderr.buffer.write(result.stdout + result.stderr)
    sys.exit(result.returncode)
assert (root / 'observed.json').is_file(), 'real observer must run'
if os.environ['CORRAL_RETRY_CLEANUP_FAIL'] == '1':
    sys.stderr.write('fixture observed reap failure: inventory unavailable\n')
    sys.exit(47)
"#).unwrap();
            std::fs::set_permissions(&podman, std::fs::Permissions::from_mode(0o700)).unwrap();
            let mut paths = vec![dir.path().to_owned()];
            paths.extend(std::env::split_paths(
                &std::env::var_os("PATH").unwrap_or_default(),
            ));
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", TEST, "--nocapture"])
                .env(ROOT, dir.path())
                .env("CORRAL_RETRY_CLEANUP_EXE", std::env::current_exe().unwrap())
                .env("CORRAL_RETRY_CLEANUP_TEST", TEST)
                .env("CORRAL_RETRY_CLEANUP_FAIL", if fail { "1" } else { "0" })
                .env_remove("CORRAL_RETRY_CLEANUP_OBSERVE")
                .env("PATH", std::env::join_paths(paths).unwrap())
                .stdin(std::process::Stdio::null())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "failure={fail}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let observed: Value =
                serde_json::from_slice(&std::fs::read(dir.path().join("observed.json")).unwrap())
                    .unwrap();
            assert_eq!(observed["outcome"], "superseded");
            assert_eq!(observed["cleanup_failed"], 0);
        }
        return;
    };
    if std::env::var_os("CORRAL_RETRY_CLEANUP_OBSERVE").is_some() {
        let context: Value =
            serde_json::from_slice(&std::fs::read(boundary.join("context.json")).unwrap()).unwrap();
        let store = corral_core::Store::open_unobserved(std::path::Path::new(
            context["root"].as_str().unwrap(),
        ))
        .unwrap();
        let task = store.get(context["task"].as_str().unwrap()).unwrap();
        let run = task
            .runs
            .iter()
            .find(|run| run.id == context["run"].as_str().unwrap())
            .unwrap();
        assert_eq!(task.status, Status::Ready);
        assert!(task.queued);
        assert_eq!(run.outcome, Some(corral_core::RunOutcome::Superseded));
        assert!(run.ended.is_some());
        let events = store.events().unwrap();
        assert!(
            events
                .iter()
                .any(|event| event.kind == "superseded" && event.task.as_deref() == Some(&task.id))
        );
        let failed = events
            .iter()
            .filter(|event| event.kind == "cleanup_failed" && event.detail["resource"] == "sandbox")
            .count();
        assert_eq!(
            failed, 0,
            "no failure diagnostic before the actual failing effect"
        );
        std::fs::write(
            boundary.join("observed.json"),
            serde_json::to_vec(&json!({
                "task": task.id, "outcome": run.outcome, "seq": store.view().unwrap().seq().0,
                "cleanup_failed": failed,
            }))
            .unwrap(),
        )
        .unwrap();
        return;
    }
    let mut fx = Fx::new(|_| {});
    fx.done();
    let run = fx.run().id;
    fx.ctx.store.config.dispatch.paused = true;
    fx.ctx.store.save_config().unwrap();
    std::fs::write(
        boundary.join("context.json"),
        serde_json::to_vec(&json!({
            "root": fx.ctx.store.root(), "task": fx.id, "run": run,
        }))
        .unwrap(),
    )
    .unwrap();
    let outcome =
        corral_ops::retry(&fx.ctx, &fx.id, &corral_core::ProfileChoice::default()).unwrap();
    assert!(matches!(outcome, DispatchOutcome::Queued { .. }));
    let events = fx.ctx.store.events().unwrap();
    let failures: Vec<_> = events
        .iter()
        .filter(|event| event.kind == "cleanup_failed" && event.detail["resource"] == "sandbox")
        .collect();
    let fail = std::env::var("CORRAL_RETRY_CLEANUP_FAIL").unwrap() == "1";
    assert_eq!(failures.len(), usize::from(fail));
    if fail {
        assert_eq!(failures[0].task.as_deref(), Some(fx.id.as_str()));
        assert!(
            failures[0].detail["error"]
                .as_str()
                .unwrap()
                .contains("fixture observed reap failure")
        );
    }
    assert!(boundary.join("observed.json").is_file());
}
