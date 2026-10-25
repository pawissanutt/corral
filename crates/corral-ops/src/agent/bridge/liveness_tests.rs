//! Host liveness (T-252 recovery): only a verified fact makes a host lost, each loss is recorded
//! once, and a fresh corral process reconnects through `ctl.cap` and continues `events_since`
//! from the saved sequence after a board restart, recording a gap instead of guessing.
use corral_core::agent::{ContainerRef, HostHealth};
use corral_core::inbox::Recipient;
use serde_json::{Value, json};

use super::fixture::{self as f, Fx, HostProc};
use super::liveness::{LossReason, Probe, probe, record};

fn recipient(fx: &Fx) -> Recipient {
    Recipient::Run {
        task: fx.id.clone(),
        run: "R1".into(),
    }
}

fn probe_now(fx: &Fx) -> Probe {
    let task = fx.task();
    let state = corral_core::agent::container_state(&task, &fx.worker()).unwrap();
    probe(&fx.ctx.store, &recipient(fx), &fx.worker(), state).unwrap()
}

fn reason(probe: &Probe) -> Option<LossReason> {
    match probe {
        Probe::Lost { reason, .. } => Some(*reason),
        _ => None,
    }
}

fn foreign(frame: &Value) -> Value {
    match frame["command"].as_str() {
        Some("get_state") => json!({"ok": true, "result": {
            "generation": 1, "session": {"bound_id": "another-session"}}}),
        _ => f::healthy(frame),
    }
}

fn busy(_: &Value) -> Value {
    json!({"ok": false, "error": {"code": "busy", "reason": "at most 4 requests in flight"}})
}

fn gapped(frame: &Value) -> Value {
    match frame["command"].as_str() {
        Some("events_since") => json!({"ok": true, "result": {"generation": 1,
            "events": [{"seq": 9, "generation": 1, "kind": "queue"}], "last": 9, "next": 9,
            "more": false, "gap": {"after": 3, "resumed": 9, "missing": 5, "reconcile": true}}}),
        _ => f::healthy(frame),
    }
}

#[test]
fn only_a_verified_fact_makes_a_host_lost_and_each_loss_is_recorded_once() {
    // Given a bound host that is alive but whose generation has no control files:
    let fx = f::fixture(false, 5);
    let mut host = HostProc::spawn(true);
    fx.bind(&fx.worker_target(), "w0:p1", &host);
    let missing = probe_now(&fx);
    assert_eq!(reason(&missing), Some(LossReason::ControlCredentialMissing));

    // When ctl.cap exists but the socket does not, or the socket answers for another session:
    let generation = crate::agent::launch::generation_dir(&fx.ctx.store, &recipient(&fx), 1);
    let generation = generation.unwrap();
    crate::agent::launch::ensure_private_dir(&crate::agent::launch::runtime_root(), &generation)
        .unwrap();
    crate::agent::launch::write_ctl_cap(&generation, f::CAP).unwrap();
    assert_eq!(reason(&probe_now(&fx)), Some(LossReason::SocketMissing));
    std::fs::remove_file(generation.join("ctl.cap")).unwrap();
    f::bridge(&fx.ctx.store, &recipient(&fx), 1, Some(f::CAP), foreign);
    assert_eq!(reason(&probe_now(&fx)), Some(LossReason::HostMismatch));

    // Then none of those probes wrote anything until recorded, and a record writes once:
    assert!(fx.events("agent_lost").is_empty());
    assert!(record(&fx.ctx.store, &fx.worker(), &missing).unwrap());
    assert!(!record(&fx.ctx.store, &fx.worker(), &missing).unwrap());
    assert_eq!(fx.events("agent_lost").len(), 1);
    let task = fx.task();
    let state = corral_core::agent::container_state(&task, &fx.worker()).unwrap();
    assert_eq!(state.binding.health, HostHealth::Lost);
    assert_eq!(
        probe_now(&fx),
        Probe::Skip,
        "a lost binding is not probed again"
    );

    // Given another host whose process exits, and one whose bridge is busy:
    let fx = f::fixture(false, 5);
    let mut exited = HostProc::spawn(true);
    fx.bind(&fx.worker_target(), "w0:p1", &exited);
    f::bridge(&fx.ctx.store, &recipient(&fx), 1, Some(f::CAP), busy);
    assert_eq!(probe_now(&fx), Probe::Busy, "busy is no conclusion");
    exited.kill();
    assert_eq!(reason(&probe_now(&fx)), Some(LossReason::HostExited));
    host.kill();
}

#[test]
fn a_fresh_process_reconnects_through_ctl_cap_and_continues_events_from_the_saved_sequence() {
    // Given a live host whose first probe reconciled events 1 to 3:
    let fx = f::fixture(false, 5);
    let host = HostProc::spawn(true);
    fx.bind(&fx.worker_target(), "w0:p1", &host);
    let bridge = f::bridge(&fx.ctx.store, &recipient(&fx), 1, Some(f::CAP), f::healthy);
    let first = probe_now(&fx);
    assert_eq!(
        first,
        Probe::Healthy {
            generation: 1,
            seq: 3
        }
    );
    assert!(record(&fx.ctx.store, &fx.worker(), &first).unwrap());
    let before = fx.task();

    // When a fresh corral process (a restarted board) probes again:
    let fresh = f::ctx_at(fx.root());
    let task = fresh.store.get(&fx.id).unwrap();
    let state = corral_core::agent::container_state(&task, &fx.worker()).unwrap();
    let again = probe(&fresh.store, &recipient(&fx), &fx.worker(), state).unwrap();

    // Then it read get_state and asked for events after the saved sequence only, and nothing
    // about turns, receipts or health was inferred:
    let frames = bridge.commands();
    let asked: Vec<&Value> = frames
        .iter()
        .filter(|f| f["command"] == "events_since")
        .map(|f| &f["args"]["seq"])
        .collect();
    assert_eq!(asked, [&json!(0), &json!(3)]);
    assert!(frames.iter().all(|f| f["ctl_cap"] == f::CAP));
    assert!(!record(&fresh.store, &fx.worker(), &again).unwrap());
    let after = fx.task();
    let state = |t: &corral_core::Task| {
        corral_core::agent::container_state(t, &fx.worker())
            .unwrap()
            .clone()
    };
    assert_eq!(state(&after), state(&before));

    // When events were evicted before the reader saw them, the gap is recorded for
    // reconciliation from canonical records and the cursor moves past it:
    let fx = f::fixture(false, 5);
    fx.bind(&fx.worker_target(), "w0:p1", &host);
    f::bridge(&fx.ctx.store, &recipient(&fx), 1, Some(f::CAP), gapped);
    let page = probe_now(&fx);
    assert_eq!(
        page,
        Probe::Healthy {
            generation: 1,
            seq: 9
        }
    );
    let gaps = fx.events("agent_events_gap");
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].detail["reconcile"], true);
    assert_eq!(
        gaps[0].detail["container"],
        json!(
            ContainerRef::Worker {
                task: fx.id.clone(),
                run: "R1".into()
            }
            .key()
        )
    );
}
