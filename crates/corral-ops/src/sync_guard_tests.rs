//! The per-tick agent-state reconcile's guard on its real path (plan T-253 section 4, "CAS
//! guards"): `stuck`, `pending_reply`, `seen_pane` and `obligations_rev` stand in for
//! `db316644`'s comparison of the scalars and the whole obligations. A write between the tick's
//! read and its guard is decided as `db316644` decides it, except A->B->A of obligation
//! content, which is refused; the next tick then behaves exactly as after a genuine change.
use std::collections::HashMap;

use corral_core::{Event, Run, Status, Store};
use corral_herdr::{AgentStatus, PaneInfo};
use jiff::Timestamp;
use serde_json::json;

use super::*;

fn fixture() -> (tempfile::TempDir, Ctx, String) {
    let tmp = tempfile::tempdir().unwrap();
    let ctx = Ctx {
        store: Store::init(tmp.path()).unwrap(),
        herdr: corral_herdr::Herdr::with_bin(tmp.path().join("no-herdr"), None),
        resource_probe: crate::resources::FixedResourceProbe::arc(),
    };
    let task = ctx
        .store
        .create("Agent", None, Status::Running, "")
        .unwrap();
    let socket = ctx.herdr.socket_path().unwrap();
    // A working agent whose stored state is stale, plus one final wait only the writer edits.
    let run: Run = serde_json::from_value(json!({
        "id": "R1", "agent": "claude", "worktree": tmp.path(), "branch": "main", "base": "main",
        "base_sha": "fixture", "started": "2026-10-19T00:00:00Z",
        "herdr": {"workspace_id": "w1", "pane_id": "w1:p1", "socket": socket},
        "seen_pane": true, "seen_working": true, "agent_state": "idle",
        "obligations": {"waits": [{
            "request": {"version": 1, "id": "W2", "run": "R1", "condition": "review",
                        "owner": "worker", "source": "worker", "deadline": null,
                        "handle": {"kind": "channel", "channel_id": "c2", "capability": "wait_v1"}},
            "registered_at": "2026-10-19T00:00:00Z",
            "observation": {"at": "2026-10-19T00:00:00Z", "state": "ready", "detail": "A"},
            "receipt": {"version": 1, "id": "rc-W2", "run": "R1", "outcome": "answered",
                        "delivery": "acknowledged", "observed_at": "2026-10-19T00:00:00Z"}
        }], "decisions": []}
    }))
    .unwrap();
    ctx.store
        .update(&task.id, |t| {
            t.runs.push(run);
            Ok(())
        })
        .unwrap();
    (tmp, ctx, task.id)
}

fn set_w2(ctx: &Ctx, id: &str, detail: &str) {
    ctx.store
        .update_head(id, |head| {
            head.edit_wait("R1", "W2", |wait| {
                wait.observation.detail = detail.into();
                Ok(())
            })
        })
        .unwrap();
}

fn interleave(ctx: &Ctx, id: &str, kind: &str) {
    match kind {
        "none" => {}
        // Rewrites the stored document unchanged: never a conflict.
        "identical" => set_w2(ctx, id, "A"),
        "genuine" => set_w2(ctx, id, "B"),
        "aba" => {
            set_w2(ctx, id, "B");
            set_w2(ctx, id, "A");
        }
        other => panic!("unknown interleaving {other}"),
    }
}

/// The stored agent state, status and the task's events after one tick.
#[derive(Debug, PartialEq)]
struct Pass {
    agent_state: Option<String>,
    status: Status,
    events: Vec<(String, serde_json::Value)>,
}

fn pass(ctx: &Ctx, id: &str) -> Pass {
    let task = ctx.store.get(id).unwrap();
    let events: Vec<Event> = ctx
        .store
        .events_of(
            Some(id),
            &["needs_me", "resumed", "stuck", "idle_no_report"],
        )
        .unwrap();
    Pass {
        agent_state: task.runs[0].agent_state.clone(),
        status: task.status,
        events: events.into_iter().map(|e| (e.kind, e.detail)).collect(),
    }
}

fn tick(ctx: &Ctx, head: &corral_core::TaskHead) {
    let pane = PaneInfo {
        pane_id: "w1:p1".into(),
        workspace_id: "w1".into(),
        tab_id: "t1".into(),
        agent: None,
        status: AgentStatus::Working,
        title: None,
        cwd: None,
    };
    let panes = HashMap::from([("w1:p1".to_owned(), pane)]);
    let now: Timestamp = "2026-10-20T00:00:00Z".parse().unwrap();
    let snapshot = crate::pane_liveness::Snapshot::injected(&panes);
    reconcile(ctx, head, snapshot, now, &SystemProcesses, false, None).unwrap();
}

fn run(kind: &str) -> (Pass, Pass) {
    let (_tmp, ctx, id) = fixture();
    // The tick reads its head, then a writer commits between that read and the guard.
    let head = ctx.store.head(&id).unwrap();
    interleave(&ctx, &id, kind);
    tick(&ctx, &head);
    let first = pass(&ctx, &id);
    tick(&ctx, &ctx.store.head(&id).unwrap());
    (first, pass(&ctx, &id))
}

#[test]
fn obligations_rev_guard_decides_as_the_whole_comparison_except_aba() {
    let (none_first, none_second) = run("none");
    assert_eq!(none_first.agent_state.as_deref(), Some("working"));
    assert_eq!(none_second, none_first);

    // A write leaving the stored bytes equal is accepted, exactly as with no write.
    assert_eq!(run("identical"), (none_first, none_second));

    // A genuine obligation change is refused by both guards; the next tick applies the state.
    let (genuine_first, genuine_second) = run("genuine");
    assert_eq!(genuine_first.agent_state.as_deref(), Some("idle"));
    assert_eq!(genuine_second.agent_state.as_deref(), Some("working"));

    // A->B->A: db316644's whole comparison accepted it; obligations_rev refuses it, and the
    // path then behaves exactly as after the genuine change's refusal.
    assert_eq!(run("aba"), (genuine_first, genuine_second));
}
