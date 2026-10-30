//! Post-land hooks that call corral during sync (T-509).

use std::process::{Command, Stdio};

use corral_core::{PostLandHook, Status};
use corral_ops::{HookReceipt, HookState, PostLandOperation};

#[test]
fn post_land_hook_running_corral_completes_during_sync() {
    // Given a landed post-land operation whose hook runs the candidate corral binary to
    // attach an artifact, which needs artifact storage, against this disposable store.
    let f = super::queue_fixture();
    let task = f.task("app", Status::Review);
    let root = f.temp.path();
    let repo = root.join("app");
    let head = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["rev-parse", "HEAD"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let head = String::from_utf8(head.stdout).unwrap().trim().to_owned();
    let note = root.join("nested-note.txt");
    std::fs::write(&note, "written by a post-land hook\n").unwrap();
    let hook = PostLandHook {
        name: "nested-corral".into(),
        argv: [
            env!("CARGO_BIN_EXE_corral"),
            "artifact",
            "add",
            &task.id,
            &note.display().to_string(),
            "--kind",
            "log",
            "--title",
            "Nested post-land artifact",
        ]
        .map(String::from)
        .into(),
        cwd: ".".into(),
        env: [("CORRAL_ROOT".to_owned(), root.display().to_string())].into(),
        // A hook that hangs against its parent sync is killed here: bounded failure.
        timeout_secs: 30,
        max_attempts: 1,
        identity: None,
    };
    let op = PostLandOperation {
        version: 1,
        key: "t509-nested-corral".into(),
        project: "app".into(),
        repo: repo.clone(),
        task: task.id.clone(),
        run: "R1".into(),
        base: "main".into(),
        base_before: "0".repeat(40),
        landed_sha: head,
        policy_digest: "fixture".into(),
        created: jiff::Timestamp::now(),
        landed: true,
        abandoned: false,
        notified: false,
        notice: None,
        hooks: vec![HookReceipt {
            hook,
            state: HookState::Pending,
            attempts: Vec::new(),
            reason: None,
        }],
    };
    let dir = f.ctx.store.state_dir().join("post-land");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{}.json", op.key)),
        serde_json::to_vec_pretty(&op).unwrap(),
    )
    .unwrap();

    // When a real sync process reconciles the operation.
    f.ok(&["sync"]);

    // Then the nested corral command completed: the hook succeeded and its artifact exists.
    let ops = corral_ops::post_land_operations(&f.ctx, Some("app")).unwrap();
    assert!(ops[0].ready(), "{:?}", ops[0].hooks[0]);
    let task = f.ctx.store.get(&task.id).unwrap();
    assert!(
        task.artifacts
            .iter()
            .any(|a| a.title == "Nested post-land artifact"),
        "{:?}",
        task.artifacts
    );
}
