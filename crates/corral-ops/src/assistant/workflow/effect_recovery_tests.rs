//! Tiny authenticated store fixtures, no processes/providers/owner terminal or live data.
use super::super::tests::Fixture;
use super::*;
use crate::agent::{BindTarget, fence_generation, test_support as ts};

fn fence(f: &Fixture) {
    fence_generation(
        &f.fx.ctx.store,
        &BindTarget::Assistant {
            session: f.session.clone(),
        }
        .container(),
        1,
        "fixture host loss",
    )
    .unwrap();
}
fn start(f: &Fixture, seq: u64, id: &str, tool: &str) {
    ts::committed(&f.boundary(
        seq,
        json!({"action":"start","id":id,"tool":tool,"authority_epoch":0}),
    ));
}
fn target_task(f: &Fixture) -> String {
    let task =
        f.fx.ctx
            .store
            .create(
                "Retain current driver and spent repair/check state",
                None,
                corral_core::Status::Running,
                "preserved history",
            )
            .unwrap();
    f.fx.ctx
        .store
        .update(&task.id, |t| {
            t.project = Some(f.container().state.binding.project.clone());
            t.runs.push(ts::run("R1", "fixture-pane"));
            Ok(())
        })
        .unwrap();
    task.id
}
fn request(f: &Fixture, id: &str) -> Request {
    Request {
        assistant: f.session.clone(),
        task: None,
        generation: 1,
        admissions: vec![id.into()],
        request_id: "recover-1".into(),
    }
}
fn apply_fixture(f: &Fixture, p: Preview) -> Result<(Receipt, bool)> {
    apply_confirmed(&f.fx.ctx.store, authority::fixture(p))
}
fn legacy_file(f: &Fixture) -> std::path::PathBuf {
    let dir = corral_core::agent::paths::container_sessions_dir(
        &f.fx.ctx.store,
        &corral_core::agent::ContainerRef::Assistant {
            session: f.session.clone(),
        },
    );
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("retained.jsonl");
    let session_id = f.host.identity.session_id.clone();
    let entries = [
        json!({"type":"session","id":session_id,"version":3}),
        json!({"type":"message","id":"model","parentId":null,"message":{"role":"assistant","content":[{"type":"toolCall","id":"call-bash","name":"bash","arguments":{"command":"never replay"}}]}}),
        json!({"type":"custom","id":"prepared","parentId":"model","customType":"corral_command","data":{"job":"j1-1","generation":1,"consumed":false}}),
        json!({"type":"custom","id":"terminal","parentId":"prepared","customType":"corral_command","data":{"job":"j1-1","generation":1,"consumed":false,"result":{"job":"j1-1","state":"exited","exit_code":0,"group_clear":true}}}),
        json!({"type":"custom","id":"consumed","parentId":"terminal","customType":"corral_command","data":{"job":"j1-1","generation":1,"consumed":true,"delivery":"result","result":{"job":"j1-1","state":"exited","exit_code":0,"group_clear":true}}}),
        json!({"type":"message","id":"result","parentId":"consumed","message":{"role":"toolResult","toolCallId":"call-bash","toolName":"bash","isError":true,"content":[{"type":"text","text":"record-unit CAS conflict"}]}}),
    ];
    std::fs::write(
        &path,
        entries.iter().map(|v| format!("{v}\n")).collect::<String>(),
    )
    .unwrap();
    f.fx.ctx
        .store
        .update_assistant_container(&f.session, |c| {
            c.state.generations[0].bound.as_mut().unwrap().session_file = path.clone();
            Ok(())
        })
        .unwrap();
    path
}

#[test]
fn assistant_effect_recovery_legacy_subset_is_exactly_once_and_unknown_resources_still_block() {
    let f = Fixture::in_project("app");
    let task = target_task(&f);
    let original_task = serde_json::to_vec(&f.fx.ctx.store.get(&task).unwrap()).unwrap();
    start(&f, 51, "7:call-bash", "bash");
    start(&f, 52, "1:1:mcp", "mcp_connect");
    start(&f, 53, "2:1:lsp", "lsp_startup");
    let path = legacy_file(&f);
    fence(&f);
    let mut req = request(&f, "7:call-bash");
    req.task = Some(task.clone());
    let p = preview(&f.fx.ctx.store, &req).unwrap();
    assert_eq!(p.selected[0].blocked, None);
    assert_eq!(p.retained.len(), 2);
    let before = f.container();
    let session = f.fx.ctx.store.get_assistant(&f.session).unwrap();
    let transcript = std::fs::read(&path).unwrap();
    let concurrent = Store::open_unobserved(f.fx.ctx.store.root()).unwrap();
    let profile_writer = concurrent.try_lock("profile-records").unwrap().unwrap();
    assert!(apply_fixture(&f, p.clone()).is_err());
    assert_eq!(
        boundary::activities(&concurrent, &f.session).unwrap().len(),
        3
    );
    assert!(concurrent.record_rows(OUTCOMES).unwrap().is_empty());
    drop(profile_writer);
    let (receipt, replayed) = apply_fixture(&f, p).unwrap();
    assert!(!replayed);
    assert_eq!(receipt.remaining_count, 2);
    assert_eq!(
        f.container(),
        before,
        "no input, consent, workflow, driver, grant, receipt or credit changes"
    );
    assert_eq!(f.fx.ctx.store.get_assistant(&f.session).unwrap(), session);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        transcript,
        "executor journal is never rewritten/replayed"
    );
    let reopened = Store::open_unobserved(f.fx.ctx.store.root()).unwrap();
    assert_eq!(
        lookup(&reopened, &f.session, &req.request_id).unwrap(),
        Some(receipt.clone())
    );
    assert_eq!(
        boundary::activities(&reopened, &f.session).unwrap().len(),
        2
    );
    let replay = apply_fixture(&f, preview(&reopened, &req).unwrap()).unwrap();
    assert!(replay.1);
    assert_eq!(replay.0, receipt);
    let mut another = req.clone();
    another.request_id = "recover-another".into();
    let replay = apply_fixture(&f, preview(&reopened, &another).unwrap()).unwrap();
    assert!(replay.1);
    assert_eq!(replay.0, receipt);
    assert_eq!(reopened.record_rows(OUTCOMES).unwrap().len(), 1);
    assert_eq!(reopened.record_rows(RECEIPTS).unwrap().len(), 1);
    assert_eq!(
        reopened
            .events_page(None, &["assistant_effects_reconciled"], None, 0, 10)
            .unwrap()
            .len(),
        1
    );
    let unknown = preview(&reopened, &request(&f, "1:1:mcp")).unwrap();
    assert!(
        unknown.selected[0]
            .blocked
            .as_ref()
            .unwrap()
            .contains("no authoritative shutdown")
    );
    assert!(apply_fixture(&f, unknown).is_err());
    let successor_settle = f
        .fx
        .ctx
        .store
        .settle_assistant_container(&f.session, |c, _| {
            boundary::apply(
                &f.fx.ctx.store,
                &f.session,
                c,
                2,
                serde_json::from_value(json!({"action":"settle","id":"1:1:mcp","uncertain":false}))
                    .unwrap(),
            )
        });
    assert!(successor_settle.is_err());
    assert_eq!(
        boundary::activities(&reopened, &f.session).unwrap().len(),
        2
    );
    assert_eq!(
        serde_json::to_vec(&f.fx.ctx.store.get(&task).unwrap()).unwrap(),
        original_task,
        "task/driver/run/checks/history/repair authority are unchanged"
    );
}

#[test]
fn assistant_effect_recovery_refuses_stale_context_truncation_wrong_source_and_managed_actor() {
    let f = Fixture::in_project("app");
    start(&f, 51, "7:call-bash", "bash");
    let path = legacy_file(&f);
    fence(&f);
    let task = target_task(&f);
    let mut req = request(&f, "7:call-bash");
    req.task = Some(task.clone());
    let stale_task = preview(&f.fx.ctx.store, &req).unwrap();
    f.fx.ctx
        .store
        .update(&task, |t| {
            t.body.push_str(" changed task contract");
            Ok(())
        })
        .unwrap();
    assert!(apply_fixture(&f, stale_task).is_err());
    let p = preview(&f.fx.ctx.store, &req).unwrap();
    f.fx.ctx
        .store
        .update_assistant(&f.session, |s| {
            s.summary = "source changed".into();
            Ok(())
        })
        .unwrap();
    assert!(apply_fixture(&f, p).is_err());
    let original = std::fs::read(&path).unwrap();
    for corrupt in [
        String::from_utf8(original.clone())
            .unwrap()
            .replace("\"group_clear\":true", "\"group_clear\":false"),
        String::from_utf8(original.clone())
            .unwrap()
            .replace("j1-1", "j2-1"),
        String::from_utf8(original.clone())
            .unwrap()
            .replace("\"consumed\":true", "\"consumed\":false"),
        String::from_utf8(original.clone())
            .unwrap()
            .replace("call-bash", "call-other"),
        String::from_utf8(original.clone())
            .unwrap()
            .trim_end()
            .to_string(),
    ] {
        std::fs::write(&path, corrupt).unwrap();
        let p = preview(&f.fx.ctx.store, &req).unwrap();
        assert!(p.selected[0].blocked.is_some());
        assert!(apply_fixture(&f, p).is_err());
    }
    std::fs::write(&path, original).unwrap();
    let p = preview(&f.fx.ctx.store, &req).unwrap();
    assert!(
        confirm(
            &f.fx.ctx.store,
            &p,
            "task text, prior takeover approval, PM answer or an operating grant is not a terminal"
        )
        .is_err()
    );
    assert_eq!(
        boundary::activities(&f.fx.ctx.store, &f.session)
            .unwrap()
            .len(),
        1
    );
    assert!(f.fx.ctx.store.record_rows(OUTCOMES).unwrap().is_empty());
}

#[test]
fn assistant_effect_recovery_managed_proof_requires_real_identity_terminal_and_seal() {
    let f = Fixture::new();
    start(&f, 51, "mcp-admission", "mcp_connect");
    let executor = facts::Executor::Process {
        resource: "fixture".into(),
        config_sha256: "c".repeat(64),
        process: facts::ProcessIdentity {
            pid: 999,
            start_time: "123".into(),
            boot_id: "fixture-boot".into(),
        },
    };
    let digest = facts::executor_digest(&executor).unwrap();
    let mut lifecycle = Vec::new();
    for (seq, event) in [
        (
            52,
            json!({"action":"prepare","instance":"unique","executor":{"kind":"preparing","resource":"fixture","config_sha256":"c".repeat(64)}}),
        ),
        (
            53,
            json!({"action":"bind","instance":"unique","executor":executor}),
        ),
        (
            54,
            json!({"action":"terminal","instance":"unique","terminal":{"disposition":"shutdown_verified","executor_sha256":digest,
            "exit_observed":true,"group_clear":true,"requests_terminal":true,"outcome":"earlier effect result unknown; executor terminated",
            "delivery":"lifecycle_journal","requests":[]}}),
        ),
    ] {
        lifecycle.push(event.clone());
        ts::committed(&f.boundary(
            seq,
            json!({"action":"evidence","id":"mcp-admission","event":event}),
        ));
    }
    let path = legacy_file(&f);
    // No sealed aggregate means there might still be another executor. Host loss is not seal.
    fence(&f);
    let req = request(&f, "mcp-admission");
    assert!(
        preview(&f.fx.ctx.store, &req).unwrap().selected[0]
            .blocked
            .is_some()
    );
    let key = facts::key(&f.session, 1, "mcp-admission");
    let evidence: facts::Evidence = f.fx.ctx.store.record(facts::AREA, &key).unwrap().unwrap();
    // Simulate terminal+seal journaled before canonical acknowledgement failed. Reconciliation
    // reads these exact retained bytes, not a successor manager's empty registry.
    lifecycle.push(json!({"action":"seal"}));
    let mut retained = std::fs::read_to_string(&path).unwrap();
    for (n, event) in lifecycle.into_iter().enumerate() {
        retained.push_str(&format!("{}\n", json!({"type":"custom","customType":"corral_effect_lifecycle",
            "id":format!("life-{n}"),"parentId":if n == 0 { "result".into() } else { format!("life-{}",n-1) },
            "data":{"version":1,"assistant":f.session,"generation":1,"session_id":f.host.identity.session_id,
            "admission":"mcp-admission","event":event}})));
    }
    std::fs::write(&path, retained).unwrap();
    let p = preview(&f.fx.ctx.store, &req).unwrap();
    assert_eq!(p.selected[0].blocked, None);
    for bad in ["version", "generation", "terminal"] {
        let mut corrupt = evidence.clone();
        match bad {
            "version" => corrupt.version = 2,
            "generation" => corrupt.generation = 2,
            _ => {
                corrupt
                    .instances
                    .get_mut("unique")
                    .unwrap()
                    .terminal
                    .as_mut()
                    .unwrap()
                    .group_clear = false
            }
        }
        f.fx.ctx
            .store
            .save_record(facts::AREA, &key, &corrupt)
            .unwrap();
        assert!(
            preview(&f.fx.ctx.store, &req).unwrap().selected[0]
                .blocked
                .is_some()
        );
        assert!(apply_fixture(&f, p.clone()).is_err());
    }
    f.fx.ctx
        .store
        .save_record(facts::AREA, &key, &evidence)
        .unwrap();
    // Fresh proof record version is part of CAS even when its bytes return to the same value.
    assert!(apply_fixture(&f, p).is_err());
    assert_eq!(
        apply_fixture(&f, preview(&f.fx.ctx.store, &req).unwrap())
            .unwrap()
            .0
            .remaining_count,
        0
    );
    assert_eq!(
        f.container().workflow,
        None,
        "metadata recovery is not handoff_drain or workflow entry"
    );
}
