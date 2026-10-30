//! T-605: corral's side of the herdr projection. The hello receipt fixes a generation's sequence
//! range and names the questions its host must show blocked; a lost generation is released once,
//! at the last number of that range, whichever sync path recorded the loss, and that number is
//! below every number its replacement reports with.
use corral_core::agent::{self, AgentQuestion};
use corral_core::inbox::Recipient;
use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

use super::*;
use crate::agent::bridge::fixture::{self as f, HostProc, SESSION};
use crate::agent::bridge::liveness;

fn sync(fx: &f::Fx) {
    liveness::probe_hosts_in_tests();
    crate::agent::hooks::sync::task(&fx.ctx, &fx.task(), Timestamp::now()).unwrap();
}

fn generation(fx: &f::Fx, n: u32) -> AgentGeneration {
    let task = fx.task();
    let state = agent::container_state(&task, &fx.worker()).unwrap();
    state.generation(n).unwrap().clone()
}

fn hello_result(fx: &f::Fx, n: u32) -> Value {
    let task = fx.task();
    let state = agent::container_state(&task, &fx.worker()).unwrap();
    let request = crate::agent::event_request_id(n, 0);
    state.receipt(&request).unwrap().result.clone()
}

fn released(fx: &f::Fx) -> Vec<String> {
    let mut calls = fx.calls("pane release-agent");
    calls.extend(fx.calls("pane report-metadata w0:p1 --source custom:corral-pi"));
    calls
}

fn release_calls(seq: u64) -> Vec<String> {
    let clears: String = TOKENS
        .iter()
        .map(|t| format!(" --clear-token {t}"))
        .collect();
    vec![
        format!("pane release-agent w0:p1 --source custom:corral-pi --agent corral-pi --seq {seq}"),
        format!(
            "pane report-metadata w0:p1 --source custom:corral-pi --clear-display-agent{clears} --seq {seq}"
        ),
    ]
}

fn exited(fx: &f::Fx, host: &mut HostProc) {
    host.kill();
    let exit = crate::launch_exit::receipt(&fx.ctx, &fx.id, "R1");
    std::fs::create_dir_all(exit.parent().unwrap()).unwrap();
    std::fs::write(&exit, "137\n").unwrap();
}

fn ask_blocking(fx: &f::Fx) -> String {
    let key = format!("{}/Q1", fx.worker().key());
    let question: AgentQuestion = serde_json::from_value(json!({
        "version": 1, "key": key, "revision": 1, "kind": "owner_only",
        "asked": {"request_id": "rq-q", "payload_sha256": "00", "session_id": SESSION,
                  "generation": 1, "at": Timestamp::now()},
        "blocking": true, "text": "Which input?", "state": "open",
        "wake": {"route": "none", "key": "qwake", "state": "pending"},
    }))
    .unwrap();
    let worker = fx.worker();
    fx.ctx
        .store
        .update(&fx.id, |t| {
            agent::container_state_mut(t, &worker)
                .unwrap()
                .questions
                .push(question.clone());
            Ok(())
        })
        .unwrap();
    key
}

#[test]
fn a_generation_range_ends_below_the_first_number_of_any_later_millisecond() {
    let at = Timestamp::from_millisecond(1_793_000_000_123).unwrap();
    let end = release_seq(at).unwrap();
    assert_eq!(end, (1_793_000_000_123_u64 << 20) + (1 << 20) - 1);
    let later = at.checked_add(SignedDuration::from_millis(1)).unwrap();
    assert_eq!(release_seq(later).unwrap() - ((1 << 20) - 1), end + 1);
    assert_eq!(release_seq(Timestamp::from_millisecond(-1).unwrap()), None);
}

#[test]
fn a_host_that_exits_to_its_pane_is_released_once_at_its_range_end() {
    // Given a bound worker host whose process exits to its pane shell:
    let fx = f::fixture(false, 5);
    let mut host = HostProc::spawn(true);
    fx.bind(&fx.worker_target(), "w0:p1", &host);
    exited(&fx, &mut host);

    // When sync runs twice:
    sync(&fx);
    sync(&fx);

    // Then the one recorded loss releases the pane's report and metadata once, at the range end:
    assert_eq!(fx.events("agent_lost").len(), 1);
    let at = generation(&fx, 1).bound.unwrap().at;
    assert_eq!(released(&fx), release_calls(release_seq(at).unwrap()));
}

#[test]
fn a_probe_that_cannot_reach_a_live_host_releases_it_once() {
    // Given a bound host process that is alive but has no control socket:
    let fx = f::fixture(false, 5);
    let host = HostProc::spawn(true);
    fx.bind(&fx.worker_target(), "w0:p1", &host);

    // When sync probes it twice:
    sync(&fx);
    sync(&fx);

    // Then its loss is recorded once and released once, and the live process is left alone:
    assert_eq!(fx.events("agent_lost").len(), 1);
    let at = generation(&fx, 1).bound.unwrap().at;
    assert_eq!(released(&fx), release_calls(release_seq(at).unwrap()));
    assert!(fx.calls("pane close").is_empty() && fx.calls("pane send").is_empty());
    assert!(
        !liveness::gone(&corral_core::agent::Executor {
            pid: host.pid(),
            start_time: host.start_time(),
        })
        .unwrap()
    );
}

#[test]
fn hello_names_the_range_and_pending_questions_and_a_replacement_reports_above_the_release() {
    // Given generation 1 with a pending owner question, lost and released:
    let fx = f::fixture(false, 5);
    let mut first = HostProc::spawn(true);
    let bound = fx.bind(&fx.worker_target(), "w0:p1", &first);
    let key = ask_blocking(&fx);
    exited(&fx, &mut first);
    sync(&fx);

    // When the owner resumes the run and generation 2 binds with hello:
    crate::agent::recovery::resume_container(&f::ctx_at(fx.root()), &fx.worker(), false).unwrap();
    let second = HostProc::spawn(true);
    let recipient = Recipient::Run {
        task: fx.id.clone(),
        run: "R1".into(),
    };
    fx.hello_next(&bound, &recipient, &second);

    // Then each hello receipt names its generation's bind time as the herdr epoch:
    let (one, two) = (generation(&fx, 1), generation(&fx, 2));
    let (at1, at2) = (one.bound.unwrap().at, two.bound.unwrap().at);
    for (n, at) in [(1, at1), (2, at2)] {
        assert_eq!(
            hello_result(&fx, n)["herdr_seq_epoch_ms"],
            json!(at.as_millisecond())
        );
    }
    // And generation 2's hello names the still-pending question, so its host starts blocked:
    assert_eq!(
        hello_result(&fx, 2)["questions"],
        json!([{"key": key, "kind": "owner_only", "blocking": true, "state": "open"}])
    );
    // And generation 1's release is below every number generation 2 reports with:
    let base2 = u64::try_from(at2.as_millisecond()).unwrap() << 20;
    assert!(release_seq(at1).unwrap() < base2);
    assert_eq!(released(&fx), release_calls(release_seq(at1).unwrap()));
}

#[test]
fn nothing_is_released_for_an_unbound_generation_or_a_gone_pane() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("herdr");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {log}\nprintf '{{\"error\":{{\"code\":\"pane_not_found\",\"message\":\"gone\"}}}}\\n' >&2\nexit 1\n",
        log = dir.path().join("calls.log").display()
    );
    crate::agent::test_support::write_executable(&bin, &script);
    let herdr = Herdr::with_bin(bin, None);
    let mut generation: AgentGeneration = serde_json::from_value(json!({
        "n": 1, "bind_nonce_sha256": "00", "pane": "w9:p9", "created": Timestamp::now(),
    }))
    .unwrap();

    assert!(!release_lost(&herdr, &generation).unwrap());
    assert!(!dir.path().join("calls.log").exists());

    generation.bound = Some(
        serde_json::from_value(json!({
            "host_cap_sha256": "00", "ctl_cap_sha256": "00", "session_id": SESSION,
            "session_file": "/tmp/s.jsonl", "pid": 1, "start_time": "1", "request_id": "ev-1-0",
            "at": Timestamp::now(),
        }))
        .unwrap(),
    );
    assert!(!release_lost(&herdr, &generation).unwrap());
    let calls = std::fs::read_to_string(dir.path().join("calls.log")).unwrap();
    assert_eq!(calls.lines().count(), 1, "{calls}");
}
