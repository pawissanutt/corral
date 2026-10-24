//! Integrated `sync_at` regressions for the Pi-native seams, against a scripted terminal engine:
//! owner attention survives pane reconciliation (round-1 F1), open review children get a
//! delivery pass (F2), and closed containers settle their residual inbox and runtime once (F3).
use std::os::unix::fs::PermissionsExt;

use corral_core::agent::{
    AgentBinding, AgentQuestion, AgentRole, AgentRunState, ContainerRef, HostHealth, paths,
};
use corral_core::inbox::{
    DeliveryMode, Inbox, MAX_PAYLOAD, MessageBinding, Post, Recipient, TerminalState, Urgency,
};
use corral_core::{Run, RunOutcome, Status, Store, Task};
use jiff::Timestamp;
use serde_json::json;

use crate::Ctx;

/// Lists one pane `w1:p1` whose agent is working; every other call succeeds empty.
const ENGINE: &str = r#"#!/usr/bin/env bash
case "$1 $2" in
  "workspace list") echo '{"result":{"type":"workspace_list","workspaces":[]}}' ;;
  "pane list") echo '{"result":{"type":"pane_list","panes":[{"pane_id":"w1:p1","workspace_id":"w1","tab_id":"w1:t1","agent":"pi","agent_status":"working"}]}}' ;;
  "pane read") echo "fixture" ;;
  "pane process-info") echo '{"result":{"process_info":{"shell_pid":10,"foreground_processes":[]}}}' ;;
  *) echo '{"result":{}}' ;;
esac
"#;

struct Fx {
    _tmp: tempfile::TempDir,
    ctx: Ctx,
}

impl Drop for Fx {
    fn drop(&mut self) {
        // The runtime root is the real one; this store's subtree is private to the test.
        let root = super::super::launch::runtime_root();
        let _ = std::fs::remove_dir_all(root.join(paths::store_id(&self.ctx.store)));
    }
}

fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("herdr");
    std::fs::write(&bin, ENGINE).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let ctx = Ctx {
        store: Store::init(tmp.path()).unwrap(),
        herdr: corral_herdr::Herdr::with_bin(bin, None),
        resource_probe: crate::resources::FixedResourceProbe::arc(),
    };
    Fx { _tmp: tmp, ctx }
}

fn host(store: &Store, role: AgentRole) -> Box<AgentRunState> {
    Box::new(AgentRunState::new(AgentBinding {
        role,
        store: paths::store_id(store),
        project: "corral".into(),
        generation: 1,
        health: HostHealth::Healthy,
        callback: None,
        closing: false,
    }))
}

fn run(ctx: &Ctx, id: &str, harness: &str) -> Run {
    let mut run: Run = serde_json::from_value(json!({
        "id": id, "agent": "pi", "harness": harness, "worktree": ctx.store.root(),
        "branch": "main", "base": "main", "base_sha": "fixture",
        "started": "2026-10-15T00:00:00Z",
        "herdr": {"workspace_id": "w1", "pane_id": "w1:p1",
                  "socket": ctx.herdr.socket_path().unwrap()},
        "seen_pane": true, "seen_working": true
    }))
    .unwrap();
    if corral_core::is_pi_native(harness) {
        run.delivery = DeliveryMode::Inbox;
    }
    run
}

fn question(task: &str, state: &str) -> AgentQuestion {
    serde_json::from_value(json!({
        "version": 1, "key": format!("{task}/R1/Q1"), "revision": 1,
        "asked": {"request_id": "rq-fixture", "payload_sha256": "fixture",
                  "session_id": "fixture", "generation": 1, "at": "2026-10-15T00:00:00Z"},
        "kind": "clarification", "blocking": true, "text": "Which implementation?",
        "state": state, "wake": {"route": "none", "key": "fixture", "state": "posted"}
    }))
    .unwrap()
}

/// A Running task whose working pi-native worker holds one question in `state`.
fn asking_worker(fx: &Fx, state: &str, wait: bool) -> String {
    let task = fx
        .ctx
        .store
        .create("Asking", None, Status::Running, "")
        .unwrap();
    fx.ctx
        .store
        .update(&task.id, |t| {
            let mut r = run(&fx.ctx, "R1", "pi-native");
            let mut state_ = host(&fx.ctx.store, AgentRole::Worker);
            state_.questions.push(question(&t.id, state));
            r.agent_host = Some(state_);
            if wait {
                r.obligations.waits.push(
                    serde_json::from_value(json!({
                        "request": {"version": 1, "id": "w1", "run": "R1", "condition": "CI",
                            "owner": "worker", "source": "worker",
                            "deadline": "2027-01-01T00:00:00Z",
                            "handle": {"kind": "channel", "channel_id": "ci", "capability": null}},
                        "registered_at": "2026-10-15T00:00:00Z",
                        "observation": {"at": "2026-10-15T00:00:00Z", "state": "pending",
                                        "detail": "waiting"},
                        "receipt": null
                    }))
                    .unwrap(),
                );
            }
            t.runs.push(r);
            Ok(())
        })
        .unwrap();
    task.id
}

fn at(text: &str) -> Timestamp {
    text.parse().unwrap()
}

fn status(fx: &Fx, id: &str) -> Status {
    fx.ctx.store.get(id).unwrap().status
}

#[test]
fn owner_attention_on_a_pi_question_survives_pane_reconciliation() {
    // Given a working pi-native worker whose blocking question has waited past
    // question_unattended_secs:
    let fx = fx();
    let unattended = asking_worker(&fx, "open", false);
    // When sync marks it unattended, and again on a later pass while the pane still works:
    crate::sync_at(&fx.ctx, at("2026-10-15T00:12:00Z")).unwrap();
    let marked = fx.ctx.store.get(&unattended).unwrap();
    let question = &marked.runs[0].agent_host.as_ref().unwrap().questions[0];
    assert!(question.unattended_at.is_some());
    assert_eq!(marked.status, Status::NeedsMe);
    crate::sync_at(&fx.ctx, at("2026-10-15T00:13:00Z")).unwrap();
    // Then the task stays in Needs me: the working pane does not override the owner.
    assert_eq!(status(&fx, &unattended), Status::NeedsMe);

    // Given an already-escalated question, alone and beside a pending declared wait, on a
    // Running task whose pane works:
    for wait in [false, true] {
        let escalated = asking_worker(&fx, "escalated", wait);
        // When sync runs twice:
        for now in ["2026-10-15T00:01:00Z", "2026-10-15T00:02:00Z"] {
            crate::sync_at(&fx.ctx, at(now)).unwrap();
            // Then the escalation holds Needs me, with the question as the excerpt.
            let task = fx.ctx.store.get(&escalated).unwrap();
            assert_eq!(task.status, Status::NeedsMe, "wait={wait} at {now}");
            let excerpt = task.runs[0].blocked_excerpt.clone().unwrap_or_default();
            assert!(excerpt.contains("Which implementation?"), "{excerpt:?}");
        }
    }
}

fn post(inbox: &Inbox) -> u64 {
    inbox
        .post(Post {
            kind: "recovery_notice",
            urgency: Urgency::Wake,
            author: "system",
            body: "fixture",
            binding: MessageBinding::default(),
            subject: None,
            max_payload: MAX_PAYLOAD,
        })
        .unwrap()
        .seq
}

/// A Review task: an OmO parent run with one open pi-native review child bound to a host.
fn reviewed(fx: &Fx) -> Task {
    let task = fx
        .ctx
        .store
        .create("Reviewed", None, Status::Review, "")
        .unwrap();
    fx.ctx
        .store
        .update(&task.id, |t| {
            let mut parent = run(&fx.ctx, "R1", "omo");
            parent.outcome = Some(RunOutcome::Done);
            let mut child = run(&fx.ctx, "R1-review1", "pi-native");
            child.agent_host = Some(host(&fx.ctx.store, AgentRole::Reviewer));
            parent.review_runs.push(child);
            t.runs.push(parent);
            Ok(())
        })
        .unwrap()
}

#[test]
fn review_children_get_delivery_while_open_and_settle_once_closed() {
    // Given an open pi-native review child with a queued message, a runtime directory and a
    // session transcript:
    let fx = fx();
    let task = reviewed(&fx);
    let child = Recipient::ReviewRun {
        task: task.id.clone(),
        run: "R1".into(),
        review: "R1-review1".into(),
    };
    let inbox = Inbox::new(&fx.ctx.store, child.clone()).unwrap();
    let seq = post(&inbox);
    let runtime = super::super::launch::runtime_dir(&fx.ctx.store, &child).unwrap();
    std::fs::create_dir_all(runtime.join("g1")).unwrap();
    let container = ContainerRef::Reviewer {
        task: task.id.clone(),
        run: "R1".into(),
        review: "R1-review1".into(),
    };
    let sessions = paths::container_sessions_dir(&fx.ctx.store, &container);
    std::fs::create_dir_all(&sessions).unwrap();
    std::fs::write(sessions.join("session.jsonl"), "{}\n").unwrap();
    // When sync runs while it is open:
    crate::sync_at(&fx.ctx, at("2026-10-15T00:01:00Z")).unwrap();
    // Then the registered pi-native adapter was reached (it records its refusal), the message
    // stays queued, and the open child keeps its runtime.
    assert!(inbox.dir().join("blocked").exists());
    assert!(inbox.terminal(seq).unwrap().is_none());
    assert!(runtime.exists());

    // Given the child superseded by an OmO child on the OmO parent: no Pi container is open and
    // no question is pending.
    let superseded = fx
        .ctx
        .store
        .update(&task.id, |t| {
            t.runs[0]
                .review_runs
                .push(run(&fx.ctx, "R1-review2", "omo"));
            Ok(())
        })
        .unwrap();
    assert!(super::sync::wants(&superseded));
    // When sync runs:
    crate::sync_at(&fx.ctx, at("2026-10-15T00:02:00Z")).unwrap();
    // Then the message is settled, not sent, the runtime is gone and the transcript stays;
    let terminal = inbox
        .terminal(seq)
        .unwrap()
        .expect("closed child's message settled");
    assert_eq!(terminal, TerminalState::Cancelled);
    assert!(!runtime.exists());
    assert!(sessions.join("session.jsonl").exists());
    // and the container is settled for good: sync never calls the hook for it again.
    let settled = fx.ctx.store.get(&task.id).unwrap();
    assert!(settled.runs[0].review_runs[0].inbox.settled.is_some());
    assert!(!super::sync::wants(&settled));
}

#[test]
fn a_closed_worker_with_only_runtime_left_is_settled_once() {
    // Given a dropped task whose pi-native worker left only a runtime directory:
    let fx = fx();
    let task = fx
        .ctx
        .store
        .create("Dropped", None, Status::Dropped, "")
        .unwrap();
    fx.ctx
        .store
        .update(&task.id, |t| {
            t.runs.push(run(&fx.ctx, "R1", "pi-native"));
            Ok(())
        })
        .unwrap();
    let worker = Recipient::Run {
        task: task.id.clone(),
        run: "R1".into(),
    };
    let runtime = super::super::launch::runtime_dir(&fx.ctx.store, &worker).unwrap();
    std::fs::create_dir_all(runtime.join("g1")).unwrap();
    // When sync runs, then the runtime goes and the container is recorded settled.
    crate::sync_at(&fx.ctx, at("2026-10-15T00:01:00Z")).unwrap();
    assert!(!runtime.exists());
    let after = fx.ctx.store.get(&task.id).unwrap();
    assert!(after.runs[0].inbox.settled.is_some());
    assert!(!super::sync::wants(&after));
}
