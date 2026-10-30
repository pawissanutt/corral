use super::*;
use corral_core::{RoleSession, Run, SessionAction, SessionPolicy, Status, Store};
use serde_json::Value;
use serde_json::json;

#[test]
fn show_reports_effective_role_policies_and_distinct_persisted_session_ids() {
    // Given independent stored conversations and a project-level worker policy.
    let ws = init_ws();
    let mut store = Store::open(&ws.root).unwrap();
    store
        .config
        .projects
        .get_mut("app")
        .unwrap()
        .review
        .worker_session = Some(SessionPolicy::Fresh);
    store.save_config().unwrap();
    let task = store
        .create("Session readback", Some("app"), Status::Running, "")
        .unwrap();
    let cwd = ws.root.join("app");
    let head = Command::new("git")
        .arg("-C")
        .arg(&cwd)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert!(head.status.success());
    let head = String::from_utf8(head.stdout).unwrap();
    let mut run: Run = serde_json::from_value(json!({
        "id":"R1", "agent":"generic", "worktree":cwd, "branch":"main", "base":"main",
        "base_sha":head.trim(), "started":"2026-10-26T00:00:00Z"
    }))
    .unwrap();
    for reviewer in [false, true] {
        let id = if reviewer {
            "reviewer-context"
        } else {
            "worker-context"
        };
        let directory = ws.root.join(id);
        std::fs::create_dir_all(&directory).unwrap();
        let transcript = directory.join("conversation.jsonl");
        std::fs::write(
            &transcript,
            format!("{}\n", json!({"type":"session", "id":id, "cwd":cwd})),
        )
        .unwrap();
        let session = RoleSession {
            policy: if reviewer {
                SessionPolicy::Reuse
            } else {
                SessionPolicy::Fresh
            },
            id: Some(id.into()),
            transcript: Some(transcript),
            directory,
            harness: "generic".into(),
            adapter: None,
            model: None,
            vendor: None,
            action: if reviewer {
                SessionAction::Resumed
            } else {
                SessionAction::New
            },
            diagnostic: None,
        };
        if reviewer {
            let mut child = run.clone();
            child.id = "R1-review1".into();
            child.session = Some(session);
            run.review_runs.push(child);
        } else {
            run.session = Some(session);
        }
    }
    store
        .update(&task.id, |task| {
            task.runs.push(run);
            Ok(())
        })
        .unwrap();

    // When another CLI process reads the task.
    let output = ok(&ws.root, &["show", &task.id, "--json"]);
    let value: Value = serde_json::from_str(&output).unwrap();

    // Then effective policy and the actual role identities survive process boundaries.
    assert_eq!(
        value["session_policy"],
        json!({"worker":"fresh", "reviewer":"reuse"})
    );
    assert_eq!(value["runs"][0]["session"]["id"], "worker-context");
    assert_eq!(
        value["runs"][0]["review_runs"][0]["session"]["id"],
        "reviewer-context"
    );
    assert_eq!(
        value["runs"][0]["review_runs"][0]["session"]["action"],
        "resumed"
    );
}
