//! T-605: corral's side of the herdr projection. Each binding gets an epoch above every earlier one,
//! whatever the wall clock does; hello records it and names the questions its host must show
//! blocked; owner and PM cancellations and escalations reach the asking host at their new revision
//! (and sync resends them while its projection differs); and a lost worker, planner, plan-reviewer
//! or assistant generation is recorded on its own container and released once, at the last number
//! of its range, without closing or typing into its pane.
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use corral_core::agent::{self, AgentQuestion, HostHealth};
use corral_core::inbox::Recipient;
use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

use super::*;
use crate::agent::BindTarget;
use crate::agent::bridge::fixture::{self as f, HostProc, SESSION};
use crate::agent::bridge::liveness;
use crate::questions::Actor;

fn sync(fx: &f::Fx) {
    liveness::probe_hosts_in_tests();
    crate::agent::hooks::sync::task(&fx.ctx, &fx.task(), Timestamp::now()).unwrap();
}

fn sync_pass(fx: &f::Fx) {
    liveness::probe_hosts_in_tests();
    crate::agent::hooks::sync::pass(&fx.ctx).unwrap();
}

fn state_of(fx: &f::Fx, container: &ContainerRef) -> AgentRunState {
    match container {
        ContainerRef::Assistant { session } => {
            fx.ctx.store.get_assistant_container(session).unwrap().state
        }
        _ => agent::container_state(&fx.task(), container)
            .unwrap()
            .clone(),
    }
}

fn epoch_of(fx: &f::Fx, container: &ContainerRef, n: u32) -> u64 {
    let state = state_of(fx, container);
    let bound = state.generation(n).unwrap().bound.clone().unwrap();
    bound.herdr_seq_epoch.expect("hello allocated an epoch")
}

fn hello_result(fx: &f::Fx, n: u32) -> Value {
    let state = state_of(fx, &fx.worker());
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

fn untouched_panes(fx: &f::Fx) -> bool {
    ["pane close", "pane run", "pane send"]
        .iter()
        .all(|verb| fx.calls(verb).is_empty())
}

fn exited(fx: &f::Fx, host: &mut HostProc) {
    host.kill();
    let exit = crate::launch_exit::receipt(&fx.ctx, &fx.id, "R1");
    std::fs::create_dir_all(exit.parent().unwrap()).unwrap();
    std::fs::write(&exit, "137\n").unwrap();
}

fn question(key: &str, kind: &str, blocking: bool) -> AgentQuestion {
    serde_json::from_value(json!({
        "version": 1, "key": key, "revision": 1, "kind": kind,
        "asked": {"request_id": format!("rq-{key}"), "payload_sha256": "00", "session_id": SESSION,
                  "generation": 1, "at": Timestamp::now()},
        "blocking": blocking, "text": "Which input?", "state": "open",
        "wake": {"route": "none", "key": format!("qwake:{key}"), "state": "pending"},
    }))
    .unwrap()
}

fn add_questions(fx: &f::Fx, questions: &[AgentQuestion]) -> Vec<String> {
    let worker = fx.worker();
    fx.ctx
        .store
        .update(&fx.id, |t| {
            let state = agent::container_state_mut(t, &worker).unwrap();
            state.questions.extend(questions.iter().cloned());
            Ok(())
        })
        .unwrap();
    questions.iter().map(|q| q.key.clone()).collect()
}

fn run_recipient(fx: &f::Fx) -> Recipient {
    Recipient::Run {
        task: fx.id.clone(),
        run: "R1".into(),
    }
}

fn pushes(bridge: &f::Bridge) -> Vec<Value> {
    bridge
        .commands()
        .into_iter()
        .filter(|frame| frame["command"] == "questions")
        .map(|frame| frame["args"]["questions"].clone())
        .collect()
}

fn entry(snapshot: &Value, key: &str) -> (Value, Value) {
    let found = snapshot
        .as_array()
        .unwrap()
        .iter()
        .find(|q| q["key"] == key)
        .unwrap();
    (found["revision"].clone(), found["state"].clone())
}

/// A healthy host whose projection holds `held`, and that takes question pushes.
fn projecting(held: Arc<Mutex<Vec<String>>>) -> impl Fn(&Value) -> Value + Send + Sync + 'static {
    move |frame: &Value| match frame["command"].as_str() {
        Some("questions") => json!({"ok": true, "result": {"applied": true}}),
        Some("get_state") => json!({"ok": true, "result": {
            "generation": frame["generation"], "session": {"bound_id": SESSION},
            "herdr": {"blocked_on": *held.lock().unwrap()}}}),
        _ => f::quiet(frame),
    }
}

#[test]
fn epochs_rise_above_every_earlier_epoch_even_when_the_clock_steps_back() {
    // Given an epoch allocated now:
    let now = Timestamp::now();
    let first = allocate_epoch(now).unwrap();
    assert!(first >= u64::try_from(now.as_millisecond()).unwrap());

    // When two more bind within the same millisecond and after a 100 ms backward clock step:
    let same = allocate_epoch(now).unwrap();
    let back = allocate_epoch(now.checked_sub(SignedDuration::from_millis(100)).unwrap()).unwrap();

    // Then each epoch is above the last, so each range starts above the previous range's end:
    assert!(first < same && same < back, "{first} {same} {back}");
    assert!(range_end(first).unwrap() < same << 20);
    assert!(range_end(same).unwrap() < back << 20);
    assert_eq!(range_end(first).unwrap(), ((first + 1) << 20) - 1);
    assert_eq!(range_end(u64::MAX), None);
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

    // Then the one recorded loss releases the pane's report and metadata once, at the end of the
    // range hello allocated and named in its receipt:
    assert_eq!(fx.events("agent_lost").len(), 1);
    let epoch = epoch_of(&fx, &fx.worker(), 1);
    assert_eq!(hello_result(&fx, 1)["herdr_seq_epoch_ms"], json!(epoch));
    assert_eq!(released(&fx), release_calls(range_end(epoch).unwrap()));
    assert!(untouched_panes(&fx));
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
    let epoch = epoch_of(&fx, &fx.worker(), 1);
    assert_eq!(released(&fx), release_calls(range_end(epoch).unwrap()));
    assert!(untouched_panes(&fx));
    assert!(
        !liveness::gone(&corral_core::agent::Executor {
            pid: host.pid(),
            start_time: host.start_time(),
        })
        .unwrap()
    );
}

#[test]
fn hello_names_the_epoch_and_questions_and_a_replacement_reports_above_the_release() {
    // Given generation 1 with a pending owner question, lost and released:
    let fx = f::fixture(false, 5);
    let mut first = HostProc::spawn(true);
    let bound = fx.bind(&fx.worker_target(), "w0:p1", &first);
    let keys = add_questions(
        &fx,
        &[question(
            &format!("{}/Q1", fx.worker().key()),
            "owner_only",
            true,
        )],
    );
    exited(&fx, &mut first);
    sync(&fx);

    // When the owner resumes the run and generation 2 binds with hello:
    crate::agent::recovery::resume_container(&f::ctx_at(fx.root()), &fx.worker(), false).unwrap();
    let second = HostProc::spawn(true);
    fx.hello_next(&bound, &run_recipient(&fx), &second);

    // Then generation 2's epoch is above generation 1's, and its hello names the still-pending
    // question with its revision, so its host starts blocked:
    let (one, two) = (
        epoch_of(&fx, &fx.worker(), 1),
        epoch_of(&fx, &fx.worker(), 2),
    );
    assert!(one < two);
    assert_eq!(hello_result(&fx, 2)["herdr_seq_epoch_ms"], json!(two));
    assert_eq!(
        hello_result(&fx, 2)["questions"],
        json!([{"key": keys[0], "revision": 1, "kind": "owner_only", "blocking": true, "state": "open"}])
    );
    // And generation 1's release is below every number generation 2 reports with:
    assert!(range_end(one).unwrap() < two << 20);
    assert_eq!(released(&fx), release_calls(range_end(one).unwrap()));
}

#[test]
fn owner_and_pm_cancellations_and_escalations_reach_the_asking_host_at_their_new_revision() {
    for pm in [false, true] {
        // Given a bound worker with three pending questions and a host that takes pushes:
        let fx = f::fixture(false, 5);
        let host = HostProc::spawn(true);
        fx.bind(&fx.worker_target(), "w0:p1", &host);
        let held = Arc::new(Mutex::new(Vec::new()));
        let bridge = f::bridge(
            &fx.ctx.store,
            &run_recipient(&fx),
            1,
            Some(f::CAP),
            projecting(held),
        );
        let keys = add_questions(
            &fx,
            &[
                question(&format!("{}/Q1", fx.worker().key()), "owner_only", true),
                question(&format!("{}/Q2", fx.worker().key()), "clarification", false),
                question(&format!("{}/Q3", fx.worker().key()), "permission", true),
            ],
        );
        let actor = || if pm { Actor::Pm(None) } else { Actor::Owner };

        // When the owner (or PM) cancels Q1 and escalates the non-blocking Q2:
        crate::questions::cancel(&fx.ctx, &keys[0], 1, actor(), "no longer needed").unwrap();
        crate::questions::escalate(&fx.ctx, &keys[1], 1, actor(), None, None).unwrap();

        // Then each transition reached the host at its new revision, and Q3 stays pending:
        let sent = pushes(&bridge);
        assert_eq!(sent.len(), 2, "pm={pm}");
        assert_eq!(entry(&sent[0], &keys[0]), (json!(2), json!("cancelled")));
        let last = &sent[1];
        assert_eq!(entry(last, &keys[0]), (json!(2), json!("cancelled")));
        assert_eq!(entry(last, &keys[1]), (json!(2), json!("escalated")));
        assert_eq!(entry(last, &keys[2]), (json!(1), json!("open")));
        assert!(untouched_panes(&fx));
    }
}

#[test]
fn a_probe_resends_questions_only_while_the_host_projection_differs() {
    // Given a bound worker whose host projects the one pending owner question:
    let fx = f::fixture(false, 5);
    let host = HostProc::spawn(true);
    fx.bind(&fx.worker_target(), "w0:p1", &host);
    let keys = add_questions(
        &fx,
        &[question(
            &format!("{}/Q1", fx.worker().key()),
            "owner_only",
            true,
        )],
    );
    let held = Arc::new(Mutex::new(keys.clone()));
    let bridge = f::bridge(
        &fx.ctx.store,
        &run_recipient(&fx),
        1,
        Some(f::CAP),
        projecting(held.clone()),
    );

    // When sync probes it, then the question is cancelled without the host hearing of it:
    sync(&fx);
    assert!(pushes(&bridge).is_empty());
    let worker = fx.worker();
    fx.ctx
        .store
        .update(&fx.id, |t| {
            let state = agent::container_state_mut(t, &worker).unwrap();
            state.questions[0].revision = 2;
            state.questions[0].state = corral_core::agent::QuestionState::Cancelled;
            Ok(())
        })
        .unwrap();
    sync(&fx);

    // Then the probe sends the questions once, and stops once the host no longer holds it:
    let sent = pushes(&bridge);
    assert_eq!(sent.len(), 1);
    assert_eq!(entry(&sent[0], &keys[0]), (json!(2), json!("cancelled")));
    held.lock().unwrap().clear();
    sync(&fx);
    assert!(pushes(&bridge).is_empty());
}

/// The fixture's task turned planning-only: native planner D1, or plan P1's native review PR1.
fn planning(fx: &f::Fx, review: bool, pane: &str) -> BindTarget {
    let id = fx.id.clone();
    let socket = fx.ctx.herdr.socket_path().unwrap();
    let output = fx.ctx.store.state_dir().join("plans");
    fx.ctx
        .store
        .update(&id, |task| {
            task.runs.clear();
            let revision = crate::plan_spec_revision(task)?;
            task.plan_runs.push(serde_json::from_value(json!({
                "id":"D1", "profile":"pi", "harness":"pi-native", "delivery":"inbox",
                "spec_revision":revision, "started":"2026-10-29T00:00:00Z", "output_dir":output,
                "rounds":0, "state":if review { "submitted" } else { "planning" },
                "herdr":{"workspace_id":"w0","pane_id":pane,"socket":socket}
            }))?);
            if review {
                task.plans.push(serde_json::from_value(json!({
                    "id":"P1", "run":"D1", "spec_revision":revision,
                    "artifact":{"run":"D1", "kind":"plan", "title":"Plan", "path":"plans/P1.md",
                        "source":"/tmp/plan.md", "sha256":"abc", "bytes":1,
                        "added":"2026-10-29T00:00:00Z"},
                    "document":{"goal":"Plan", "non_goals":[], "decisions":[], "affected_paths":[],
                        "contracts":[], "dependencies":[], "acceptance":[], "verification":[],
                        "owner_decisions":[]},
                    "reviews":[{"id":"PR1", "reviewer":"pi", "policy_digest":"fixture",
                        "delivery":"inbox", "at":"2026-10-29T00:00:00Z",
                        "herdr":{"workspace_id":"w0","pane_id":pane,"socket":socket}}]
                }))?);
            }
            Ok(())
        })
        .unwrap();
    if review {
        BindTarget::PlanReviewer {
            task: id,
            plan: "P1".into(),
            review: "PR1".into(),
        }
    } else {
        BindTarget::Planner {
            task: id,
            run: "D1".into(),
        }
    }
}

/// One fixture per role whose host binds in `pane`; sync then probes it the way it probes that role.
fn bound_role(fx: &f::Fx, role: &str, pane: &str, host: &HostProc) -> ContainerRef {
    let target = match role {
        "planner" => planning(fx, false, pane),
        "plan_reviewer" => planning(fx, true, pane),
        _ => {
            let session = fx
                .ctx
                .store
                .create_assistant("pi", None, Some(pane), None)
                .unwrap();
            BindTarget::Assistant {
                session: session.id,
            }
        }
    };
    let container = target.container();
    fx.bind(&target, pane, host);
    container
}

fn sync_role(fx: &f::Fx, container: &ContainerRef) {
    match container {
        ContainerRef::Assistant { .. } => sync_pass(fx),
        _ => sync(fx),
    }
}

#[test]
fn planner_plan_reviewer_and_assistant_losses_are_recorded_on_their_container_and_released() {
    for role in ["planner", "plan_reviewer", "assistant"] {
        // Given a bound host of the role that is alive but unreachable over its bridge:
        let fx = f::fixture(false, 5);
        let host = HostProc::spawn(true);
        let container = bound_role(&fx, role, "w0:p1", &host);

        // When sync probes it twice:
        sync_role(&fx, &container);
        sync_role(&fx, &container);

        // Then the loss is recorded once against that container, which is lost, and its
        // generation's report is released once, with no pane closed or typed into:
        let lost = fx.events("agent_lost");
        assert_eq!(lost.len(), 1, "{role}");
        assert_eq!(
            lost[0].detail["container"],
            json!(container.key()),
            "{role}"
        );
        assert_eq!(
            state_of(&fx, &container).binding.health,
            HostHealth::Lost,
            "{role}"
        );
        let epoch = epoch_of(&fx, &container, 1);
        assert_eq!(
            released(&fx),
            release_calls(range_end(epoch).unwrap()),
            "{role}"
        );
        assert!(untouched_panes(&fx), "{role}");
    }
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
            "at": Timestamp::now(), "herdr_seq_epoch": 7,
        }))
        .unwrap(),
    );
    assert!(!release_lost(&herdr, &generation).unwrap());
    let calls = std::fs::read_to_string(dir.path().join("calls.log")).unwrap();
    assert_eq!(calls.lines().count(), 1, "{calls}");
    assert!(calls.contains(&format!("--seq {}", range_end(7).unwrap())));
}

fn herdr_json(args: &[&str]) -> Value {
    let out = Command::new("herdr")
        .args(["--session", "corral-test"])
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(out.status.success(), "herdr {args:?}: {out:?}");
    // Report commands print nothing on success; reads print one JSON result.
    if out.stdout.iter().all(u8::is_ascii_whitespace) {
        return Value::Null;
    }
    serde_json::from_slice::<Value>(&out.stdout).unwrap()["result"].clone()
}

#[test]
fn live_lost_planning_and_assistant_hosts_leave_no_corral_pi_state_on_their_pane() {
    let live = Herdr::new(Some("corral-test".into()));
    // Only the private server scripts/test-herdr.sh starts, never an owner-configured one.
    let private = std::env::var("CORRAL_HERDR_SESSION").as_deref() == Ok("corral-test");
    if !private || !live.available() {
        eprintln!("SKIP: herdr --session corral-test unavailable");
        return;
    }
    for role in ["planner", "plan_reviewer", "assistant"] {
        // Given a bound host of the role in a real pane of the private server, which reported
        // its identity and `working` in its generation's range, as herdr.ts does:
        let mut fx = f::fixture(false, 5);
        let created = live
            .workspace_create(fx.root(), &format!("t605-{role}"), false)
            .unwrap();
        let pane = created.pane_id.clone();
        fx.ctx.herdr = live.clone();
        let host = HostProc::spawn(true);
        let container = bound_role(&fx, role, &pane, &host);
        let base = epoch_of(&fx, &container, 1) << 20;
        let source = ["--source", SOURCE, "--agent", AGENT];
        let (first, second) = ((base + 1).to_string(), (base + 2).to_string());
        let token = format!("corral_role={role}");
        herdr_json(
            &[
                &["pane", "report-metadata", &pane][..],
                &source,
                &[
                    "--applies-to-source",
                    SOURCE,
                    "--display-agent",
                    "corral-pi",
                    "--token",
                    &token,
                    "--seq",
                    &first,
                ],
            ]
            .concat(),
        );
        herdr_json(
            &[
                &["pane", "report-agent", &pane][..],
                &source,
                &["--state", "working", "--seq", &second],
            ]
            .concat(),
        );
        let shown = herdr_json(&["pane", "get", &pane])["pane"].clone();
        assert_eq!(
            (shown["agent"].clone(), shown["agent_status"].clone()),
            (json!(AGENT), json!("working"))
        );

        // When sync records the host lost:
        sync_role(&fx, &container);

        // Then herdr no longer shows corral-pi or its identity there, and the pane stays:
        let after = herdr_json(&["pane", "get", &pane])["pane"].clone();
        let leftover: Vec<&String> = after["tokens"]
            .as_object()
            .map(|tokens| tokens.keys().filter(|k| k.starts_with("corral_")).collect())
            .unwrap_or_default();
        assert_eq!(after["pane_id"], json!(pane), "{role}");
        assert_ne!(after["agent"], json!(AGENT), "{role}");
        assert!(
            leftover.is_empty() && after["display_agent"].is_null(),
            "{role}: {after}"
        );
        live.workspace_close(&created.workspace_id).unwrap();
    }
}
