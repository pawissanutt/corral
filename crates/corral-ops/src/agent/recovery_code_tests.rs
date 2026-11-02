//! Code Mode recovery (T-683): a lost host's journaled nested calls are classified from their own
//! receipts under their child keys, whether or not the `codemode` call that started the cell has a
//! result, and a child the host saw settle is not reported again. Nothing is replayed.
use corral_core::agent as core_agent;
use serde_json::json;

use super::tests::{classes, messages, receipt, resume, run_recipient};
use super::*;
use crate::agent::bridge::fixture::{self as f, Fx, HostProc, SESSION};
use crate::agent::tool_request_id;

fn child(phase: &str, parent: &str, call: &str, tool: &str) -> String {
    json!({"type": "custom", "customType": interrupted::CODE_CALL_ENTRY, "data": {
        "phase": phase, "tool": tool, "generation": 1,
        "parent": {"tool_call_id": parent, "cell": call.split('.').next(), "call": call},
    }})
    .to_string()
        + "\n"
}

fn store(fx: &Fx, receipts: Vec<RequestReceipt>) {
    let update = |t: &mut Task| {
        let state = core_agent::container_state_mut(t, &fx.worker()).unwrap();
        state.receipts.extend(receipts.clone());
        Ok(())
    };
    fx.ctx.store.update(&fx.id, update).unwrap();
}

#[test]
fn resume_classifies_each_unsettled_code_mode_child_from_its_receipt() {
    // Given a lost host whose detached cell (its codemode call has a result) left a committed
    // child, an unsent child, a shell child and a read child unsettled, and whose dangling
    // foreground cell left one committed child:
    let fx = f::fixture(false, 5);
    let mut host = HostProc::spawn(true);
    let bound = fx.bind(&fx.worker_target(), "w0:p1", &host);
    let _control = f::bridge(
        &fx.ctx.store,
        &run_recipient(&fx),
        1,
        Some(f::CAP),
        f::healthy,
    );
    let session = f::session(
        &[("detached", "codemode"), ("live", "codemode")],
        &["detached"],
    ) + &child("started", "detached", "cell-1.1", "corral_progress")
        + &child("settled", "detached", "cell-1.1", "corral_progress")
        + &child("started", "detached", "cell-1.2", "corral_progress")
        + &child("started", "detached", "cell-1.3", "corral_evidence")
        + &child("started", "detached", "cell-1.4", "bash")
        + &child("started", "detached", "cell-1.5", "read")
        + &child("started", "live", "cell-2.1", "corral_progress");
    f::write_session(&bound, &session);
    let saved = json!({"outcome": "saved"});
    store(
        &fx,
        vec![
            receipt(
                &bound,
                "detached/cell-1.1",
                "progress",
                "committed",
                saved.clone(),
            ),
            receipt(
                &bound,
                "detached/cell-1.2",
                "progress",
                "committed",
                saved.clone(),
            ),
            receipt(&bound, "live/cell-2.1", "progress", "committed", saved),
        ],
    );
    host.kill();

    // When the owner resumes the run:
    let outcome = resume(&fx, &fx.worker(), false).unwrap();

    // Then the notice reports the dangling foreground call and every unsettled child, each
    // committed child as a result under its own request ID, and the settled child not at all:
    let notices = messages(&fx, &run_recipient(&fx), NOTICE_KIND);
    assert_eq!(notices.len(), 1);
    let calls = notices[0].subject.clone().unwrap()["calls"].clone();
    let expected = [
        ("codemode", "outcome_unknown"),
        ("corral_progress", "result"),
        ("corral_evidence", "not_committed"),
        ("bash", "outcome_unknown"),
        ("read", "interrupted_no_effect"),
        ("corral_progress", "result"),
    ];
    let expected: Vec<(String, String)> = expected
        .iter()
        .map(|(t, c)| ((*t).into(), (*c).into()))
        .collect();
    assert_eq!(classes(&calls), expected);
    assert_eq!(calls, outcome["calls"]);
    let ids: Vec<&str> = calls
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["tool_call_id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "live",
            "detached/cell-1.2",
            "detached/cell-1.3",
            "detached/cell-1.4",
            "detached/cell-1.5",
            "live/cell-2.1"
        ]
    );
    for (i, key) in [
        (1, "detached/cell-1.2"),
        (2, "detached/cell-1.3"),
        (5, "live/cell-2.1"),
    ] {
        assert_eq!(calls[i]["request_id"], json!(tool_request_id(SESSION, key)));
    }
    assert!(calls[3]["request_id"].is_null());

    // And nothing was replayed or rewritten: the receipts are unchanged and the session intact.
    let after = core_agent::container_state(&fx.task(), &fx.worker())
        .unwrap()
        .clone();
    assert_eq!(
        after.receipts.iter().filter(|r| r.op == "progress").count(),
        3
    );
    assert_eq!(
        std::fs::read_to_string(f::session_file(&bound)).unwrap(),
        session
    );
}
