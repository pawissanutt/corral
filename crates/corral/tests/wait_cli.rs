use std::process::{Command, Output, Stdio};

// Nonzero CLI timeouts below are hang guards for exact retained events, not timing assertions.
fn invoke(root: &std::path::Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_corral"));
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("CORRAL_") || name.starts_with("HERDR_") {
            command.env_remove(key);
        }
    }
    command
        .env("CORRAL_ROOT", root)
        .env("HERDR_BIN_PATH", "/nonexistent/herdr")
        .arg("--root")
        .arg(root)
        .arg("wait")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn configuration_events_are_readable_by_harnesses_in_a_new_process() {
    let dir = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(dir.path()).unwrap();
    store
        .edit_config(|doc| doc.set_str("ui.mouse", "false"))
        .unwrap();
    let output = invoke(
        dir.path(),
        &[
            "--for",
            "config_changed",
            "--after-offset",
            "0",
            "--timeout",
            "120s",
            "--json",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    let event: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(event["kind"], "config_changed");
    assert_eq!(event["detail"]["pm_relevant"], false);
    assert_eq!(event["detail"]["changes"][0]["key"], "ui.mouse");
}

#[test]
fn timeout_exits_124_without_output() {
    let dir = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(dir.path()).unwrap();
    store
        .log_event(Some("T-001"), "done", serde_json::json!({}))
        .unwrap();
    let output = invoke(dir.path(), &["--timeout", "0ms"]);
    assert_eq!(output.status.code(), Some(124));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn after_offset_is_gapless_across_two_processes() {
    let dir = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(dir.path()).unwrap();
    store
        .log_event(Some("T-001"), "created", serde_json::json!({}))
        .unwrap();
    store
        .log_event(
            Some("T-001"),
            "done",
            serde_json::json!({"run":"R1","added":3,"removed":1}),
        )
        .unwrap();
    store
        .log_event(
            Some("T-002"),
            "autopilot",
            serde_json::json!({"to":"escalated"}),
        )
        .unwrap();
    let first = invoke(
        dir.path(),
        &[
            "--after-offset",
            "0",
            "--json",
            "--print-cursor",
            "--timeout",
            "120s",
        ],
    );
    assert!(first.status.success(), "{first:?}");
    assert_eq!(String::from_utf8_lossy(&first.stdout).lines().count(), 1);
    let value: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(value["kind"], "done");
    let offset = value["after_offset"].as_u64().unwrap().to_string();
    let second = invoke(
        dir.path(),
        &[
            "--after-offset",
            &offset,
            "--for",
            "escalated",
            "--timeout",
            "120s",
        ],
    );
    assert!(second.status.success(), "{second:?}");
    assert_eq!(
        String::from_utf8(second.stdout).unwrap(),
        "T-002 escalated\n"
    );
}

#[test]
fn since_replays_history_and_normalizes_task_id_with_project_filter() {
    let dir = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(dir.path()).unwrap();
    let task = store
        .create("target", Some("app"), corral_core::Status::Review, "")
        .unwrap();
    store
        .log_event(
            Some(&task.id),
            "autopilot",
            serde_json::json!({"to":"blocked"}),
        )
        .unwrap();
    let output = invoke(
        dir.path(),
        &[
            "--since",
            "2000-01-01T00:00:00Z",
            "--task",
            "1",
            "--project",
            "app",
            "--for",
            "blocked",
            "--json",
            "--timeout",
            "120s",
        ],
    );
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["task"], task.id);
    assert_eq!(value["detail"]["to"], "blocked");
    assert!(value.get("cursor").is_none());
}

fn outcome_subscription(kind: &str, explicit: bool) {
    use corral_core::{
        ObligationSource, ReceiptOutcome, ResumeDelivery, ResumeReceipt, Run, Status, WaitHandle,
        WaitRequest,
    };

    // Given a disposable board and a transport that cannot reach any herdr server.
    let dir = tempfile::tempdir().unwrap();
    let store = corral_core::Store::init(dir.path()).unwrap();
    let task = store.create("Held work", None, Status::Ready, "").unwrap();
    let ctx = corral_ops::Ctx {
        store,
        herdr: corral_herdr::Herdr::with_bin(dir.path().join("absent-herdr"), None),
        resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
    };
    let now = "2026-10-18T12:00:00Z".parse().unwrap();
    let request = WaitRequest {
        version: 1,
        id: "external".into(),
        run: "R1".into(),
        condition: "external outcome".into(),
        owner: "fixture".into(),
        source: ObligationSource::Worker,
        deadline: Some("2026-10-18T13:00:00Z".parse().unwrap()),
        handle: WaitHandle::Channel {
            channel_id: "fixture:external".into(),
            capability: Some("wait_v1".into()),
        },
    };
    let decision = if kind == "pm_decision_resolved" {
        ctx.store
            .update(&task.id, |task| {
                task.dispatch_hold = true;
                Ok(())
            })
            .unwrap();
        Some(
            corral_ops::pm_workset(&ctx)
                .unwrap()
                .decisions
                .into_iter()
                .find(|decision| decision.task == task.id)
                .unwrap()
                .id,
        )
    } else {
        ctx.store
            .update(&task.id, |task| {
                task.status = Status::Running;
                task.runs
                    .push(serde_json::from_value::<Run>(serde_json::json!({
                        "id":"R1", "agent":"omo", "worktree":dir.path(),
                        "branch":"corral/T-001", "base":"main", "base_sha":"fixture",
                        "started":now
                    }))?);
                Ok(())
            })
            .unwrap();
        if kind == "run_resumed" {
            corral_ops::register_wait(&ctx, &task.id, request.clone(), now).unwrap();
        }
        None
    };
    // Capture the resumable subscription BEFORE the action, not after its event.
    let cursor = corral_core::EventReader::open(&ctx.store.dir().join("events.jsonl"), None)
        .unwrap()
        .cursor()
        .offset
        .to_string();

    // When the real producer persists the outcome (no live panes or grants).
    match kind {
        "run_resumed" => {
            corral_ops::resume_run(
                &ctx,
                &task.id,
                corral_ops::ResumeTarget::Wait("external"),
                ResumeReceipt {
                    version: 1,
                    id: "receipt-1".into(),
                    run: "R1".into(),
                    outcome: ReceiptOutcome::Ready,
                    delivery: ResumeDelivery::Acknowledged,
                    observed_at: now,
                    answer: None,
                    data: serde_json::json!({}),
                },
                now,
            )
            .unwrap();
        }
        "wait_transition" => {
            let mut unsupported = request;
            unsupported.handle = WaitHandle::Channel {
                channel_id: "fixture:external".into(),
                capability: None,
            };
            corral_ops::register_wait(&ctx, &task.id, unsupported, now).unwrap();
        }
        "pm_decision_resolved" => {
            corral_ops::pm_resolve(&ctx, decision.as_deref().unwrap(), "Keep held").unwrap();
        }
        _ => panic!("unexpected outcome"),
    }
    let mut args = vec![
        "--after-offset",
        &cursor,
        "--timeout",
        "1s",
        "--json",
        "--print-cursor",
    ];
    if explicit {
        args.extend(["--for", kind]);
    }
    let output = invoke(dir.path(), &args);
    // Then the actual CLI consumes exactly that outcome, using either subscription form.
    assert!(
        output.status.success(),
        "{kind} explicit={explicit}: {output:?}"
    );
    let event: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(event["kind"], kind);
    assert_eq!(event["task"], task.id);
    assert!(event["after_offset"].as_u64().unwrap() > cursor.parse::<u64>().unwrap());
    assert_eq!(String::from_utf8_lossy(&output.stdout).lines().count(), 1);
}

#[test]
fn run_resumed_explicit_subscription() {
    outcome_subscription("run_resumed", true);
}

#[test]
fn run_resumed_default_subscription() {
    outcome_subscription("run_resumed", false);
}

#[test]
fn wait_transition_explicit_subscription() {
    outcome_subscription("wait_transition", true);
}

#[test]
fn wait_transition_default_subscription() {
    outcome_subscription("wait_transition", false);
}

#[test]
fn pm_decision_resolved_explicit_subscription() {
    outcome_subscription("pm_decision_resolved", true);
}

#[test]
fn pm_decision_resolved_default_subscription() {
    outcome_subscription("pm_decision_resolved", false);
}

#[test]
fn presentation_and_policy_accounting_cannot_self_wake() {
    // Given a cursor before each accounting fixture, followed by a known actionable sentinel.
    for kind in [
        "pm_decision_presented",
        "pm_policy_delivery",
        "pm_event_delivery",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let store = corral_core::Store::init(dir.path()).unwrap();
        let path = store.dir().join("events.jsonl");
        let cursor = corral_core::EventReader::open(&path, None)
            .unwrap()
            .cursor()
            .offset
            .to_string();
        store
            .log_event(
                Some("T-001"),
                kind,
                serde_json::json!({"event_kind":"run_resumed","submitted":true,"confirmed":true}),
            )
            .unwrap();
        store
            .log_event(Some("T-002"), "done", serde_json::json!({}))
            .unwrap();
        let before = std::fs::read(&path).unwrap();
        // When the real CLI consumes the stream with default and explicit subscriptions.
        for explicit in [false, true] {
            let mut args = vec!["--after-offset", &cursor, "--timeout", "1s", "--json"];
            if explicit {
                args.extend(["--for", "done"]);
            }
            let output = invoke(dir.path(), &args);
            // Then accounting is skipped, and consuming the stream creates no new events.
            assert!(output.status.success(), "{kind}: {output:?}");
            let event: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(event["kind"], "done");
            assert_eq!(event["task"], "T-002");
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
        let rejected = invoke(dir.path(), &["--for", kind, "--timeout", "0ms"]);
        assert_eq!(rejected.status.code(), Some(2));
    }
}
