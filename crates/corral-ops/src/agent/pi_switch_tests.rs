//! `/resume` supervisor transitions (T-728): switch away and back, ineligible and claimed
//! targets, a target that cannot start, stale requests and clean quit, without a real host.
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};

use super::*;
use crate::agent::bridge::fixture::{self as f, Fx};
use crate::agent::hooks::pm::tests::fixture;
use crate::agent::launch;
use crate::agent::ops::assistant::{self, SWITCH_NOTICE_FILE, SWITCH_REQUEST_FILE};

const PANE: &str = "w1:p9";

/// A-1 (prepared, generation 1) and a stopped A-2 whose generation 1 recorded a conversation.
fn fixture_with_role() -> Fx {
    let fx = fixture();
    std::fs::write(fx.root().join(PANE), "").unwrap();
    let role = fx.root().join("assets/src/roles/assistant");
    std::fs::create_dir_all(&role).unwrap();
    std::fs::write(role.join("index.ts"), "export {}").unwrap();
    fx
}

fn setup() -> (Fx, Prepared, PathBuf, PathBuf) {
    let fx = fixture_with_role();
    let ctx = f::ctx_at(fx.root());
    let open = |ctx: &Ctx| {
        assistant::open(ctx, ctx.store.root(), PANE, &Overrides::default(), false).unwrap()
    };
    let (first, prepared, _) = open(&ctx);
    let (second, _, _) = open(&ctx);
    assert_eq!((first.id.as_str(), second.id.as_str()), ("A-1", "A-2"));
    let files = ["a1.jsonl", "a2.jsonl"].map(|name| {
        let file = fx.root().join(name);
        std::fs::write(&file, "{\"type\":\"session\"}\n").unwrap();
        file
    });
    bind(&ctx, "A-2", &files[1], None);
    let [a1, a2] = files;
    (fx, prepared, a1, a2)
}

/// Records hello on `id`'s current generation: a dead host unless `live` names this process.
fn bind(ctx: &Ctx, id: &str, file: &Path, live: Option<u32>) {
    let (pid, start_time) = match live {
        Some(pid) => (
            pid,
            crate::agent::effects::process_start(pid).unwrap().unwrap(),
        ),
        None => (u32::MAX, "gone".into()),
    };
    ctx.store
        .update_assistant_container(id, |container| {
            let generation = container.state.generations.last_mut().unwrap();
            generation.bound = Some(corral_core::agent::GenerationBound {
                host_cap_sha256: "host".into(),
                ctl_cap_sha256: "ctl".into(),
                session_id: id.into(),
                session_file: file.into(),
                pid,
                start_time,
                request_id: format!("hello-{id}-{}", generation.n),
                at: corral_core::clock::now(),
                herdr_seq_epoch: None,
            });
            Ok(())
        })
        .unwrap();
}

/// What the host's `/resume` picker leaves in its generation directory before exiting 77.
fn request(ctx: &Ctx, prepared: &Prepared, id: &str, generation: u32, target: &str) {
    let dir = launch::assistant_runtime_dir(&ctx.store, id).join(format!("g{generation}"));
    // The host's runtimeDir is the directory its bootstrap was written to.
    assert_eq!(prepared.bootstrap.parent(), Some(dir.as_path()));
    let body = serde_json::json!({"version": 1, "assistant": id, "generation": generation, "target": target});
    std::fs::write(dir.join(SWITCH_REQUEST_FILE), body.to_string()).unwrap();
}

fn status(code: i32) -> std::process::ExitStatus {
    std::process::ExitStatus::from_raw(code << 8)
}

fn bootstrap(prepared: &Prepared) -> launch::Bootstrap {
    serde_json::from_slice(&std::fs::read(&prepared.bootstrap).unwrap()).unwrap()
}

fn notice(prepared: &Prepared) -> String {
    std::fs::read_to_string(
        prepared
            .bootstrap
            .parent()
            .unwrap()
            .join(SWITCH_NOTICE_FILE),
    )
    .unwrap_or_default()
}

fn fence_reason(ctx: &Ctx, id: &str, generation: u32) -> Option<String> {
    let container = ctx.store.get_assistant_container(id).unwrap();
    container
        .state
        .generation(generation)?
        .fenced
        .as_ref()
        .map(|f| f.reason.clone())
}

#[test]
fn switch_away_and_return_resumes_each_conversation_under_its_own_identity() {
    let (fx, prepared, a1, a2) = setup();
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
            let b = bootstrap(prepared);
            let id = b.assistant.clone().unwrap();
            launched.push((id.clone(), b.generation, b.resume_session.clone()));
            match launched.len() {
                1 => {
                    bind(&ctx, "A-1", &a1, None);
                    request(&ctx, prepared, "A-1", 1, "A-2");
                    Ok(status(RESUME_ASSISTANT_EXIT))
                }
                2 => {
                    // While A-2 runs, nobody else can claim it, but A-1 is free to return to.
                    assert!(assistant::claim(&ctx.store, "A-2").is_err());
                    drop(assistant::claim(&ctx.store, "A-1").unwrap());
                    bind(&ctx, "A-2", &a2, None);
                    request(&ctx, prepared, "A-2", 2, "A-1");
                    Ok(status(RESUME_ASSISTANT_EXIT))
                }
                _ => Ok(status(0)),
            }
        },
    )
    .unwrap();
    assert_eq!(
        launched,
        [
            ("A-1".into(), 1, None),
            ("A-2".into(), 2, Some(a2.clone())),
            ("A-1".into(), 2, Some(a1.clone())),
        ]
    );
    for id in ["A-1", "A-2"] {
        assert!(ctx.store.get_assistant(id).unwrap().ended.is_none(), "{id}");
    }
    // Every left generation is fenced, so a stale host's calls are refused `agent_fenced`.
    assert_eq!(
        fence_reason(&ctx, "A-1", 1).as_deref(),
        Some("resume another assistant")
    );
    assert_eq!(
        fence_reason(&ctx, "A-2", 2).as_deref(),
        Some("resume another assistant")
    );
    assert_eq!(fence_reason(&ctx, "A-1", 2).as_deref(), Some("quit"));
    let events = ctx.store.events().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "assistant_switched")
            .count(),
        2
    );
    assert!(fx.calls("pane run").is_empty() && fx.calls("pane send").is_empty());
}

#[test]
fn ineligible_stale_or_claimed_targets_return_to_the_source_with_a_notice() {
    let cases = ["ended", "missing", "live", "current", "stale", "claimed"];
    for case in cases {
        let (fx, prepared, a1, _) = setup();
        let ctx = f::ctx_at(fx.root());
        let _held = (case == "claimed").then(|| assistant::claim(&ctx.store, "A-2").unwrap());
        match case {
            "ended" => drop(ctx.store.end_assistant("A-2").unwrap()),
            "live" => bind(
                &ctx,
                "A-2",
                &fx.root().join("a2.jsonl"),
                Some(std::process::id()),
            ),
            _ => {}
        }
        let mut calls = 0;
        supervise_with(
            &ctx,
            PANE,
            assistant::claim(&ctx.store, "A-1").unwrap(),
            prepared,
            &Overrides::default(),
            false,
            |prepared| {
                calls += 1;
                let b = bootstrap(prepared);
                assert_eq!(b.assistant.as_deref(), Some("A-1"), "{case}");
                if calls == 1 {
                    bind(&ctx, "A-1", &a1, None);
                    let (target, generation) = match case {
                        "missing" => ("A-99", 1),
                        "current" => ("A-1", 1),
                        "stale" => ("A-2", 7),
                        _ => ("A-2", 1),
                    };
                    let dir = prepared.bootstrap.parent().unwrap();
                    let body = serde_json::json!({"version": 1, "assistant": "A-1",
                        "generation": generation, "target": target});
                    std::fs::write(dir.join(SWITCH_REQUEST_FILE), body.to_string()).unwrap();
                    return Ok(status(RESUME_ASSISTANT_EXIT));
                }
                assert_eq!(
                    (b.generation, b.resume_session.as_ref()),
                    (2, Some(&a1)),
                    "{case}"
                );
                let text = notice(prepared);
                assert!(
                    text.contains("/resume did not switch") && text.contains("Continuing A-1"),
                    "{case}: {text}"
                );
                let code = match case {
                    "ended" => "assistant_ended",
                    "missing" => "assistant_unknown",
                    "live" => "assistant_alive",
                    "current" => "assistant_current",
                    "stale" => "switch_request_invalid",
                    _ => "assistant_claimed",
                };
                assert!(text.contains(code), "{case}: {text}");
                Ok(status(0))
            },
        )
        .unwrap();
        assert_eq!(calls, 2, "{case}");
        // The refused target is untouched: no generation opened, a live binding not stolen.
        if case != "missing" {
            let target = ctx.store.get_assistant_container("A-2").unwrap();
            assert_eq!(target.state.binding.generation, 1, "{case}");
            assert_eq!(
                ctx.store.get_assistant("A-2").unwrap().ended.is_some(),
                case == "ended"
            );
        }
    }
}

#[test]
fn a_target_that_cannot_start_stays_resumable_and_the_owner_returns_to_the_source() {
    let (fx, prepared, a1, _) = setup();
    let ctx = f::ctx_at(fx.root());
    let mut calls = 0;
    supervise_with(
        &ctx,
        PANE,
        assistant::claim(&ctx.store, "A-1").unwrap(),
        prepared,
        &Overrides::default(),
        false,
        |prepared| {
            calls += 1;
            match calls {
                1 => {
                    bind(&ctx, "A-1", &a1, None);
                    request(&ctx, prepared, "A-1", 1, "A-2");
                    Ok(status(RESUME_ASSISTANT_EXIT))
                }
                2 => anyhow::bail!("fixture spawn failed"),
                _ => {
                    let b = bootstrap(prepared);
                    assert_eq!((b.assistant.as_deref(), b.generation), (Some("A-1"), 2));
                    let text = notice(prepared);
                    assert!(
                        text.contains("A-2 could not start") && text.contains("Returned to A-1"),
                        "{text}"
                    );
                    Ok(status(0))
                }
            }
        },
    )
    .unwrap();
    assert_eq!(calls, 3);
    assert!(ctx.store.get_assistant("A-2").unwrap().ended.is_none());
    // The failed generation is replaced by the next resume, never left as a live binding.
    assistant::resume(&ctx, "A-2", PANE, &Overrides::default()).unwrap();
    assert_eq!(
        ctx.store
            .get_assistant_container("A-2")
            .unwrap()
            .state
            .binding
            .generation,
        3
    );
}

#[test]
fn clean_quit_suspends_a_returnable_conversation_and_explicit_end_stays_ended() {
    let (fx, prepared, a1, _) = setup();
    let ctx = f::ctx_at(fx.root());
    supervise_with(
        &ctx,
        PANE,
        assistant::claim(&ctx.store, "A-1").unwrap(),
        prepared,
        &Overrides::default(),
        false,
        |_| {
            bind(&ctx, "A-1", &a1, None);
            Ok(status(0))
        },
    )
    .unwrap();
    assert!(ctx.store.get_assistant("A-1").unwrap().ended.is_none());
    assert_eq!(fence_reason(&ctx, "A-1", 1).as_deref(), Some("quit"));
    let (_, prepared) = assistant::resume(&ctx, "A-1", PANE, &Overrides::default()).unwrap();
    assert_eq!(bootstrap(&prepared).resume_session.as_ref(), Some(&a1));
    assistant::end(&ctx, "A-1").unwrap();
    let error = assistant::resume(&ctx, "A-1", PANE, &Overrides::default()).unwrap_err();
    assert!(format!("{error:#}").contains("assistant_ended"));
}

#[test]
fn a_fresh_session_is_claimed_before_any_resumer_can_see_it() {
    // The launch and /new share open_claimed; pause it between claim and publication.
    let fx = fixture_with_role();
    let ctx = f::ctx_at(fx.root());
    let mut paused = None;
    let (session, _, _, claim) = assistant::open_observed(
        &ctx,
        ctx.store.root(),
        PANE,
        &Overrides::default(),
        false,
        None,
        |id| {
            // A concurrent `corral pi --resume` or `/resume` claims first: refused, and the
            // record it would resume is not visible yet.
            let rival = assistant::claim(&ctx.store, id).unwrap_err();
            assert!(format!("{rival:#}").contains("assistant_claimed"));
            assert!(!ctx.store.assistant_exists(id).unwrap());
            paused = Some(id.to_owned());
        },
    )
    .unwrap();
    assert_eq!(paused.as_deref(), Some(session.id.as_str()));
    // After publication the rival is still refused; the winner keeps one open generation.
    assert!(assistant::claim(&ctx.store, &session.id).is_err());
    let container = ctx.store.get_assistant_container(&session.id).unwrap();
    assert_eq!(container.state.generations.len(), 1);
    assert!(
        ctx.store
            .get_assistant(&session.id)
            .unwrap()
            .ended
            .is_none()
    );
    drop(claim);

    // A launcher that loses the claim publishes nothing and ends nobody else's session.
    let held = assistant::claim(&ctx.store, "A-2").unwrap();
    let error = assistant::open_claimed(
        &ctx,
        ctx.store.root(),
        PANE,
        &Overrides::default(),
        false,
        None,
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("assistant_claimed"));
    assert!(!ctx.store.assistant_exists("A-2").unwrap());
    assert!(ctx.store.get_assistant("A-1").unwrap().ended.is_none());
    drop(held);
}
