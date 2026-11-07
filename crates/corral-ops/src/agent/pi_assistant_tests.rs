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
    let (session, prepared, _) =
        assistant::open(&ctx, ctx.store.root(), PANE, &Overrides::default(), false).unwrap();
    assert_eq!(session.id, "A-1");
    (fx, prepared)
}

fn status(code: i32) -> std::process::ExitStatus {
    std::process::ExitStatus::from_raw(code << 8)
}

#[test]
fn clean_exit_suspends_and_resumes_until_explicitly_ended() {
    let (fx, prepared) = setup();
    let ctx = f::ctx_at(fx.root());
    let runtime = launch::assistant_runtime_dir(&ctx.store, "A-1").join("g1");
    let control = runtime.join(launch::CTL_CAP_FILE);
    let socket = runtime.join(launch::SOCKET_FILE);
    std::fs::write(&control, "fixture control credential").unwrap();
    std::fs::write(&socket, "fixture socket").unwrap();
    supervise_with(
        &ctx,
        PANE,
        assistant::claim(&ctx.store, "A-1").unwrap(),
        prepared,
        &Overrides::default(),
        false,
        |_| Ok(status(0)),
    )
    .unwrap();
    assert!(ctx.store.get_assistant("A-1").unwrap().ended.is_none());
    let container = ctx.store.get_assistant_container("A-1").unwrap();
    assert!(container.state.generation(1).unwrap().fenced.is_some());
    assert!(!control.exists() && !socket.exists());
    let (fenced, _) = assistant::resume(&ctx, "A-1", PANE, &Overrides::default()).unwrap();
    assert_eq!(fenced, 1);
    assert_eq!(
        ctx.store
            .get_assistant_container("A-1")
            .unwrap()
            .state
            .binding
            .generation,
        2
    );
    end_assistant(&ctx, "A-1").unwrap();
    finish(&ctx, "A-1", 2, status(0)).unwrap();
    assert!(ctx.store.get_assistant("A-1").unwrap().ended.is_some());
    assert!(assistant::resume(&ctx, "A-1", PANE, &Overrides::default()).is_err());
}

#[test]
fn new_suspends_the_old_identity_and_runs_a_fresh_assistant_in_the_same_pane() {
    let (fx, prepared) = setup();
    let ctx = f::ctx_at(fx.root());
    let mut launched = Vec::new();
    supervise_with(
        &ctx,
        PANE,
        assistant::claim(&ctx.store, "A-1").unwrap(),
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
    assert!(ctx.store.get_assistant("A-2").unwrap().ended.is_none());
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
        assistant::claim(&ctx.store, "A-1").unwrap(),
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
            assistant::claim(&ctx.store, "A-1").unwrap(),
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

#[test]
fn relink_resumes_the_same_assistant_file_in_the_same_pane() {
    // Given a generation whose Pi file has a conversation, and a newly created scratch.
    let (fx, prepared) = setup();
    let ctx = f::ctx_at(fx.root());
    let file = fx.root().join("same-session.jsonl");
    std::fs::write(&file, "{\"type\":\"session\",\"id\":\"same\"}\n").unwrap();
    let mut launched = Vec::new();
    let mut scratch_path = None;
    // When the host requests relink (76), then the supervisor reopens only this session.
    supervise_with(
        &ctx,
        PANE,
        assistant::claim(&ctx.store, "A-1").unwrap(),
        prepared,
        &Overrides::default(),
        false,
        |prepared| {
            let bootstrap: launch::Bootstrap =
                serde_json::from_slice(&std::fs::read(&prepared.bootstrap).unwrap()).unwrap();
            assert_eq!(bootstrap.assistant.as_deref(), Some("A-1"));
            assert_eq!(bootstrap.pane, PANE);
            launched.push(bootstrap.generation);
            if bootstrap.generation == 1 {
                ctx.store
                    .update_assistant_container("A-1", |container| {
                        container.state.generations[0].bound =
                            Some(corral_core::agent::GenerationBound {
                                host_cap_sha256: "host".into(),
                                ctl_cap_sha256: "ctl".into(),
                                session_id: "same".into(),
                                session_file: file.clone(),
                                pid: u32::MAX,
                                start_time: "gone".into(),
                                request_id: "hello".into(),
                                at: corral_core::clock::now(),
                                herdr_seq_epoch: None,
                            });
                        Ok(())
                    })
                    .unwrap();
                let scratch =
                    crate::assistant::scratch::create(&ctx, "A-1", "corral", Some("linked"), None)
                        .unwrap();
                scratch_path = Some(scratch.path);
                Ok(status(76))
            } else {
                assert_eq!(bootstrap.generation, 2);
                assert_eq!(bootstrap.resume_session.as_ref(), Some(&file));
                assert!(!bootstrap.fresh_session);
                assert_eq!(Some(&bootstrap.cwd), scratch_path.as_ref());
                assert_eq!(bootstrap.project_dir.as_ref(), scratch_path.as_ref());
                Ok(status(0))
            }
        },
    )
    .unwrap();
    assert_eq!(launched, [1, 2]);
    assert_eq!(ctx.store.list_assistants().unwrap().len(), 1);
    let container = ctx.store.get_assistant_container("A-1").unwrap();
    assert!(container.state.generations[0].fenced.is_some());
    assert!(fx.calls("pane run").is_empty() && fx.calls("pane send").is_empty());
}

#[test]
#[ignore = "private scripts/test-herdr.sh server and offline fixture assets required"]
fn assistant_project_private_host_relink() {
    use corral_core::Store;
    use serde_json::{Value, json};
    use std::io::Write;
    use std::process::{Command, Stdio};

    let Ok(input) = std::env::var("CORRAL_ASSISTANT_PROJECT_LIVE") else {
        return;
    };
    let input: Value = serde_json::from_str(&input).unwrap();
    assert_eq!(
        std::env::var("CORRAL_HERDR_SESSION").unwrap(),
        "corral-test"
    );
    let root = Path::new(input["root"].as_str().unwrap());
    let mut store = Store::init(root).unwrap();
    store.config.agent.assets_dir = Some(input["assets"].as_str().unwrap().into());
    store.config.assistant.agent = Some("owner".into());
    store.config.profiles.insert(
        "owner".into(),
        serde_json::from_value(json!({
            "harness":"corral-pi", "model":"gpt-6.1-sol", "vendor":"openai",
        }))
        .unwrap(),
    );
    store.config.projects.insert(
        "app".into(),
        serde_json::from_value(json!({"path":"app"})).unwrap(),
    );
    store.config.resources.max_load_per_core = 1.0e6;
    store.save_config().unwrap();
    let ctx = Ctx::from_store(Store::open(root).unwrap());
    if ctx.herdr.workspace_list().is_err() {
        eprintln!("assistant project live: skipped (private herdr unavailable)");
        return;
    }
    let pane = input["pane"].as_str().unwrap();
    let (session, prepared, _) =
        assistant::open(&ctx, &root.join("app"), pane, &Overrides::default(), false).unwrap();
    assert_eq!(session.project.as_deref(), Some("app"));
    let mut directories = Vec::new();
    supervise_with(
        &ctx,
        pane,
        assistant::claim(&ctx.store, &session.id).unwrap(),
        prepared,
        &Overrides::default(),
        false,
        |prepared| {
            let mut bootstrap = launch::consume_bootstrap(&prepared.bootstrap)?;
            // The test process is not a corral CLI executable; bind calls to the built candidate.
            bootstrap.corral = input["corral"].as_str().unwrap().into();
            let dir = prepared.bootstrap.parent().unwrap();
            let ready = launch::check(&bootstrap, dir, prepared.assets.as_deref())?;
            let secrets = launch::Secrets::generate()?;
            launch::write_ctl_cap(dir, &secrets.ctl_cap)?;
            let message = launch::launch_message(&bootstrap, &ready, &secrets)?;
            directories.push(bootstrap.cwd);
            let mut child = Command::new("bun")
                .arg(input["driver"].as_str().unwrap())
                .stdin(Stdio::piped())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()?;
            child.stdin.take().unwrap().write_all(&message)?;
            Ok(child.wait()?)
        },
    )
    .unwrap();
    let container = ctx.store.get_assistant_container(&session.id).unwrap();
    let first = container.state.generation(1).unwrap();
    let second = container.state.generation(2).unwrap();
    let files: Vec<_> = [first, second]
        .iter()
        .map(|g| g.bound.as_ref().unwrap().session_file.clone())
        .collect();
    assert_eq!(files[0], files[1]);
    assert_eq!(
        directories,
        [root.join("app"), root.join("app/.worktrees/A-1-linked")]
    );
    assert!(first.fenced.is_some());
    assert_eq!(ctx.store.list_assistants().unwrap().len(), 1);
    assert!(
        ctx.store
            .events()
            .unwrap()
            .iter()
            .any(|e| e.kind == "assistant_relinked")
    );
    println!(
        "ASSISTANT_PROJECT_LIVE {}",
        json!({
            "session": session.id, "project": container.state.binding.project, "generations": [1,2],
            "directories": directories, "session_files": files, "same_pane": container.pane,
            "provider": "offline fixture; production HostSession, CLI, store and supervisor",
        })
    );
}
