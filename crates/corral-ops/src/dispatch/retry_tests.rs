//! A head's history-free host projection must not turn an earlier hello into a pre-exec refusal.
use super::*;
use serde_json::json;
use std::path::Path;
use std::process::{Command, Stdio};

fn git(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    String::from_utf8(output.stdout).unwrap().trim().into()
}

#[test]
fn refusal_bypass_reads_canonical_generations_and_requires_an_unchanged_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(tmp.path()).unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "base",
        ],
    );
    let base = git(&repo, &["rev-parse", "HEAD"]);
    let task = store
        .create("Refused launch", None, Status::NeedsMe, "")
        .unwrap();
    let mut run: corral_core::Run = serde_json::from_value(json!({
        "id":"R1", "agent":"fixture", "harness":"corral-pi", "worktree":repo,
        "branch":"fixture", "base":"main", "base_sha":base,
        "started":"2026-11-02T00:00:00Z", "outcome":"failed",
        "blocked_excerpt":"corral agent launch refused: memory_contract_unavailable: fixture",
        "agent_host": {"version":1, "binding": {"role":"worker", "store":"fixture",
            "project":"demo", "generation":2, "health":"healthy"},
            "generations":[
                {"n":1, "bind_nonce_sha256":"fixture", "pane":"old", "created":"2026-11-02T00:00:00Z"},
                {"n":2, "bind_nonce_sha256":"fixture", "pane":"new", "created":"2026-11-02T00:01:00Z"}
            ]}
    })).unwrap();
    let mut task = store
        .update(&task.id, |task| {
            task.runs.push(run.clone());
            Ok(())
        })
        .unwrap();
    let head = store.head(&task.id).unwrap();
    assert!(launch_refused_before_start(&head, head.current_run.as_deref().unwrap()).unwrap());
    // Generation 2 is unbound, but generation 1 actually started. The shell omits both rows.
    run.agent_host.as_mut().unwrap().generations[0].bound = Some(
        serde_json::from_value(json!({
            "host_cap_sha256":"fixture", "ctl_cap_sha256":"fixture", "session_id":"fixture",
            "session_file":"fixture.jsonl", "pid":1, "start_time":"fixture", "request_id":"hello",
            "at":"2026-11-02T00:00:00Z"
        }))
        .unwrap(),
    );
    task.runs[0] = run.clone();
    store.fixture_put_task(&task).unwrap();
    let head = store.head(&task.id).unwrap();
    assert!(
        head.shell_task().unwrap().runs[0]
            .agent_host
            .as_ref()
            .unwrap()
            .generations
            .is_empty()
    );
    assert!(!launch_refused_before_start(&head, head.current_run.as_deref().unwrap()).unwrap());
    run.agent_host.as_mut().unwrap().generations[0].bound = None;
    task.runs[0] = run;
    store.fixture_put_task(&task).unwrap();
    let head = store.head(&task.id).unwrap();
    std::fs::write(repo.join("uncommitted"), "work must not be discarded").unwrap();
    assert!(!launch_refused_before_start(&head, head.current_run.as_deref().unwrap()).unwrap());
    git(&repo, &["add", "."]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "work",
        ],
    );
    assert!(!launch_refused_before_start(&head, head.current_run.as_deref().unwrap()).unwrap());
}
