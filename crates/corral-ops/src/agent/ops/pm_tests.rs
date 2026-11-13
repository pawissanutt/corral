use super::*;
use crate::agent::receipts::FAILPOINT;
use crate::agent::{self, BindTarget, test_support as ts};
use corral_core::agent::{ContainerRef, pm_state};

fn fixture() -> (ts::Fx, ts::Host) {
    let mut fx = ts::fx();
    for project in ["corral", "other"] {
        fx.ctx.store.config.projects.insert(
            project.into(),
            serde_json::from_value(json!({"path":fx.ctx.store.root()})).unwrap(),
        );
    }
    fx.ctx.store.save_config().unwrap();
    let host = ts::bind(
        &fx,
        &BindTarget::Pm {
            project: "corral".into(),
        },
        "pm-pane",
        "pm-session",
    );
    ts::committed(&ts::hello(&fx, &host));
    (fx, host)
}

fn send(fx: &ts::Fx, host: &ts::Host, op: &str, id: &str, payload: Value) -> CallResponse {
    let id = agent::tool_request_id(&host.identity.session_id, id);
    ts::send(fx, &ts::envelope(host, op, &id, payload, false), op)
}

fn task(fx: &ts::Fx, project: &str, body: &str) -> Task {
    fx.ctx
        .store
        .create(project, Some(project), Status::Backlog, body)
        .unwrap()
}

#[test]
fn inbox_limits_distinguish_items_from_body_characters_without_mutating() {
    let (fx, host) = fixture();
    fx.ctx
        .store
        .pm_inbox_post(corral_core::pm_inbox::NewPmItem {
            from: "owner".into(),
            kind: corral_core::pm_inbox::ItemKind::Question,
            subject: "Bounds".into(),
            body: "x".repeat(2048),
            refs: vec![],
            requested_action: None,
            owner_words: None,
            key: None,
        })
        .unwrap();
    let before = fx.ctx.store.pm_inbox_item("PMI-1").unwrap();
    for limit in [1, 100, 101, 1024] {
        let response = send(
            &fx,
            &host,
            "inbox_read",
            &format!("body-{limit}"),
            json!({"id":"PMI-1", "limit":limit}),
        );
        let receipt = response.receipt().unwrap();
        assert_eq!(receipt.state, ReceiptState::Committed);
        assert_eq!(
            receipt.result["content_json"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            limit
        );
        assert_eq!(receipt.result["next_offset"], limit);
    }
    for limit in [1, 100] {
        let response = send(
            &fx,
            &host,
            "inbox_read",
            &format!("list-{limit}"),
            json!({"limit":limit}),
        );
        assert_eq!(response.receipt().unwrap().state, ReceiptState::Committed);
    }
    for input in [
        json!({"limit":0}),
        json!({"limit":101}),
        json!({"id":"PMI-1", "limit":0}),
        json!({"id":"PMI-1", "limit":1025}),
        json!({"offset":-1}),
    ] {
        let response = send(&fx, &host, "inbox_read", "invalid-bounds", input);
        assert_eq!(ts::code(&response), Some("bad_request"));
    }
    let response = send(
        &fx,
        &host,
        "inbox_resolve",
        "missing-outcome",
        json!({"id":"PMI-1", "action":"resolve", "expected_revision":0}),
    );
    let receipt = response.receipt().unwrap();
    assert_eq!(receipt.state, ReceiptState::Rejected);
    assert_eq!(receipt.result["code"], "bad_request");
    assert_eq!(before, fx.ctx.store.pm_inbox_item("PMI-1").unwrap());
}

#[test]
fn inbox_tools_claim_and_resolve_at_revisions_and_refuse_a_stale_claim() {
    // Given a bound corral-pi PM and owner item PMI-1 at revision 0.
    let (fx, host) = fixture();
    fx.ctx
        .store
        .pm_inbox_post(corral_core::pm_inbox::NewPmItem {
            from: "owner".into(),
            kind: corral_core::pm_inbox::ItemKind::Question,
            subject: "Which cache?".into(),
            body: "Context for the PM.".into(),
            refs: vec![],
            requested_action: None,
            owner_words: None,
            key: None,
        })
        .unwrap();
    let resolve = |id: &str, payload: Value| send(&fx, &host, "inbox_resolve", id, payload);
    // When it reads the open page, claims at 0 (and replays that call), resolves at 1 and
    // then claims at revision 0 again.
    let page = send(&fx, &host, "inbox_read", "page", json!({}));
    let claim = json!({"id":"PMI-1","expected_revision":0,"action":"claim"});
    let claimed = resolve("claim", claim.clone());
    let replayed = resolve("claim", claim.clone());
    let resolved = resolve(
        "resolve",
        json!({"id":"PMI-1","expected_revision":1,"action":"resolve",
            "outcome":"done","text":"Filed T-9.","refs":["T-9"]}),
    );
    let stale = resolve("stale", claim);
    let shown = send(&fx, &host, "inbox_read", "show", json!({"id":"PMI-1"}));
    // Then the page lists the item, the claim and resolve commit revisions 1 and 2 once each,
    // and the stale claim is a rejected stale_revision receipt that changed nothing.
    assert_eq!(ts::committed(&page).result["items"][0]["id"], "PMI-1");
    assert_eq!(ts::committed(&claimed).result["revision"], 1);
    assert_eq!(claimed.receipt(), replayed.receipt());
    assert_eq!(ts::committed(&resolved).result["revision"], 2);
    let stale = stale.receipt().unwrap();
    assert_eq!(stale.state, ReceiptState::Rejected);
    assert_eq!(stale.result["code"], "stale_revision");
    let item = fx.ctx.store.pm_inbox_item("PMI-1").unwrap();
    assert_eq!(item.revision, 2);
    let requests: Vec<_> = item.history.iter().map(|t| t.request.clone()).collect();
    assert_eq!(
        requests,
        [
            Some(claimed.receipt().unwrap().request_id.clone()),
            Some(resolved.receipt().unwrap().request_id.clone())
        ]
    );
    let shown = ts::committed(&shown).result;
    assert_eq!(shown["item"]["state"], "resolved");
    assert_eq!(shown["outcome"], "done");
    let content: Value = serde_json::from_str(shown["content_json"].as_str().unwrap()).unwrap();
    assert_eq!(content["outcome"]["text"], "Filed T-9.");
    assert_eq!(content["body"], "Context for the PM.");
}

#[test]
fn inbox_reads_fit_one_frame_for_the_largest_accepted_item() {
    use corral_core::pm_inbox::{ItemKind, NewPmItem, RequestedAction, Resolution, owner_words};
    // Given the review's F2 item: 32 refs of 191 bytes, a 33-word requested command, a quote
    // and a body that JSON-escapes and spans pages, resolved with a 4096-byte outcome and refs.
    let (fx, host) = fixture();
    fx.ctx
        .store
        .create_assistant("pi", None, None, None)
        .unwrap();
    let refs: Vec<String> = (0..32).map(|i| format!("{i}{}", "r".repeat(190))).collect();
    let words: Vec<String> = (0..31).map(|i| format!("{i}{}", "a".repeat(190))).collect();
    let action = RequestedAction::parse(&format!("approve T-1 {}", words.join(" "))).unwrap();
    let body = "\"\u{fc}".repeat(1500);
    fx.ctx
        .store
        .pm_inbox_post(NewPmItem {
            from: "assistant:A-1".into(),
            kind: ItemKind::ActionRequest,
            subject: "Batch request".into(),
            body: body.clone(),
            refs: refs.clone(),
            requested_action: Some(action.clone()),
            owner_words: Some(owner_words("q".repeat(1000), None)),
            key: None,
        })
        .unwrap();
    fx.ctx.store.pm_inbox_claim("PMI-1", 0, None).unwrap();
    let text = "x".repeat(4096);
    fx.ctx
        .store
        .pm_inbox_resolve("PMI-1", 1, (Resolution::Done, &text, refs.clone()), None)
        .unwrap();
    // When the PM lists one item per page, then reads the item by ID page by page.
    let page = send(
        &fx,
        &host,
        "inbox_read",
        "page",
        json!({"state":"resolved","limit":1}),
    );
    let mut content = String::new();
    let mut offset = 0;
    loop {
        let read = send(
            &fx,
            &host,
            "inbox_read",
            &format!("show-{offset}"),
            json!({"id":"PMI-1","offset":offset}),
        );
        let result = ts::committed(&read).result;
        content.push_str(result["content_json"].as_str().unwrap());
        match result["next_offset"].as_u64() {
            Some(next) => offset = usize::try_from(next).unwrap(),
            None => break,
        }
    }
    // Then every page fits one result frame, the summary counts every ref, and the pages
    // reassemble the complete item.
    let listed = &ts::committed(&page).result["items"][0];
    assert_eq!(
        (listed["id"].as_str(), listed["refs_total"].as_u64()),
        (Some("PMI-1"), Some(32))
    );
    let full: Value = serde_json::from_str(&content).unwrap();
    assert_eq!(full["body"], body);
    assert_eq!(full["refs"], json!(refs));
    assert_eq!(full["requested_action"], json!(action));
    assert_eq!(full["outcome"]["text"], text);
    assert_eq!(full["outcome"]["refs"], json!(refs));
}

#[test]
fn comment_replays_after_fencing_without_a_second_effect() {
    // Given a bound PM and a task in its scope.
    let (fx, host) = fixture();
    let task = task(&fx, "corral", "");
    let payload = json!({"task":task.id,"text":"technical context"});
    // When its acknowledgement is lost and its generation is fenced.
    let first = send(&fx, &host, "comment", "comment", payload.clone());
    agent::fence_generation(&fx.ctx.store, &ContainerRef::Pm, 1, "fixture").unwrap();
    let replay = send(&fx, &host, "comment", "comment", payload);
    // Then the stored task-local receipt is returned and exactly one comment exists.
    assert_eq!(first.receipt(), replay.receipt());
    assert_eq!(fx.ctx.store.get(&task.id).unwrap().comments.len(), 1);
    assert!(matches!(
        replay,
        CallResponse::Receipt { replayed: true, .. }
    ));
}

#[test]
fn capture_replays_its_marker_without_creating_another_task() {
    // Given an authenticated PM and a capture in its project.
    let (fx, host) = fixture();
    let payload =
        json!({"project":"corral","title":"captured","body":"finite spec","difficulty":"routine"});
    // When the same request is submitted twice.
    let first = send(&fx, &host, "capture", "capture", payload.clone());
    let replay = send(&fx, &host, "capture", "capture", payload);
    // Then the canonical creation marker and one stored result survive the lost ack.
    assert_eq!(first.receipt(), replay.receipt());
    assert_eq!(
        fx.ctx.store.list().unwrap()[0].difficulty,
        Some(corral_core::TaskDifficulty::Routine)
    );
    let tasks = fx.ctx.store.list().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(
        tasks[0].agent_origin.as_ref().unwrap().request_id,
        first.receipt().unwrap().request_id
    );
    assert_eq!(tasks[0].status, Status::Backlog);
}

#[test]
fn mutations_refuse_other_projects_and_owner_authority() {
    // Given eligible tasks in two projects and the PM scoped to only one.
    let (fx, host) = fixture();
    let inside = task(&fx, "corral", "");
    let outside = task(&fx, "other", "");
    // When the PM attempts each foreign target and an owner-only status.
    for op in ["comment", "reply"] {
        let reply = send(&fx, &host, op, op, json!({"task":outside.id,"text":"no"}));
        assert_eq!(ts::code(&reply), Some("out_of_scope"));
    }
    let foreign = send(
        &fx,
        &host,
        "capture",
        "foreign",
        json!({"project":"other","title":"no","body":""}),
    );
    let ready = send(
        &fx,
        &host,
        "capture",
        "ready",
        json!({"project":"corral","title":"no","body":"","status":"ready"}),
    );
    // Then both projects and their existing records stay unchanged.
    assert_eq!(ts::code(&foreign), Some("out_of_scope"));
    assert_eq!(ts::code(&ready), Some("bad_request"));
    assert_eq!(fx.ctx.store.list().unwrap().len(), 2);
    assert!(fx.ctx.store.get(&outside.id).unwrap().comments.is_empty());
    assert!(fx.ctx.store.get(&inside.id).unwrap().comments.is_empty());
}

#[test]
fn capture_rechecks_scope_in_the_creation_transaction() {
    // Given admission in project corral, followed by a binding scope change.
    let (fx, host) = fixture();
    let store = fx.ctx.store.clone();
    FAILPOINT.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move |name| {
            if name == "pm_reserved_before_effect" {
                pm_state::update(&store, |pm| {
                    pm.as_mut().unwrap().state.binding.project = "other".into();
                    Ok(((), true))
                })
                .unwrap();
            }
        }))
    });
    // When the capture reaches its initializer under the creation lock.
    let reply = send(
        &fx,
        &host,
        "capture",
        "scope-race",
        json!({"project":"corral","title":"no","body":""}),
    );
    FAILPOINT.with(|slot| *slot.borrow_mut() = None);
    // Then no task is created and the reservation reports the explicit scope refusal.
    let receipt = reply.receipt().unwrap();
    assert_eq!(receipt.state, ReceiptState::Rejected);
    assert_eq!(receipt.result["code"], "out_of_scope");
    assert!(fx.ctx.store.list().unwrap().is_empty());
}

#[test]
fn comment_rechecks_generation_in_its_commit_transaction() {
    // Given admission before an old PM generation is fenced.
    let (fx, host) = fixture();
    let task = task(&fx, "corral", "");
    let store = fx.ctx.store.clone();
    FAILPOINT.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move |name| {
            if name == "pm_admitted_before_commit" {
                agent::fence_generation(&store, &ContainerRef::Pm, 1, "fixture").unwrap();
            }
        }))
    });
    // When a comment reaches its existing operation's final transaction.
    let reply = send(
        &fx,
        &host,
        "comment",
        "fence-race",
        json!({"task":task.id,"text":"no"}),
    );
    FAILPOINT.with(|slot| *slot.borrow_mut() = None);
    // Then neither the comment nor its task-local receipt commits.
    assert_eq!(ts::code(&reply), Some("agent_fenced"));
    let saved = fx.ctx.store.get(&task.id).unwrap();
    assert!(saved.comments.is_empty() && saved.agent_pm_receipts.is_empty());
}

#[path = "pm_next_tests.rs"]
mod next_tests;

#[test]
fn codemode_allowlisted_reads_leave_no_receipts_or_request_id_obligations() {
    let (fx, host) = fixture();
    let task = task(&fx, "corral", "finite spec");
    fx.ctx
        .store
        .pm_inbox_post(corral_core::pm_inbox::NewPmItem {
            from: "owner".into(),
            kind: corral_core::pm_inbox::ItemKind::Question,
            subject: "Read only".into(),
            body: "No claim or resolution".into(),
            refs: vec![],
            requested_action: None,
            owner_words: None,
            key: None,
        })
        .unwrap();
    let before = pm_state::load(&fx.ctx.store).unwrap().unwrap();
    let bytes = ts::task_bytes(&fx, &task.id);
    let inbox = fx.ctx.store.pm_inbox_item("PMI-1").unwrap();
    // Mirrors NESTED_CORRAL_READ_OPS in the Pi host. Reuse one request ID for different
    // operations/payloads: reads must not reserve it, replay a result, or store a digest.
    for (op, payload) in [
        ("read_brief", json!({})),
        ("read_next", json!({})),
        ("read_show", json!({"task":task.id})),
        ("read_list", json!({"limit":1})),
        ("read_search", json!({"query":"finite", "limit":1})),
        ("read_route", json!({"task":task.id})),
        ("read_artifacts", json!({"task":task.id})),
        ("read_lessons", json!({})),
        ("inbox_read", json!({"id":"PMI-1"})),
    ] {
        for _ in 0..2 {
            let response = send(&fx, &host, op, "shared-read-id", payload.clone());
            assert_eq!(ts::committed(&response).op, op, "{op}");
            assert!(
                matches!(
                    response,
                    CallResponse::Receipt {
                        replayed: false,
                        ..
                    }
                ),
                "{op}"
            );
            assert_eq!(
                before,
                pm_state::load(&fx.ctx.store).unwrap().unwrap(),
                "{op}"
            );
            assert_eq!(bytes, ts::task_bytes(&fx, &task.id), "{op}");
            assert_eq!(inbox, fx.ctx.store.pm_inbox_item("PMI-1").unwrap(), "{op}");
        }
    }
}

#[test]
fn exact_question_and_automation_reads_preserve_identity_and_state() {
    let (fx, host) = fixture();
    let id = ts::worker_task(&fx, "worker");
    let worker = ts::bind(
        &fx,
        &BindTarget::Worker {
            task: id.clone(),
            run: "R1".into(),
        },
        "worker",
        "worker-session",
    );
    ts::committed(&ts::hello(&fx, &worker));
    let asked = send(
        &fx,
        &worker,
        "ask",
        "ask",
        json!({"kind":"clarification","text":"Which bounded read?","blocking":false}),
    );
    let key = asked.receipt().unwrap().result["key"].as_str().unwrap();
    fx.ctx
        .store
        .update(&id, |task| {
            let run = &mut task.runs[0];
            run.autopilot = Some(corral_core::AutopilotState {
                stage: corral_core::AutoStage::Blocked,
                reason: Some("landing refused: dirty destination".into()),
                sha: Some("abc123".into()),
                at: Timestamp::now(),
            });
            let state = run.agent_host.as_mut().unwrap();
            state.questions[0].revision = 3;
            for n in 2..18 {
                let mut historical = state.questions[0].clone();
                historical.key = format!("{id}/R1/Q{n}");
                historical.text = "unrelated historical question".repeat(100);
                state.questions.push(historical);
            }
            Ok(())
        })
        .unwrap();
    let op: crate::PostLandOperation = serde_json::from_value(json!({
        "version":1,"key":"pending-exact-operation","project":"corral",
        "repo":"/tmp/repo","task":id,"run":"R1","base":"main",
        "base_before":"old","landed_sha":"abc123","policy_digest":"policy",
        "created":"2026-10-23T00:00:00Z","landed":false,"abandoned":false,
        "notified":false,"hooks":[]
    }))
    .unwrap();
    crate::hooks::save(&fx.ctx, &op).unwrap();
    let operations_before =
        serde_json::to_value(crate::post_land_operations(&fx.ctx, Some("corral")).unwrap())
            .unwrap();
    let before = ts::task_bytes(&fx, &id);
    let shown = send(
        &fx,
        &host,
        "read_show",
        "exact",
        json!({"task":id,"section":"questions","key":key,"limit":4096}),
    );
    let result = &shown.receipt().unwrap().result;
    let view: Value = serde_json::from_str(result["content_json"].as_str().unwrap()).unwrap();
    assert_eq!(view["question"]["key"], key);
    assert_eq!(result["revision"], view["question"]["revision"]);
    assert_eq!(result["next_offset"], Value::Null);
    assert_eq!(result["revision"], 3);
    assert!(
        !result["content_json"]
            .as_str()
            .unwrap()
            .contains("unrelated historical")
    );
    assert!(view.get("pending").is_some());
    let missing = send(
        &fx,
        &host,
        "read_show",
        "missing",
        json!({"task":id,"section":"questions","key":"T-999/R1/Q1"}),
    );
    assert_eq!(ts::code(&missing), Some("bad_request"));
    let invalid = send(
        &fx,
        &host,
        "read_show",
        "invalid-key",
        json!({"task":id,"key":key}),
    );
    assert_eq!(ts::code(&invalid), Some("bad_request"));
    let automation = send(
        &fx,
        &host,
        "read_show",
        "automation",
        json!({"task":id,"section":"automation","limit":4096}),
    );
    let content: Value = serde_json::from_str(
        automation.receipt().unwrap().result["content_json"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(content["run"], "R1");
    assert_eq!(content["autopilot"]["stage"], "blocked");
    assert_eq!(
        content["autopilot"]["reason"],
        "landing refused: dirty destination"
    );
    assert!(
        automation.receipt().unwrap().result["diagnostic"]
            .as_str()
            .unwrap()
            .contains("dirty destination")
    );
    assert_eq!(content["operations"][0]["key"], "pending-exact-operation");
    assert_eq!(content["operations"][0]["landed"], false);
    assert_eq!(content["operations"][0]["ready"], false);
    assert_eq!(
        operations_before,
        serde_json::to_value(crate::post_land_operations(&fx.ctx, Some("corral")).unwrap())
            .unwrap()
    );
    assert_eq!(before, ts::task_bytes(&fx, &id));
}

#[test]
fn show_pages_reassemble_escaped_unicode_within_wire_bound() {
    let (fx, host) = fixture();
    let task = task(&fx, "corral", &"\"\\\n😀".repeat(3000));
    let before = ts::task_bytes(&fx, &task.id);
    let mut offset = 0;
    let mut content = String::new();
    loop {
        let response = send(
            &fx,
            &host,
            "read_show",
            &format!("page-{offset}"),
            json!({"task":task.id,"section":"spec","offset":offset,"limit":4096}),
        );
        let result = &response.receipt().unwrap().result;
        assert!(serde_json::to_vec(result).unwrap().len() <= RESULT_MAX_BYTES);
        assert_eq!(result["offset"], offset);
        content.push_str(result["content_json"].as_str().unwrap());
        match result["next_offset"].as_u64() {
            Some(next) => {
                assert!(next > offset);
                offset = next;
                assert_eq!(result["truncated"], true);
            }
            None => break,
        }
    }
    assert_eq!(
        serde_json::from_str::<String>(&content).unwrap(),
        fx.ctx.store.get(&task.id).unwrap().body
    );
    let default = send(
        &fx,
        &host,
        "read_show",
        "default",
        json!({"task":task.id,"section":"spec"}),
    );
    assert_eq!(default.receipt().unwrap().result["end_offset"], 1024);
    let bad = send(
        &fx,
        &host,
        "read_show",
        "too-large",
        json!({"task":task.id,"section":"spec","limit":4097}),
    );
    assert_eq!(ts::code(&bad), Some("bad_request"));
    assert_eq!(before, ts::task_bytes(&fx, &task.id));
}

#[test]
fn typed_reads_are_scoped_bounded_and_do_not_write_receipts() {
    // Given tasks in two projects, and a bound PM with one hello receipt.
    let (fx, host) = fixture();
    let outside = task(&fx, "other", "spec");
    let task = task(&fx, "corral", &"x".repeat(20_000));
    fx.ctx
        .store
        .update(&task.id, |saved| {
            saved.difficulty = Some(corral_core::TaskDifficulty::Hard);
            Ok(())
        })
        .unwrap();
    let before = pm_state::load(&fx.ctx.store).unwrap().unwrap();
    let bytes = ts::task_bytes(&fx, &task.id);
    // When the PM lists and inspects its records.
    let listed = send(&fx, &host, "read_list", "list", json!({"limit":1}));
    let shown = send(&fx, &host, "read_show", "show", json!({"task":task.id}));
    let spec = send(
        &fx,
        &host,
        "read_show",
        "spec",
        json!({"task":task.id,"section":"spec","limit":128}),
    );
    let foreign = send(
        &fx,
        &host,
        "read_show",
        "foreign-show",
        json!({"task":outside.id}),
    );
    // Then reads are bounded, foreign records are refused, and canonical state is unchanged.
    assert_eq!(listed.receipt().unwrap().result["total"], 1);
    assert_eq!(
        listed.receipt().unwrap().result["items"][0]["difficulty"],
        "hard"
    );
    assert_eq!(shown.receipt().unwrap().result["task"]["id"], task.id);
    assert_eq!(
        shown.receipt().unwrap().result["task"]["difficulty"],
        "hard"
    );
    assert_eq!(spec.receipt().unwrap().result["next_offset"], 128);
    assert!(
        spec.receipt().unwrap().result["total_chars"]
            .as_u64()
            .unwrap()
            > 20_000
    );
    assert_eq!(ts::code(&foreign), Some("out_of_scope"));
    assert_eq!(before, pm_state::load(&fx.ctx.store).unwrap().unwrap());
    assert_eq!(bytes, ts::task_bytes(&fx, &task.id));
}

#[cfg(unix)]
#[path = "pm_served_tests.rs"]
mod served_reads;
