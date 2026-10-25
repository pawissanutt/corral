//! T-252 recovery crash windows through a killed host and an actual resume. A gate executor of
//! generation 1 held at gate_submit_before_lock on the fresh path and on both restored paths
//! (its own replay, and a new request adopting a retained attempt) is released into the fenced
//! boundary, killed after either durable refusal point, killed before the check, or left past
//! the drain; generation 2's first gate then adopts the attempt and starts it once. A host killed
//! inside an edit and a command keeps their effects: the edited file and the child process.
use std::sync::mpsc;

use serde_json::{Value, json};

use super::tests::{
    WAIT, classes, gate_runs, in_generation, messages, resume, run_recipient, state,
};
use super::*;
use crate::agent::bridge::fixture::{self as f, Exec, Fx, HostProc};

#[derive(Clone, Copy, Debug)]
enum Way {
    Fresh,
    Replay,
    Adoption,
}

#[derive(Clone, Copy, Debug)]
enum Window {
    Released,
    KilledAfterRefusal(&'static str),
    KilledBeforeCheck,
    OutlivesDrain,
}

fn class_of(outcome: &Value, call: &str) -> (String, String) {
    let calls = outcome["calls"].as_array().unwrap();
    let found = calls.iter().find(|c| c["tool_call_id"] == call).unwrap();
    let text = |v: &Value| v.as_str().unwrap_or_default().to_owned();
    (text(&found["class"]), text(&found["detail"]))
}

/// Resumes on another thread and runs `at_fence` once the fence has committed.
fn resume_fencing(fx: &Fx, at_fence: impl FnOnce()) -> Value {
    let (fenced, signal) = mpsc::channel();
    let (root, container) = (fx.root().to_path_buf(), fx.worker());
    let resumer = std::thread::spawn(move || {
        let hook = move |point: &str| {
            if point == "resume_fenced" {
                let _ = fenced.send(());
            }
        };
        crate::agent::receipts::FAILPOINT.with(|h| *h.borrow_mut() = Some(Box::new(hook)));
        resume_container(&f::ctx_at(&root), &container, false)
    });
    signal.recv_timeout(WAIT).unwrap();
    at_fence();
    resumer.join().unwrap().unwrap()
}

fn gate_case(way: Way, window: Window) {
    let case = format!("{way:?} {window:?}");
    // Given a gate request of generation 1 on `way`, its executor held at the barrier, and the
    // host dead:
    let drain = if matches!(window, Window::OutlivesDrain) {
        1
    } else {
        60
    };
    let fx = f::fixture(true, drain);
    let mut host = HostProc::spawn(true);
    let bound = fx.bind(&fx.worker_target(), "w0:p1", &host);
    let mut calls = vec![("c-gate", "corral_gate")];
    if matches!(way, Way::Adoption) {
        calls.push(("c-gate-b", "corral_gate"));
    }
    f::write_session(&bound, &f::session(&calls, &[]));
    let first = fx.origin(&bound, "c-gate", "gate");
    if !matches!(way, Way::Fresh) {
        let prepared = Exec::spawn(&fx, &first, "gate_submit_before_lock");
        prepared.reached("gate_submit_before_lock");
        prepared.kill();
    }
    let (held, call) = match way {
        Way::Adoption => (fx.origin(&bound, "c-gate-b", "gate"), "c-gate-b"),
        Way::Fresh | Way::Replay => (first.clone(), "c-gate"),
    };
    let points = match window {
        Window::KilledAfterRefusal(point) => format!("gate_submit_before_lock,{point}"),
        _ => "gate_submit_before_lock".into(),
    };
    let mut exec = Some(Exec::spawn(&fx, &held, &points));
    exec.as_ref().unwrap().reached("gate_submit_before_lock");
    if matches!(window, Window::KilledBeforeCheck) {
        exec.take().unwrap().kill();
    }
    host.kill();

    // When resume fences generation 1 and the executor is then released or killed:
    let outcome = resume_fencing(&fx, || match window {
        Window::Released => {
            exec.as_mut().unwrap().release();
        }
        Window::KilledAfterRefusal(point) => {
            let mut killed = exec.take().unwrap();
            killed.release().reached(point);
            killed.kill();
        }
        Window::KilledBeforeCheck | Window::OutlivesDrain => {}
    });

    // Then the notice reports the held call from its attempt, and nothing was submitted:
    let (class, detail) = class_of(&outcome, call);
    let stored = || {
        state(&fx, &fx.worker())
            .receipt(&held.request_id)
            .cloned()
            .unwrap()
    };
    match window {
        Window::Released | Window::KilledAfterRefusal(_) => {
            assert_eq!(class, "rejected", "{case}: {detail}");
            assert!(detail.contains("submission_refused") && detail.contains("agent_fenced"));
        }
        Window::KilledBeforeCheck => {
            assert_eq!(class, "prepared", "{case}: {detail}");
            let settled = stored();
            assert_eq!(settled.state, ReceiptState::Abandoned, "{case}");
            assert_eq!(settled.result["outcome"], "not_submitted", "{case}");
            assert_eq!(settled.result["binding"], "agent_fenced", "{case}");
        }
        Window::OutlivesDrain => {
            assert_eq!(class, "in_progress", "{case}");
            assert_eq!(outcome["draining"], json!([held.request_id]), "{case}");
            exec.as_mut().unwrap().release();
        }
    }
    if matches!(way, Way::Adoption) {
        let (earlier, _) = class_of(&outcome, "c-gate");
        assert_ne!(
            earlier, "not_executed",
            "{case}: a prepared request ran its preparation"
        );
        assert_ne!(earlier, "result", "{case}");
    }
    if let Some(exec) = exec {
        assert!(exec.wait(), "{case}: the refused executor exits cleanly");
    }
    assert_eq!(gate_runs(&fx), 0, "{case}");
    if matches!(window, Window::OutlivesDrain) {
        assert_eq!(stored().state, ReceiptState::Rejected, "{case}");
        assert_eq!(post_updates(&fx.ctx, &fx.id).unwrap(), 1, "{case}");
        assert_eq!(post_updates(&fx.ctx, &fx.id).unwrap(), 0, "{case}");
        let updates = messages(&fx, &run_recipient(&fx), UPDATE_KIND);
        assert_eq!(updates[0].subject.as_ref().unwrap()["class"], "rejected");
    }

    // And generation 2's first corral_gate adopts the retained attempt and starts it once:
    let next = in_generation(&fx, &bound, 2, "c-gate-next");
    let started = effects::gate(&f::ctx_at(fx.root()), &next).unwrap();
    let started = started.receipt().unwrap();
    assert_eq!(started.state, ReceiptState::Committed, "{case}");
    assert_eq!(started.result["adopted"], true, "{case}");
    let own = effects::own_attempt(&first.request_id);
    assert_eq!(
        started.result["attempt"].as_str(),
        Some(own.as_str()),
        "{case}"
    );
    assert_eq!(gate_runs(&fx), 1, "{case}");
}

#[test]
fn a_gate_on_each_path_cut_off_by_host_loss_is_reported_from_its_attempt_and_adopted_once() {
    let refusals = ["gate_refusal_before_retain", "gate_refused_before_receipt"];
    gate_case(Way::Fresh, Window::KilledAfterRefusal(refusals[0]));
    for way in [Way::Replay, Way::Adoption] {
        for window in [
            Window::Released,
            Window::KilledAfterRefusal(refusals[0]),
            Window::KilledAfterRefusal(refusals[1]),
            Window::KilledBeforeCheck,
            Window::OutlivesDrain,
        ] {
            gate_case(way, window);
        }
    }
}

fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        !stat
            .rsplit_once(") ")
            .is_some_and(|(_, s)| s.starts_with('Z'))
    })
}

#[test]
fn a_host_killed_inside_an_edit_and_a_command_keeps_their_effects_and_reports_them_unknown() {
    // Given a host that wrote part of an edit and started a command whose child keeps running,
    // then was killed before either call had a result:
    let fx = f::fixture(false, 5);
    let pid_file = fx.root().join("child.pid");
    let script = format!(
        "printf 'half-written edit\\n' > tracked\nsleep 600 </dev/null >/dev/null 2>&1 &\necho $! > {}",
        pid_file.display()
    );
    let mut host = HostProc::spawn_busy(&fx.app(), &script);
    let bound = fx.bind(&fx.worker_target(), "w0:p1", &host);
    f::write_session(
        &bound,
        &f::session(&[("c-edit", "edit"), ("c-bash", "bash")], &[]),
    );
    host.kill();
    let child: u32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(alive(child), "the command's child outlives its host");

    // When the owner resumes the run:
    let outcome = resume(&fx, &fx.worker(), false).unwrap();

    // Then both calls are outcome_unknown, the command's child is said to possibly still run,
    // and the edit's evidence is the worktree as the dead host left it:
    let expected = [("edit", "outcome_unknown"), ("bash", "outcome_unknown")];
    let expected: Vec<(String, String)> = expected
        .iter()
        .map(|(t, c)| ((*t).into(), (*c).into()))
        .collect();
    assert_eq!(classes(&outcome["calls"]), expected);
    let (_, bash) = class_of(&outcome, "c-bash");
    assert!(
        bash.contains("processes it started may still run"),
        "{bash}"
    );
    let title = format!(
        "Worktree after host loss ({} generation 1)",
        fx.worker().key()
    );
    let task = fx.task();
    let artifact = task.artifacts.iter().find(|a| a.title == title).unwrap();
    let snapshot = std::fs::read_to_string(fx.ctx.store.dir().join(&artifact.path)).unwrap();
    assert!(
        snapshot.contains(".M") && snapshot.contains("tracked"),
        "{snapshot}"
    );

    // And nothing was replayed or reverted: the edit stays and the child still runs:
    let tracked = std::fs::read_to_string(fx.app().join("tracked")).unwrap();
    assert_eq!(tracked, "half-written edit\n");
    assert!(
        alive(child),
        "resume never signals what the dead host started"
    );
    let status = std::process::Command::new("kill")
        .args(["-9", &child.to_string()])
        .status()
        .unwrap();
    assert!(status.success());
}
