use super::*;
use corral_core::{ProfileChoice, Status, Store};

fn fixture(harness: &str) -> (tempfile::TempDir, Ctx, Task, Run, ResolvedProfile) {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::init(tmp.path()).unwrap();
    let ctx = Ctx::from_store(store);
    let task = ctx
        .store
        .create("Session adapter", None, Status::Running, "")
        .unwrap();
    let mut run: Run = serde_json::from_value(json!({
        "id":"R1", "agent":harness, "worktree":tmp.path().join("worktree"),
        "branch":"corral/T-001", "base":"main", "base_sha":"base", "started":"2026-10-26T00:00:00Z"
    }))
    .unwrap();
    run.worktree = tmp.path().join("worktree");
    run.harness = Some(harness.into());
    run.agent = harness.into();
    run.model = None;
    run.vendor = None;
    let resolved = ctx
        .store
        .config
        .resolve_profile(
            &ProfileChoice {
                profile: Some(harness.into()),
                ..Default::default()
            },
            None,
            None,
        )
        .unwrap();
    run.session = Some(fresh(&ctx, &task, &run, false).unwrap());
    (tmp, ctx, task, run, resolved)
}

#[test]
fn omo_and_omp_resume_exact_paths_without_a_newest_session_selector() {
    for (harness, selector) in [("omo", "--session"), ("omp", "--resume")] {
        // Given one selected conversation and a newer sibling that must not be selected.
        let (_tmp, ctx, task, mut run, resolved) = fixture(harness);
        let dir = &run.session.as_ref().unwrap().directory;
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("exact.jsonl");
        std::fs::write(
            &path,
            format!(
                "{}\n",
                json!({
                    "type":"session", "id":"exact", "cwd":run.worktree,
                })
            ),
        )
        .unwrap();
        let mut session = capture(&ctx, &task, &run, false).unwrap();
        std::fs::write(dir.join("newer.jsonl"), "{}\n").unwrap();
        session.action = SessionAction::Resumed;
        run.session = Some(session);

        // When the concrete adapter renders the continuation.
        let rendered = command(&ctx, &run, &resolved).unwrap();

        // Then its machine selector names the recorded file, not the newer sibling.
        assert!(
            rendered
                .command
                .contains(&format!("{selector} '{}'", path.display()))
        );
        assert!(!rendered.command.contains("--continue"));
        assert!(!rendered.command.contains("newer.jsonl"));
    }
}

#[test]
fn claude_uses_a_new_exact_uuid_then_resumes_that_uuid() {
    // Given a reserved Claude identity in a private fixture directory.
    let (tmp, ctx, task, mut run, resolved) = fixture("claude");
    let selected = run.session.as_mut().unwrap();
    let id = selected.id.clone().unwrap();
    selected.directory = tmp.path().join("claude");
    selected.transcript = Some(selected.directory.join(format!("{id}.jsonl")));
    let new = command(&ctx, &run, &resolved).unwrap();
    assert!(new.command.contains(&format!("--session-id '{id}'")));
    std::fs::create_dir_all(&run.session.as_ref().unwrap().directory).unwrap();
    let path = run.session.as_ref().unwrap().transcript.as_ref().unwrap();
    std::fs::write(
        path,
        format!(
            "{}\n",
            json!({
                "type":"user", "sessionId":id, "cwd":run.worktree,
            })
        ),
    )
    .unwrap();
    let mut session = capture(&ctx, &task, &run, false).unwrap();
    session.action = SessionAction::Resumed;
    run.session = Some(session);

    // When the process restarts.
    let resumed = command(&ctx, &run, &resolved).unwrap();

    // Then the recorded UUID is resumed without forking or allocating another identity.
    assert!(resumed.command.contains(&format!("--resume '{id}'")));
    assert!(!resumed.command.contains("--session-id"));
}

#[test]
fn session_identity_cwd_and_provider_mismatches_are_refused() {
    for mismatch in ["identity", "cwd", "provider"] {
        // Given a durable role binding whose source becomes incompatible.
        let (_tmp, ctx, task, mut run, _) = fixture("omo");
        let dir = &run.session.as_ref().unwrap().directory;
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("exact.jsonl");
        std::fs::write(
            &path,
            format!(
                "{}\n",
                json!({
                    "type":"session", "id":"exact", "cwd":run.worktree,
                })
            ),
        )
        .unwrap();
        run.session = Some(capture(&ctx, &task, &run, false).unwrap());
        match mismatch {
            "identity" => run.session.as_mut().unwrap().id = Some("foreign".into()),
            "cwd" => run.worktree = Path::new("/another-task").to_path_buf(),
            "provider" => run.vendor = Some("another-provider".into()),
            _ => unreachable!(),
        }

        // When the binding is recaptured.
        let result = capture(&ctx, &task, &run, false);

        // Then no replacement conversation is silently selected.
        assert!(result.is_err(), "{mismatch}");
    }
}

#[test]
fn resumed_sessions_refuse_a_different_effective_launch_profile() {
    // Given an exact conversation recorded for the original resolved provider.
    let (_tmp, ctx, task, mut run, mut resolved) = fixture("omo");
    let directory = &run.session.as_ref().unwrap().directory;
    std::fs::create_dir_all(directory).unwrap();
    std::fs::write(
        directory.join("exact.jsonl"),
        format!(
            "{}\n",
            json!({
                "type":"session", "id":"exact", "cwd":run.worktree
            })
        ),
    )
    .unwrap();
    let mut session = capture(&ctx, &task, &run, false).unwrap();
    session.action = SessionAction::Resumed;
    run.session = Some(session);
    resolved.vendor = Some("another-provider".into());

    // When a changed legacy profile attempts to reopen it.
    let result = command(&ctx, &run, &resolved);

    // Then it is refused before an adapter can transfer the conversation.
    assert!(result.is_err());
}
