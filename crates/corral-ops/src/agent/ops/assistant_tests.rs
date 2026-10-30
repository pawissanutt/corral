use std::sync::Arc;

use corral_core::agent::{AgentRole, ContainerRef, HostHealth};
use corral_core::{DiskSpace, ResourceSnapshot};
use serde_json::{Value, json};

use super::*;
use crate::agent::bridge::fixture::{self as f, Fx, HostProc, session_file, write_session};
use crate::agent::hooks::pm::tests::fixture;
use crate::agent::launch::{BOOTSTRAP_FILE, assistant_runtime_dir};
use crate::agent::pi::PiRefusal;
use crate::agent::test_support as ts;

const PANE: &str = "w1:p7";
const OTHER: &str = "w1:p8";

/// The PM fixture's pi-native profile, assets with an assistant role module, two owner panes.
fn assistant_fixture() -> Fx {
    let fx = fixture();
    let assets = fx.root().join("assets/src/roles/assistant");
    std::fs::create_dir_all(&assets).unwrap();
    std::fs::write(assets.join("index.ts"), "export {}").unwrap();
    for pane in [PANE, OTHER] {
        std::fs::write(fx.root().join(pane), "").unwrap();
    }
    fx
}

fn code(error: &anyhow::Error) -> Option<&'static str> {
    error
        .downcast_ref::<PiRefusal>()
        .map(|refusal| refusal.code)
}

fn bootstrap(fx: &Fx, id: &str, generation: u32) -> Value {
    let dir = assistant_runtime_dir(&fx.ctx.store, id).join(format!("g{generation}"));
    serde_json::from_slice(&std::fs::read(dir.join(BOOTSTRAP_FILE)).unwrap()).unwrap()
}

fn target(id: &str) -> BindTarget {
    BindTarget::Assistant { session: id.into() }
}

/// A recorded session bound in `pane` by a stand-in host, as a launched host's hello would.
fn bound(fx: &Fx, pane: &str, host: &HostProc) -> (String, ts::Host) {
    let session = fx
        .ctx
        .store
        .create_assistant("pm-pi", None, Some(pane), None)
        .unwrap();
    let bound = fx.bind(&target(&session.id), pane, host);
    (session.id, bound)
}

#[test]
fn two_launches_open_separate_assistant_sessions_without_a_task_or_slot() {
    // Given a workspace with one Running task and two owner panes.
    let fx = assistant_fixture();
    let tasks = fx.ctx.store.list().unwrap();
    // When `corral pi` opens an assistant in each pane.
    let ctx = f::ctx_at(fx.root());
    let (first, _, warning) = open(&ctx, PANE, &Overrides::default(), false).unwrap();
    let (second, _, _) = open(&ctx, OTHER, &Overrides::default(), false).unwrap();
    // Then A-1 and A-2 are separate assistant records, each at generation 1 in its own pane.
    assert_eq!((first.id.as_str(), second.id.as_str()), ("A-1", "A-2"));
    assert!(warning.is_none());
    for (id, pane) in [("A-1", PANE), ("A-2", OTHER)] {
        let container = fx.ctx.store.get_assistant_container(id).unwrap();
        assert_eq!(container.state.binding.role, AgentRole::Assistant);
        assert_eq!(container.state.binding.generation, 1);
        assert_eq!(container.pane.as_deref(), Some(pane));
        assert_eq!(container.state.generation(1).unwrap().pane, pane);
        let saved = bootstrap(&fx, id, 1);
        assert_eq!(saved["role"], "assistant");
        assert_eq!(saved["assistant"], id);
        assert_eq!(saved["project"], "");
        assert_eq!(saved["cwd"], json!(fx.ctx.store.root()));
        assert!(saved["capabilities"]["digest"].is_string());
    }
    // No task, run, queue entry or pane typing came with them.
    assert_eq!(fx.ctx.store.list().unwrap(), tasks);
    assert!(fx.calls("pane run").is_empty() && fx.calls("pane send").is_empty());
    assert_eq!(fx.events("assistant_launched").len(), 2);
}

#[test]
fn focused_launch_preloads_a_bounded_brief_without_task_binding_or_mutation() {
    let fx = assistant_fixture();
    let task = fx
        .ctx
        .store
        .create(
            "Task focus",
            None,
            corral_core::Status::Backlog,
            "focus spec",
        )
        .unwrap();
    let before = fx.ctx.store.get(&task.id).unwrap();
    let ctx = f::ctx_at(fx.root());
    let (session, _, _) =
        open_focused(&ctx, PANE, &Overrides::default(), false, Some(&task.id)).unwrap();
    assert_eq!(session.focus, task.id);
    let saved = bootstrap(&fx, &session.id, 1);
    let prompt = std::path::Path::new(saved["prompt_file"].as_str().unwrap());
    let brief = std::fs::read_to_string(prompt).unwrap();
    assert!(brief.len() <= crate::assistant::BRIEF_BYTES);
    assert!(brief.contains("focus spec") && brief.contains("not authority"));
    assert_eq!(saved["role"], "assistant");
    assert!(saved["task"].is_null());
    assert_eq!(fx.ctx.store.get(&task.id).unwrap(), before);
    let count = fx.ctx.store.list_assistants().unwrap().len();
    assert!(open_focused(&ctx, OTHER, &Overrides::default(), false, Some("T-999999")).is_err());
    assert_eq!(fx.ctx.store.list_assistants().unwrap().len(), count);
}

#[test]
fn a_launch_refused_before_its_record_leaves_nothing_and_a_late_failure_ends_it() {
    let fx = assistant_fixture();
    let ctx = f::ctx_at(fx.root());
    // An unknown pane refuses before any record.
    let error = open(&ctx, "w1:p99", &Overrides::default(), false).unwrap_err();
    assert_eq!(code(&error), Some("pane_unknown"), "{error}");
    // A missing role module refuses in preflight, also before any record.
    let module = fx.root().join("assets/src/roles/assistant/index.ts");
    std::fs::remove_file(&module).unwrap();
    let error = open(&ctx, PANE, &Overrides::default(), false).unwrap_err();
    assert!(error.to_string().contains("role_unavailable"), "{error}");
    assert!(fx.ctx.store.list_assistants().unwrap().is_empty());
    // A failure after the record exists (the asset digest refuses a symlink) ends that record.
    std::fs::write(&module, "export {}").unwrap();
    std::os::unix::fs::symlink(&module, fx.root().join("assets/src/link.ts")).unwrap();
    assert!(open(&ctx, PANE, &Overrides::default(), false).is_err());
    let sessions = fx.ctx.store.list_assistants().unwrap();
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].ended.is_some());
}

#[test]
fn the_soft_cap_needs_over_cap_and_a_critical_machine_refuses_with_its_values() {
    // Given two live assistants and assistant.max_live = 2.
    let mut fx = assistant_fixture();
    fx.ctx.store.config.assistant.max_live = 2;
    fx.ctx.store.save_config().unwrap();
    let hosts = [HostProc::spawn(true), HostProc::spawn(true)];
    bound(&fx, PANE, &hosts[0]);
    bound(&fx, OTHER, &hosts[1]);
    let ctx = f::ctx_at(fx.root());
    assert_eq!(live(&ctx.store).unwrap().len(), 2);
    // When a third plain `corral pi` runs, it refuses naming the live sessions and changes nothing.
    let error = open(&ctx, PANE, &Overrides::default(), false).unwrap_err();
    assert_eq!(code(&error), Some("assistant_cap"), "{error}");
    let text = error.to_string();
    assert!(
        text.contains("A-1, A-2") && text.contains("--over-cap"),
        "{text}"
    );
    assert_eq!(fx.ctx.store.list_assistants().unwrap().len(), 2);
    // With --over-cap it opens A-3 and warns.
    let (third, _, warning) = open(&ctx, PANE, &Overrides::default(), true).unwrap();
    assert_eq!(third.id, "A-3");
    assert!(warning.unwrap().contains("assistant.max_live is 2"));
    // At critical load even --over-cap refuses machine_load with the probe's values.
    let critical = ResourceSnapshot {
        load_per_core: 0.5,
        load_one: 0.5,
        cores: 1,
        mem_available: 1024,
        cpu_psi: None,
        mem_psi: None,
        disks: vec![DiskSpace {
            path: "/".into(),
            free_bytes: u64::MAX,
        }],
    };
    let loaded = crate::Ctx {
        resource_probe: Arc::new(crate::resources::FixedResourceProbe(critical)),
        ..f::ctx_at(fx.root())
    };
    let error = open(&loaded, OTHER, &Overrides::default(), true).unwrap_err();
    assert_eq!(code(&error), Some("machine_load"), "{error}");
    assert!(
        error.to_string().contains("memory available 1024 bytes"),
        "{error}"
    );
    assert_eq!(fx.ctx.store.list_assistants().unwrap().len(), 3);
}

#[test]
fn resume_after_host_death_fences_generation_one_and_opens_two_in_this_pane() {
    // Given A-1 bound in PANE whose host was killed.
    let fx = assistant_fixture();
    let mut host = HostProc::spawn(true);
    let (id, first) = bound(&fx, PANE, &host);
    let ctx = f::ctx_at(fx.root());
    // While the host runs, resume refuses and changes nothing.
    let before = fx.ctx.store.get_assistant_container(&id).unwrap();
    let error = resume(&ctx, &id, OTHER, &Overrides::default()).unwrap_err();
    assert_eq!(code(&error), Some("assistant_alive"), "{error}");
    assert!(error.to_string().contains(PANE), "{error}");
    assert_eq!(fx.ctx.store.get_assistant_container(&id).unwrap(), before);
    host.kill();
    // Pi wrote the session file with its first message, so there is a conversation to continue.
    write_session(&first, &f::session(&[], &[]));
    // When `corral pi --resume A-1` runs in another pane.
    let (fenced, _) = resume(&ctx, &id, OTHER, &Overrides::default()).unwrap();
    // Then generation 1 is fenced and generation 2 opened there on the same Pi session file.
    assert_eq!(fenced, 1);
    let container = fx.ctx.store.get_assistant_container(&id).unwrap();
    let g1 = container.state.generation(1).unwrap();
    assert_eq!(g1.fenced.as_ref().unwrap().reason, "resume");
    assert_eq!(container.state.binding.generation, 2);
    assert_eq!(container.state.generation(2).unwrap().pane, OTHER);
    assert_eq!(container.pane.as_deref(), Some(OTHER));
    let saved = bootstrap(&fx, &id, 2);
    assert_eq!(saved["resume_session"], json!(session_file(&first)));
    assert_eq!(saved["fresh_session"], false);
    assert_eq!(fx.events("assistant_resumed").len(), 1);
    assert_eq!(fx.events("agent_fenced").len(), 1);
    // Generation 1's late calls are refused agent_fenced; its committed receipts still replay.
    let event = |host: &ts::Host, seq: u64| {
        let request = crate::agent::event_request_id(host.identity.generation, seq);
        let payload = json!({"kind": "turn_started", "turn": seq});
        ts::envelope(host, "event", &request, payload, false)
    };
    let late = ts::send(&ts_fx(&fx), &event(&first, 1), "event");
    assert_eq!(ts::code(&late), Some("agent_fenced"));
    let hello = crate::agent::event_request_id(1, 0);
    let lookup = json!({"request_id": hello});
    let found = ts::send(
        &ts_fx(&fx),
        &ts::envelope(
            &first,
            "receipt_lookup",
            &crate::agent::tool_request_id(f::SESSION, "l"),
            lookup,
            false,
        ),
        "receipt_lookup",
    );
    assert!(found.found().is_some(), "{found:?}");
}

#[test]
fn resume_of_a_never_prompted_session_starts_a_fresh_pi_file_in_the_same_directory() {
    // Given A-1 bound by a host that died before Pi wrote any session file.
    let fx = assistant_fixture();
    let mut host = HostProc::spawn(true);
    let (id, first) = bound(&fx, PANE, &host);
    host.kill();
    // When it resumes, then generation 2 starts a fresh file instead of a missing one.
    let ctx = f::ctx_at(fx.root());
    resume(&ctx, &id, PANE, &Overrides::default()).unwrap();
    let saved = bootstrap(&fx, &id, 2);
    assert_eq!(saved["fresh_session"], true);
    assert!(saved.get("resume_session").is_none_or(Value::is_null));
    assert_eq!(saved["session_dir"], json!(first.sessions));
}

#[test]
fn resume_refuses_unknown_ended_and_malformed_sessions() {
    let fx = assistant_fixture();
    let ctx = f::ctx_at(fx.root());
    for (id, expected) in [("A-9", "assistant_unknown"), ("B-1", "usage_error")] {
        let error = resume(&ctx, id, PANE, &Overrides::default()).unwrap_err();
        assert_eq!(code(&error), Some(expected), "{id}: {error}");
    }
    let session = fx
        .ctx
        .store
        .create_assistant("pm-pi", None, Some(PANE), None)
        .unwrap();
    fx.ctx.store.end_assistant(&session.id).unwrap();
    let error = resume(&ctx, &session.id, PANE, &Overrides::default()).unwrap_err();
    assert_eq!(code(&error), Some("assistant_ended"), "{error}");
}

#[test]
fn assistant_calls_commit_on_the_session_container_and_end_closes_them() {
    // Given A-1 bound by a live stand-in host.
    let fx = assistant_fixture();
    let host = HostProc::spawn(true);
    let (id, bound) = bound(&fx, PANE, &host);
    let container = ContainerRef::Assistant {
        session: id.clone(),
    };
    assert_eq!(
        ts::state(&ts_fx(&fx), &container).binding.health,
        HostHealth::Healthy
    );
    // When the host records a todo ledger.
    let request = crate::agent::tool_request_id(f::SESSION, "todo-1");
    let payload = json!({"op": "init", "phases": [{"id": "p", "label": "Phase", "steps": [
        {"id": "s", "label": "Step"}]}]});
    let input = ts::envelope(&bound, "todo", &request, payload, false);
    let stored = ts::committed(&ts::send(&ts_fx(&fx), &input, "todo"));
    // Then the receipt and progress live in the assistant's container, and a replay returns it.
    let state = fx.ctx.store.get_assistant_container(&id).unwrap().state;
    assert_eq!(state.receipt(&request), Some(&stored));
    assert!(state.todo.is_some());
    assert_eq!(
        ts::send(&ts_fx(&fx), &input, "todo").receipt(),
        Some(&stored)
    );
    // A shape that names a project or omits the session is refused before admission.
    let mut foreign = bound.clone();
    foreign.identity.project = "corral".into();
    let other = crate::agent::tool_request_id(f::SESSION, "todo-2");
    let refused = ts::send(
        &ts_fx(&fx),
        &ts::envelope(&foreign, "todo", &other, json!({}), false),
        "todo",
    );
    assert_eq!(ts::code(&refused), Some("bad_request"));
    // When the session ends while its host runs, the host cannot take the shutdown here (no
    // socket), which is reported, and every later call refuses.
    let ctx = f::ctx_at(fx.root());
    let (session, warning) = end(&ctx, &id).unwrap();
    assert!(session.ended.is_some());
    assert!(
        warning
            .unwrap()
            .contains("did not take the shutdown request")
    );
    assert!(!assistant_runtime_dir(&fx.ctx.store, &id).exists());
    let late = ts::send(
        &ts_fx(&fx),
        &ts::envelope(
            &bound,
            "todo",
            &other,
            json!({"op": "start", "step": "s"}),
            false,
        ),
        "todo",
    );
    assert_eq!(ts::code(&late), Some("run_ended"));
    assert!(live(&fx.ctx.store).unwrap().is_empty());
}

#[test]
fn doctor_shows_the_resolved_assistant_column_and_its_digest() {
    let fx = assistant_fixture();
    let checks = crate::agent::hooks::doctor::checks_with(&fx.ctx, None, None).unwrap();
    let line = checks
        .iter()
        .find(|check| check.id == "pi_native_capabilities")
        .unwrap();
    let snapshot = fx
        .ctx
        .store
        .config
        .agent
        .capabilities
        .resolve(AgentRole::Assistant, Some(&Default::default()));
    let expected = format!("assistant standard [{}]", &snapshot.digest[..12]);
    assert!(line.message.contains(&expected), "{}", line.message);
}

fn ts_fx(fx: &Fx) -> ts::Fx {
    ts::Fx {
        _tmp: tempfile::tempdir().unwrap(),
        ctx: fx.ctx.clone(),
    }
}
