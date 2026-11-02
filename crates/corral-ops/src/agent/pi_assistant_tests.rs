//! Supervisor transitions, independent of provider calls and the owner's terminal.
use std::os::unix::process::ExitStatusExt;

use super::*;
use crate::agent::bridge::fixture::{self as f, Fx};
use crate::agent::hooks::pm::tests::fixture;
use crate::agent::launch;
use crate::agent::ops::assistant;

const PANE: &str = "w1:p9";

fn setup() -> (Fx, Prepared) {
    let fx = fixture();
    std::fs::write(fx.root().join(PANE), "").unwrap();
    let role = fx.root().join("assets/src/roles/assistant");
    std::fs::create_dir_all(&role).unwrap();
    std::fs::write(role.join("index.ts"), "export {}").unwrap();
    let ctx = f::ctx_at(fx.root());
    let (session, prepared, _) = assistant::open(&ctx, PANE, &Overrides::default(), false).unwrap();
    assert_eq!(session.id, "A-1");
    (fx, prepared)
}

fn status(code: i32) -> std::process::ExitStatus {
    std::process::ExitStatus::from_raw(code << 8)
}

#[test]
fn new_suspends_the_old_identity_and_runs_a_fresh_assistant_in_the_same_pane() {
    let (fx, prepared) = setup();
    let ctx = f::ctx_at(fx.root());
    let mut launched = Vec::new();
    supervise_with(
        &ctx,
        PANE,
        "A-1".into(),
        prepared,
        &Overrides::default(),
        false,
        |prepared| {
            let bootstrap: launch::Bootstrap =
                serde_json::from_slice(&std::fs::read(&prepared.bootstrap).unwrap()).unwrap();
            assert_eq!(bootstrap.pane, PANE);
            assert_eq!(bootstrap.generation, 1);
            assert!(bootstrap.resume_session.is_none());
            assert!(bootstrap.prompt_file.is_none());
            launched.push(bootstrap.assistant.unwrap());
            Ok(status(if launched.len() == 1 {
                NEW_ASSISTANT_EXIT
            } else {
                0
            }))
        },
    )
    .unwrap();
    assert_eq!(launched, ["A-1", "A-2"]);
    let first = ctx.store.get_assistant("A-1").unwrap();
    assert!(first.ended.is_none());
    let container = ctx.store.get_assistant_container("A-1").unwrap();
    assert_eq!(
        container
            .state
            .generation(1)
            .unwrap()
            .fenced
            .as_ref()
            .unwrap()
            .reason,
        "new assistant"
    );
    assert!(ctx.store.get_assistant("A-2").unwrap().ended.is_some());
    assert!(
        ctx.store
            .get_assistant_container("A-2")
            .unwrap()
            .state
            .todo
            .is_none()
    );
    assert!(fx.calls("pane run").is_empty() && fx.calls("pane send").is_empty());
    // The suspended conversation remains resumable through the existing generation protocol.
    assistant::resume(&ctx, "A-1", PANE, &Overrides::default()).unwrap();
    assert_eq!(
        ctx.store
            .get_assistant_container("A-1")
            .unwrap()
            .state
            .binding
            .generation,
        2
    );
}

#[test]
fn failed_new_admission_leaves_the_old_conversation_open_with_a_resume_command() {
    let (fx, prepared) = setup();
    let ctx = f::ctx_at(fx.root());
    let mut calls = 0;
    let error = supervise_with(
        &ctx,
        PANE,
        "A-1".into(),
        prepared,
        &Overrides::default(),
        false,
        |_| {
            calls += 1;
            std::fs::remove_file(fx.root().join("assets/src/roles/assistant/index.ts")).unwrap();
            Ok(status(NEW_ASSISTANT_EXIT))
        },
    )
    .unwrap_err();
    assert_eq!(calls, 1);
    assert!(format!("{error:#}").contains("corral pi --resume A-1"));
    assert!(ctx.store.get_assistant("A-1").unwrap().ended.is_none());
    assert_eq!(ctx.store.list_assistants().unwrap().len(), 1);
}

#[test]
fn failed_new_host_ends_only_its_fresh_record_and_a_crash_is_not_a_new_request() {
    for second_start_fails in [false, true] {
        let (fx, prepared) = setup();
        let ctx = f::ctx_at(fx.root());
        let mut calls = 0;
        let result = supervise_with(
            &ctx,
            PANE,
            "A-1".into(),
            prepared,
            &Overrides::default(),
            false,
            |_| {
                calls += 1;
                if second_start_fails && calls == 1 {
                    Ok(status(NEW_ASSISTANT_EXIT))
                } else if second_start_fails {
                    anyhow::bail!("fixture spawn failed")
                } else {
                    Ok(status(2))
                }
            },
        );
        assert!(result.is_err());
        assert!(ctx.store.get_assistant("A-1").unwrap().ended.is_none());
        assert_eq!(ctx.store.list_assistants().unwrap().len(), calls);
        if second_start_fails {
            assert!(ctx.store.get_assistant("A-2").unwrap().ended.is_some());
        }
    }
}
