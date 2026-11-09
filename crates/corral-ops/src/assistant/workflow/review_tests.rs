//! Offline protocol proof: no provider, task, pane or board review is involved.
use super::super::review;
use super::*;
use corral_core::agent::{AgentRole, AuxiliaryOutcome};
use serde_json::{Value, json};

fn fixture() -> Fixture {
    let f = Fixture::new();
    f.planning();
    let config: corral_core::agent_capabilities::AgentCapabilities =
        serde_json::from_value(json!({"assistant":{"subagents":{"enabled":true}}})).unwrap();
    f.fx.ctx
        .store
        .update_assistant_container(&f.session, |c| {
            c.state.generations[0].capabilities = Some(config.resolve(AgentRole::Assistant, None));
            Ok(())
        })
        .unwrap();
    for revision in 1..=3 {
        let expected = f.expected();
        let text = format!("# Draft r{revision}\r\nPrecise UTF-8: café 🦀\n");
        ts::committed(&f.stage(&format!("stage-{revision}"), &expected, 0, &text));
        ts::committed(&f.send(
            &format!("publish-{revision}"),
            "assistant_plan",
            f.publication(&expected, &text),
        ));
    }
    f
}
fn start(f: &Fixture) -> Value {
    ts::committed(&f.send(
        "start-review",
        "assistant_plan_review",
        json!({"action":"start", "expected":f.expected(), "input":f.owner_input()}),
    ))
    .result
}
fn event(f: &Fixture, seq: u64, r: &Value, action: &str, extra: Value) -> CallResponse {
    let mut payload = json!({"kind":"auxiliary", "action":action, "usage_kind":"explore", "request":r["invocation"],
        "plan_review":{"workflow":r["workflow"], "review":r["review"], "digest":r["digest"]}});
    for (key, value) in extra.as_object().unwrap() {
        payload[key] = value.clone();
    }
    ts::send(
        &f.fx,
        &ts::envelope(&f.host, "event", &format!("ev-1-{seq}"), payload, false),
        "event",
    )
}
fn lifecycle(f: &Fixture, seq: u64, r: &Value, value: Value) -> Value {
    ts::committed(&event(f, seq, r, "review", json!({"review_event":value}))).result
}

#[test]
fn assistant_local_review_exact_snapshot_stale_result_terminal_accounting_and_no_board_effects() {
    let f = fixture();
    let original_expected = f.expected();
    let r = start(&f);
    assert_eq!(r["revision"], 3);
    assert_eq!(r["lifecycle"], "prepared");
    let exact = std::fs::read(r["snapshot_file"].as_str().unwrap()).unwrap();
    assert_eq!(BlobHash::of(&exact).as_str(), r["digest"].as_str().unwrap());
    let before = f.container();
    let input = before.owner_inputs.last().unwrap().id.clone();
    let replay = f.send(
        "start-review",
        "assistant_plan_review",
        json!({"action":"start","expected":original_expected,"input":input}),
    );
    assert!(matches!(
        replay,
        CallResponse::Receipt { replayed: true, .. }
    ));
    assert_eq!(
        f.container(),
        before,
        "exact transport replay writes nothing"
    );
    let retry = f.send(
        "fresh-start-review",
        "assistant_plan_review",
        json!({"action":"start", "expected":f.expected(), "input":input}),
    );
    let receipt = ts::committed(&retry);
    assert_eq!(receipt.result["review"], "LR1");
    let mut expected_container = before;
    expected_container.state.receipts.push(receipt);
    assert_eq!(
        f.container(),
        expected_container,
        "new transport adds only its receipt, never a second review"
    );
    assert_eq!(
        lifecycle(&f, 101, &r, json!({"action":"claim"}))["claimed"],
        true
    );
    assert_eq!(
        lifecycle(&f, 102, &r, json!({"action":"claim"}))["claimed"],
        false
    );
    ts::committed(&event(
        &f,
        103,
        &r,
        "prepare",
        json!({"model":"openai-codex/offline-fixture"}),
    ));
    ts::committed(&event(&f, 104, &r, "started", json!({})));
    // Main-session publication remains admitted while inference is held elsewhere.
    let expected = f.expected();
    let r4 = "# r4\nChanged eviction, still no implementation consent.\n";
    ts::committed(&f.stage("stage-r4", &expected, 0, r4));
    ts::committed(&f.send("publish-r4", "assistant_plan", f.publication(&expected, r4)));
    let findings = format!(
        "No blocking findings; src/cache.rs:1 needs verification.\n{}",
        "\0".repeat(1024)
    );
    lifecycle(
        &f,
        105,
        &r,
        json!({"action":"stage", "offset":0,"chunk":findings}),
    );
    ts::committed(&event(
        &f,
        106,
        &r,
        "finish",
        json!({"outcome":"completed"}),
    ));
    let done = lifecycle(
        &f,
        107,
        &r,
        json!({"action":"settle", "lifecycle":"completed", "summary":"Advisory only", "bytes":findings.len(),"digest":BlobHash::of(findings.as_bytes())}),
    );
    assert_eq!(done["revision"], 3);
    assert_eq!(done["current_revision"], 4);
    assert_eq!(done["freshness"], "stale");
    assert_eq!(done["accounting"], "completed");
    assert_eq!(done["source_freshness"], "not_observed");
    let before_read = f.container();
    let mut offset = 0;
    let mut restored = String::new();
    loop {
        let read = ts::committed(&f.send(
            &format!("read-review-{offset}"),
            "assistant_plan_review",
            json!({"action":"read","review":"LR1","offset":offset}),
        ))
        .result;
        assert!(serde_json::to_vec(&read).unwrap().len() <= corral_core::agent::RESULT_MAX_BYTES);
        restored.push_str(read["text"].as_str().unwrap());
        match read["next_offset"].as_u64() {
            Some(next) => offset = next,
            None => break,
        }
    }
    assert_eq!(restored, findings);
    assert_eq!(f.container(), before_read, "bounded reads are receipt-free");
    let terminal = f.container();
    assert!(
        terminal
            .workflow
            .as_ref()
            .unwrap()
            .current
            .execution
            .is_none()
    );
    assert_eq!(terminal.state.usage_sessions.len(), 1);
    assert_eq!(
        terminal.state.usage_sessions[0].outcome,
        Some(AuxiliaryOutcome::Completed)
    );
    let usage_file = &terminal.state.usage_sessions[0].session_file;
    std::fs::write(usage_file, format!("{}\n{}\n",
        json!({"type":"session","id":r["invocation"]}),
        json!({"type":"message","id":"review-result","message":{"role":"assistant","responseId":"review-response","usage":{"input":7,"output":3}}})
    )).unwrap();
    let usage = crate::agent::usage::collect_container(
        &corral_core::agent::ContainerRef::Assistant {
            session: f.session.clone(),
        },
        &f.fx.ctx.store,
    )
    .unwrap();
    assert_eq!(
        (usage.input, usage.output),
        (7, 3),
        "assistant review cost must not be read from PM"
    );
    let late = ts::committed(&event(
        &f,
        108,
        &r,
        "review",
        json!({"review_event":{"action":"settle","lifecycle":"failed","summary":"late"}}),
    ));
    assert_eq!(late.result["lifecycle"], "completed");
    let mut expected_container = terminal;
    expected_container.state.receipts.push(late);
    assert_eq!(
        f.container(),
        expected_container,
        "first terminal wins; late transport adds only its receipt"
    );
    let reopened = corral_core::Store::open(f.fx._tmp.path()).unwrap();
    let container = reopened.get_assistant_container(&f.session).unwrap();
    assert_eq!(
        review::read(&reopened, &f.session, &container, Some("LR1"), 0, 0).unwrap()["freshness"],
        "stale"
    );
    f.no_tasks();
}

#[test]
fn assistant_local_review_does_not_strand_an_in_flight_design_upload() {
    let f = fixture();
    let r = start(&f);
    let old = f.expected();
    let text = "# Draft r4\n";
    ts::committed(&f.stage("concurrent-upload", &old, 0, text));
    lifecycle(&f, 601, &r, json!({"action":"claim"}));
    assert_eq!(
        ts::code(&f.send("stale-publish", "assistant_plan", f.publication(&old, text))),
        Some("stale_revision")
    );
    ts::committed(&f.send(
        "fresh-publish",
        "assistant_plan",
        f.publication(&f.expected(), text),
    ));
    let old = f.expected();
    let text = "# Draft r5\n";
    let split = 4;
    ts::committed(&f.stage("prefix-r5", &old, 0, &text[..split]));
    ts::committed(&event(
        &f,
        602,
        &r,
        "prepare",
        json!({"model":"openai-codex/offline"}),
    ));
    assert_eq!(
        ts::code(&f.stage("stale-suffix", &old, split, &text[split..])),
        Some("stale_revision")
    );
    let fresh = f.expected();
    ts::committed(&f.stage("fresh-suffix", &fresh, split, &text[split..]));
    ts::committed(&f.send("publish-r5", "assistant_plan", f.publication(&fresh, text)));
    assert_eq!(
        f.container()
            .workflow
            .as_ref()
            .unwrap()
            .current
            .current_plan
            .as_ref()
            .unwrap()
            .revision,
        5
    );
    f.no_tasks();
}

#[test]
fn assistant_local_review_cancel_is_terminal_and_forged_host_actions_cannot_start_work() {
    let f = fixture();
    let r = start(&f);
    let stale = f.expected();
    lifecycle(&f, 201, &r, json!({"action":"claim"}));
    ts::committed(&event(
        &f,
        202,
        &r,
        "prepare",
        json!({"model":"openai-codex/offline"}),
    ));
    assert_eq!(
        ts::code(&f.send(
            "stale-cancel",
            "assistant_plan_review",
            json!({"action":"cancel","expected":stale,"review":"LR1"})
        )),
        Some("stale_revision")
    );
    let cancelled = ts::committed(&f.send(
        "cancel",
        "assistant_plan_review",
        json!({"action":"cancel","expected":f.expected(),"review":"LR1"}),
    ))
    .result;
    assert_eq!(cancelled["lifecycle"], "cancelled");
    assert_eq!(
        ts::code(&event(&f, 203, &r, "started", json!({}))),
        Some("bad_request")
    );
    ts::committed(&event(
        &f,
        204,
        &r,
        "finish",
        json!({"outcome":"not_started"}),
    ));
    assert_eq!(
        lifecycle(
            &f,
            205,
            &r,
            json!({"action":"settle","lifecycle":"completed","summary":"late success"})
        )["lifecycle"],
        "cancelled"
    );
    assert_eq!(
        ts::code(&f.send(
            "host-result",
            "assistant_plan_review",
            json!({"action":"settle","lifecycle":"completed"})
        )),
        Some("bad_request")
    );
    assert_eq!(
        ts::code(&f.send(
            "host-fields",
            "assistant_plan_review",
            json!({"action":"read","review":"LR1","generation":1})
        )),
        Some("bad_request")
    );
    assert_eq!(ts::code(&f.send("fake-event", "event", json!({"kind":"auxiliary","action":"review","request":r["invocation"],"usage_kind":"explore",
        "plan_review":{"workflow":"W1","review":"LR1","digest":r["digest"]},"review_event":{"action":"claim"}}))), Some("bad_request"));
    assert_eq!(
        f.container().state.usage_sessions[0].outcome,
        Some(AuxiliaryOutcome::NotStarted)
    );
    f.no_tasks();
}

#[test]
fn assistant_local_review_failure_timeout_and_incomplete_keep_usage_honest() {
    for (status, accounting) in [
        ("failed", "uncertain"),
        ("timed_out", "uncertain"),
        ("incomplete", "completed"),
        ("unavailable", "not_started"),
    ] {
        let f = fixture();
        let r = start(&f);
        if status != "unavailable" {
            lifecycle(&f, 501, &r, json!({"action":"claim"}));
            ts::committed(&event(
                &f,
                502,
                &r,
                "prepare",
                json!({"model":"openai-codex/offline"}),
            ));
            ts::committed(&event(&f, 503, &r, "started", json!({})));
            ts::committed(&event(&f, 504, &r, "finish", json!({"outcome":accounting})));
        }
        if status == "incomplete" {
            assert_eq!(
                ts::code(&event(
                    &f,
                    505,
                    &r,
                    "review",
                    json!({"review_event":{"action":"settle","lifecycle":"completed","summary":"missing findings"}})
                )),
                Some("bad_request")
            );
            let unusable = "\0 \n";
            lifecycle(
                &f,
                508,
                &r,
                json!({"action":"stage","offset":0,"chunk":unusable}),
            );
            assert_eq!(
                ts::code(&event(
                    &f,
                    509,
                    &r,
                    "review",
                    json!({"review_event":{"action":"settle","lifecycle":"completed","summary":"control-only output","bytes":unusable.len(),"digest":BlobHash::of(unusable.as_bytes())}})
                )),
                Some("bad_request")
            );
        }
        let done = lifecycle(
            &f,
            506,
            &r,
            json!({"action":"settle","lifecycle":status,"summary":"No successful review claimed"}),
        );
        assert_eq!(done["lifecycle"], status);
        assert_eq!(done["accounting"], accounting);
        assert!(done["findings"].is_null());
        assert_eq!(
            lifecycle(&f, 507, &r, json!({"action":"claim"}))["claimed"],
            false
        );
        f.no_tasks();
    }
}

#[test]
fn assistant_local_review_source_and_revision_freshness_are_independent() {
    let mut f = fixture();
    let repo = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .arg("-C")
            .arg(repo.path())
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@invalid",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    };
    git(&["init", "-q"]);
    std::fs::write(repo.path().join("cache.rs"), "old\n").unwrap();
    git(&["add", "cache.rs"]);
    git(&["commit", "-qm", "fixture"]);
    let head = git(&["rev-parse", "HEAD"]);
    f.fx.ctx.store.config.projects.get_mut("app").unwrap().path =
        repo.path().to_string_lossy().into_owned();
    let expected = f.expected();
    let text = "# Ready design\nChange cache.rs.\n";
    ts::committed(&f.stage("ready-stage", &expected, 0, text));
    let mut p = f.publication(&expected, text);
    p["stage"] = json!("ready");
    p["execution_scope"] =
        json!({"project":"app","paths":["cache.rs"],"actions":["change eviction"]});
    p["assumptions"] = json!({"project":"app","source_head":head,"base_head":head,"files":[{"path":"cache.rs","sha256":BlobHash::of(b"old\n")}]});
    ts::committed(&f.send("ready-publish", "assistant_plan", p));
    let r = start(&f);
    assert_eq!(r["source_freshness"], "current");
    std::fs::write(repo.path().join("cache.rs"), "dirty\n").unwrap();
    let read = |id| {
        ts::committed(&f.send(
            id,
            "assistant_plan_review",
            json!({"action":"read","review":"LR1"}),
        ))
        .result
    };
    assert_eq!(read("dirty")["source_freshness"], "changed");
    assert_eq!(read("revision")["freshness"], "current");
    std::fs::write(repo.path().join("cache.rs"), "old\n").unwrap();
    assert_eq!(read("restored")["source_freshness"], "current");
    git(&["commit", "--allow-empty", "-qm", "new base"]);
    assert_eq!(read("head-drift")["source_freshness"], "changed");
    f.no_tasks();
}

// Explicit slow profile: reopen the disposable service at every review publication boundary.
#[test]
fn assistant_local_review_restart_boundaries_never_replay_inference() {
    for boundary in 0..=7 {
        let mut f = fixture();
        let r = start(&f);
        if boundary >= 1 {
            lifecycle(&f, 301, &r, json!({"action":"claim"}));
        }
        if boundary >= 2 {
            ts::committed(&event(
                &f,
                302,
                &r,
                "prepare",
                json!({"model":"openai-codex/offline"}),
            ));
        }
        if boundary >= 3 {
            ts::committed(&event(&f, 303, &r, "started", json!({})));
        }
        let text = "Advisory findings.\n";
        if boundary >= 4 {
            lifecycle(
                &f,
                304,
                &r,
                json!({"action":"stage","offset":0,"chunk":text}),
            );
        }
        if boundary >= 5 {
            ts::committed(&event(
                &f,
                305,
                &r,
                "finish",
                json!({"outcome":"completed"}),
            ));
        }
        if boundary >= 6 {
            let done = lifecycle(
                &f,
                306,
                &r,
                json!({"action":"settle","lifecycle":"completed","summary":"r3 only","bytes":text.len(),"digest":BlobHash::of(text.as_bytes())}),
            );
            if boundary >= 7 {
                lifecycle(
                    &f,
                    307,
                    &r,
                    json!({"action":"delivered","result_revision":done["result_revision"]}),
                );
            }
        }
        let before = f.container();
        let read = ts::committed(&f.send(
            "before-restart",
            "assistant_plan_review",
            json!({"action":"read","review":"LR1"}),
        ))
        .result;
        assert_eq!(read["publication_pending"], boundary == 5);
        f.fx.ctx.store = corral_core::Store::open(f.fx._tmp.path()).unwrap();
        let recovered = lifecycle(&f, 308, &r, json!({"action":"recover"}));
        assert_eq!(
            recovered["lifecycle"],
            if boundary >= 6 {
                "completed"
            } else {
                "interrupted"
            }
        );
        assert_eq!(
            recovered["accounting"],
            if boundary >= 5 {
                "completed"
            } else if boundary >= 3 {
                "uncertain"
            } else {
                "not_started"
            }
        );
        assert_eq!(
            lifecycle(&f, 309, &r, json!({"action":"claim"}))["claimed"],
            false
        );
        let read = ts::committed(&f.send(
            "read-after-restart",
            "assistant_plan_review",
            json!({"action":"read","review":"LR1"}),
        ))
        .result;
        assert_eq!(read["revision"], 3);
        assert_eq!(read["freshness"], "current");
        assert_eq!(read["notice_delivered"], boundary >= 7);
        assert_eq!(read["publication_pending"], false);
        assert_eq!(
            f.container().state.usage_sessions.len(),
            before.state.usage_sessions.len()
        );
        assert_eq!(
            f.container()
                .workflow
                .as_ref()
                .unwrap()
                .current
                .reviews
                .len(),
            1
        );
        f.no_tasks();
    }
    // Inject an exception after staging a lifecycle row but before atomic state/receipt commit.
    let f = fixture();
    let r = start(&f);
    let before = f.container();
    FAILPOINT.with_borrow_mut(|hook| {
        *hook = Some(Box::new(|point| {
            if point == "local_review_staged" {
                panic!("before review commit");
            }
        }))
    });
    let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        lifecycle(&f, 401, &r, json!({"action":"claim"}))
    }));
    FAILPOINT.with_borrow_mut(|hook| *hook = None);
    assert!(crash.is_err());
    assert_eq!(f.container(), before);
    assert_eq!(
        lifecycle(&f, 402, &r, json!({"action":"recover"}))["lifecycle"],
        "interrupted"
    );
    assert_eq!(
        lifecycle(&f, 403, &r, json!({"action":"claim"}))["claimed"],
        false
    );
    f.no_tasks();
    // A crash after snapshot publication but before the very first reservation leaves only
    // complete immutable files; an explicit retry can reuse them without duplicating a row.
    let mut f = fixture();
    let input = f.owner_input();
    let expected = f.expected();
    let before = f.container();
    FAILPOINT.with_borrow_mut(|hook| {
        *hook = Some(Box::new(|point| {
            if point == "local_review_staged" {
                panic!("first publication");
            }
        }))
    });
    let crash = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        f.send(
            "first-publication",
            "assistant_plan_review",
            json!({"action":"start","expected":expected,"input":input}),
        )
    }));
    FAILPOINT.with_borrow_mut(|hook| *hook = None);
    assert!(crash.is_err());
    assert_eq!(f.container(), before);
    f.fx.ctx.store = corral_core::Store::open(f.fx._tmp.path()).unwrap();
    let r = ts::committed(&f.send(
        "explicit-retry",
        "assistant_plan_review",
        json!({"action":"start","expected":expected,"input":input}),
    ))
    .result;
    assert_eq!(r["review"], "LR1");
    assert_eq!(r["lifecycle"], "prepared");
    assert_eq!(
        BlobHash::of(&std::fs::read(r["snapshot_file"].as_str().unwrap()).unwrap()).as_str(),
        r["digest"].as_str().unwrap()
    );
    assert_eq!(
        f.container()
            .workflow
            .as_ref()
            .unwrap()
            .current
            .reviews
            .len(),
        1
    );
    f.no_tasks();
}
