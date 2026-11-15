use super::*;
use crate::agent::BindTarget;
use crate::agent::bridge::fixture::{self as f, HostProc};
use corral_core::agent::{Executor, pm_state};
use corral_core::inbox::{MessageBinding, Post, Urgency};
use corral_core::{RunOutcome, Status, TaskHold};

fn intent(ctx: &Ctx, target: Target) {
    STOPS
        .save_intent(
            &ctx.store,
            key(&target),
            &vec![StopIntent {
                target: target.clone(),
                actor: Actor::Owner,
                via: "test-owner".into(),
                attribution: json!({}),
                at: ctx.store.now(),
                completed: false,
            }],
        )
        .unwrap();
}

#[test]
fn sync_classifies_an_owner_stopped_worker_as_failed() {
    let fx = f::fixture(false, 1);
    let mut host = HostProc::spawn(true);
    let bound = fx.bind(&fx.worker_target(), "w0:p1", &host);
    let id = format!("{}/run/R1", fx.id);
    let target = super::super::resolve(&fx.ctx, &id).unwrap();
    let hold = TaskHold {
        reason: "session paused".into(),
        at: fx.ctx.store.now(),
        resume: None,
        focus: None,
    };
    fx.ctx
        .store
        .update(&fx.id, |task| {
            task.hold = Some(hold.clone());
            Ok(())
        })
        .unwrap();
    super::super::pause::RECORDS
        .save(
            &fx.ctx.store,
            &target.record_key,
            &super::super::pause::PauseRecord {
                id: id.clone(),
                generation: target.generation,
                at: fx.ctx.store.now(),
                actor: Some(Actor::Owner),
                task: Some(fx.id.clone()),
                hold: Some(hold),
            },
        )
        .unwrap();
    // The owner intent survives, but its driver dies before stopping/settling.
    intent(&fx.ctx, target);
    host.kill();
    let restarted = f::ctx_at(fx.root());
    crate::sync_at(&restarted, restarted.store.now()).unwrap();
    let task = restarted.store.get(&fx.id).unwrap();
    let run = task.current_run().unwrap();
    assert_eq!(run.outcome, Some(RunOutcome::Stopped));
    assert!(run.ended.is_some());
    assert_eq!(task.status, Status::NeedsMe);
    assert!(!task.holds_dispatch_slot());
    assert!(task.hold.is_none());
    assert!(run.worktree.is_dir());
    assert!(
        std::process::Command::new("git")
            .arg("-C")
            .arg(&run.worktree)
            .args(["show-ref", "--verify", "refs/heads/main"])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(fx.events("run_failed").is_empty());
    assert!(fx.events("agent_lost").is_empty());
    assert_eq!(fx.events("session_stopped").len(), 1);
    let target = super::super::resolve(&restarted, &id).unwrap();
    stop(&restarted, &target, Some(bound.identity.generation)).unwrap();
    assert_eq!(fx.events("session_stopped").len(), 1);
}

#[test]
fn stop_closes_an_adopted_pane() {
    let fx = f::fixture(false, 1);
    let host = HostProc::spawn(true);
    let bound = fx.bind(
        &BindTarget::Pm {
            project: "corral".into(),
        },
        "w0:p1",
        &host,
    );
    pm_state::update(&fx.ctx.store, |pm| {
        let pm = pm.as_mut().unwrap();
        pm.adopted_pane = Some(pm.pane.clone());
        Ok(((), true))
    })
    .unwrap();
    let process = Executor {
        pid: host.pid(),
        start_time: host.start_time(),
    };
    let exiting_host = std::sync::Mutex::new(host);
    let bridge = f::bridge(
        &fx.ctx.store,
        &Recipient::Pm,
        bound.identity.generation,
        Some(&bound.ctl_cap),
        move |frame| {
            if frame["command"] == "shutdown" {
                exiting_host.lock().unwrap().kill();
                json!({"ok":true,"result":{}})
            } else {
                f::quiet(frame)
            }
        },
    );
    let target = super::super::resolve(&fx.ctx, "pm").unwrap();
    assert!(target.adopted);
    let result = stop(&fx.ctx, &target, target.generation).unwrap();
    assert_eq!(result["pane_left"], "w0:p1");
    assert!(crate::agent::bridge::liveness::gone(&process).unwrap());
    assert!(fx.calls("pane close").is_empty());
    assert!(fx.calls("pane run").is_empty());
    assert!(
        bridge
            .commands()
            .iter()
            .any(|frame| frame["command"] == "shutdown")
    );
}

#[test]
fn supervisor_relaunches_a_pm_the_owner_stopped() {
    let mut fx = crate::agent::hooks::pm::tests::fixture();
    fx.ctx.store.config.pm.supervise = true;
    fx.ctx.store.config.agent.drain_secs = 1;
    fx.ctx.store.save_config().unwrap();
    let mut host = HostProc::spawn(true);
    let bound = fx.bind(
        &BindTarget::Pm {
            project: "corral".into(),
        },
        "pm-pane",
        &host,
    );
    host.kill();
    let target = super::super::resolve(&fx.ctx, "pm").unwrap();
    stop(&fx.ctx, &target, target.generation).unwrap();
    let inbox = Inbox::new(&fx.ctx.store, Recipient::Pm).unwrap();
    let message = inbox
        .post(Post {
            kind: "owner_message",
            urgency: Urgency::Wake,
            author: "owner",
            body: "Keep this queued",
            binding: MessageBinding::default(),
            subject: None,
            max_payload: crate::agent::delivery::INPUT_MAX_BYTES,
        })
        .unwrap();
    let restarted = f::ctx_at(fx.root());
    crate::pm_supervisor::supervise(&restarted).unwrap();
    assert!(!super::super::automation_may_start(&restarted, &Recipient::Pm).unwrap());
    assert_eq!(
        pm_state::load_current(&restarted.store)
            .unwrap()
            .unwrap()
            .state
            .binding
            .generation,
        bound.identity.generation
    );
    assert!(fx.calls("workspace create").is_empty());
    assert!(fx.events("pm_relaunched").is_empty());
    assert_eq!(
        inbox
            .pending_messages()
            .unwrap()
            .iter()
            .map(|item| item.seq)
            .collect::<Vec<_>>(),
        vec![message.seq]
    );
    let ended = super::super::resolve(&restarted, "pm").unwrap();
    assert!(
        ended
            .require_bridge()
            .unwrap_err()
            .to_string()
            .starts_with("ended:")
    );
    clear_pm_owner_stop(&restarted).unwrap();
    assert!(super::super::automation_may_start(&restarted, &Recipient::Pm).unwrap());
}
