use super::*;
use corral_core::{Status, TaskRef as _};
use serde_json::json;
use std::path::Path;
fn git(path: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().into()
}

#[test]
fn board_repair_adopts_exact_committed_seed_without_assistant_or_worker_session() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-qb", "main"]);
    git(&repo, &["config", "user.name", "test"]);
    git(&repo, &["config", "user.email", "test@test.invalid"]);
    std::fs::write(repo.join("base"), "base").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-qm", "base"]);
    let base = git(&repo, &["rev-parse", "HEAD"]);
    let source_path = tmp.path().join("source");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-qb",
            "source",
            source_path.to_str().unwrap(),
            "main",
        ],
    );
    std::fs::write(source_path.join("candidate"), "preserved work").unwrap();
    git(&source_path, &["add", "."]);
    git(&source_path, &["commit", "-qm", "candidate"]);
    let candidate = git(&source_path, &["rev-parse", "HEAD"]);
    let store = Store::init(tmp.path()).unwrap();
    let mut task = store
        .create("continue", None, Status::Review, "scope")
        .unwrap();
    let old: Run = serde_json::from_value(json!({"id":"R1","agent":"assistant-profile","worktree":source_path,
        "branch":"source","base":"main","base_sha":base,"started":task.created,"outcome":"superseded"})).unwrap();
    task.runs.push(old.clone());
    task.cowork = Some(serde_json::from_value(json!({"session":"A-999","scratch":"work","run":"R1",
        "review":"completion_v1","owner_confirmed":true,"completion":{"version":1,"contract_revision":task.contract_revision().unwrap(),
        "intent":"ship","driver":{"kind":"board"},"instruction":"take over"}})).unwrap());
    let next = Run {
        id: "R2".into(),
        agent: "normal-worker-profile".into(),
        outcome: None,
        worktree: tmp.path().join("next"),
        branch: "next".into(),
        ..old
    };
    let frozen = source(&task, &next).unwrap().unwrap();
    stage(&store, &task.id, &next.id, &frozen).unwrap();
    store
        .update(&task.id, |current| {
            current.cowork = task.cowork.clone();
            current.runs = vec![task.runs[0].clone(), next.clone()];
            Ok(())
        })
        .unwrap();
    let head = store.head(&task.id).unwrap();
    let run = RunHead::of(&next).unwrap();
    std::fs::remove_dir_all(&source_path).unwrap();
    let seed = start_head(&store, &head, &run).unwrap();
    assert_eq!(seed, candidate);
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-qb",
            "next",
            next.worktree.to_str().unwrap(),
            &seed,
        ],
    );
    assert_eq!(
        std::fs::read_to_string(next.worktree.join("candidate")).unwrap(),
        "preserved work"
    );
    store
        .delete_record(AREA, &format!("{}-R2", task.id))
        .unwrap();
    assert!(
        start_head(&store, &store.head(&task.id).unwrap(), &run).is_err(),
        "missing seed must never fall back to an empty base"
    );
}
