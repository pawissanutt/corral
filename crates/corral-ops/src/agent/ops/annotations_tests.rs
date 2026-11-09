use super::*;
use crate::agent::{BindTarget, test_support as ts};
use corral_core::agent::ContainerRef;
use std::{path::Path, process::Command};

fn git(path: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@test.invalid")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@test.invalid")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}
fn setup(role: AgentRole) -> (ts::Fx, ts::Host, BindTarget, String, Value) {
    let mut fx = ts::fx();
    fx.ctx.store.config.projects.insert(
        "corral".into(),
        serde_json::from_value(json!({"path":fx.ctx.store.root()})).unwrap(),
    );
    fx.ctx.store.save_config().unwrap();
    let (task, reviewer) = if role == AgentRole::Reviewer {
        let (task, target) = ts::reviewer_task(&fx);
        (task, Some(target))
    } else {
        (ts::worker_task(&fx, "p1"), None)
    };
    let root = fx.ctx.store.root();
    git(root, &["init", "-b", "main"]);
    std::fs::write(root.join("code"), "before\n").unwrap();
    git(root, &["add", "code"]);
    git(root, &["commit", "-m", "base"]);
    let base = git(root, &["rev-parse", "HEAD"]);
    git(root, &["branch", "base"]);
    std::fs::write(root.join("code"), "after\n").unwrap();
    git(root, &["add", "code"]);
    git(root, &["commit", "-m", "candidate"]);
    let head = git(root, &["rev-parse", "HEAD"]);
    fx.ctx
        .store
        .update(&task, |t| {
            t.runs[0].worktree = root.to_path_buf();
            t.runs[0].base = "base".into();
            t.runs[0].base_sha = base.clone();
            Ok(())
        })
        .unwrap();
    let target = match role {
        AgentRole::Reviewer => reviewer.unwrap(),
        AgentRole::Pm => BindTarget::Pm {
            project: "corral".into(),
        },
        AgentRole::Assistant => {
            let s = fx
                .ctx
                .store
                .create_assistant("pi", None, Some("p1"), None)
                .unwrap();
            BindTarget::Assistant { session: s.id }
        }
        _ => BindTarget::Worker {
            task: task.clone(),
            run: "R1".into(),
        },
    };
    let pane = if role == AgentRole::Reviewer {
        "p2"
    } else {
        "p1"
    };
    let host = ts::bind(&fx, &target, pane, "annotation-session");
    ts::committed(&ts::hello(&fx, &host));
    let action = json!({"action":"open", "expected_run":"R1", "base":base,"head":head,
        "path":"code","side":"new","range":{"start":1,"end":1},"text":"Owner note"});
    (fx, host, target, task, action)
}
fn event(
    fx: &ts::Fx,
    host: &ts::Host,
    task: &str,
    seq: u64,
    key: &str,
    action: Value,
) -> CallResponse {
    let id = crate::agent::event_request_id(host.identity.generation, seq);
    ts::send(
        fx,
        &ts::envelope(
            host,
            "event",
            &id,
            json!({"kind":"owner_annotation","task":task,"request_key":key,"action":action}),
            false,
        ),
        "event",
    )
}
fn read(
    fx: &ts::Fx,
    host: &ts::Host,
    task: &str,
    thread: Option<&str>,
    cursor: Option<&str>,
) -> CallResponse {
    ts::send(
        fx,
        &ts::envelope(
            host,
            "read_annotations",
            "rq-00000000000000000000000000000001",
            json!({"task":task,"thread":thread,"query":{"limit":1,"cursor":cursor}}),
            false,
        ),
        "read_annotations",
    )
}
fn result(response: CallResponse) -> Value {
    let value = response.to_json().unwrap();
    assert_eq!(value["state"], "committed", "{value}");
    assert_eq!(value["stored"], false);
    value["result"].clone()
}
fn snapshot(fx: &ts::Fx, task: &str) -> Value {
    serde_json::to_value(fx.ctx.store.get(task).unwrap()).unwrap()
}

#[test]
fn annotations_owner_journey_roles_replay_and_receipt_free_pages() {
    let mut views = Vec::new();
    for role in ROLES {
        let (fx, host, target, task, action) = setup(*role);
        let opened = result(event(&fx, &host, &task, 1, "one", action.clone()));
        assert_eq!(opened["revision"], 1);
        for (i, action) in ["reply", "resolve", "reopen"].iter().enumerate() {
            let r = result(event(
                &fx,
                &host,
                &task,
                i as u64 + 2,
                action,
                json!({"action":action,
                "thread":"N1","expected_revision":i + 1,"text":"owner continuation"}),
            ));
            assert_eq!(r["revision"], i + 2);
        }
        // A freshly authorized new generation replays the original entry, not the current revision.
        let next = ts::bind(&fx, &target, &host.pane, "next-session");
        ts::committed(&ts::hello(&fx, &next));
        let before = snapshot(&fx, &task);
        let events = fx.ctx.store.events().unwrap().len();
        let replay = result(event(&fx, &next, &task, 1, "one", action));
        assert_eq!(replay["replayed"], true);
        assert_eq!(replay["revision"], 1);
        let list = result(read(&fx, &next, &task, None, None));
        assert_eq!(list["version"], 1);
        assert_eq!(list["items"][0]["anchor_status"]["status"], "current");
        let mut cursor = None;
        let mut seqs = Vec::new();
        loop {
            let page = result(read(&fx, &next, &task, Some("N1"), cursor.as_deref()));
            assert!(serde_json::to_vec(&page).unwrap().len() < RESPONSE_BYTES);
            for e in page["entries"].as_array().unwrap() {
                seqs.push(e["seq"].as_u64().unwrap());
                assert_eq!(e["author"]["kind"], "owner");
                assert_eq!(
                    e["author"]["surface"],
                    json!({"kind":"pi_panel","container":target.container(),
                    "session":host.identity.session_id,"generation":1})
                );
            }
            cursor = page["next_cursor"].as_str().map(str::to_owned);
            views.push(page);
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(seqs, vec![1, 2, 3, 4]);
        views.push(list);
        assert_eq!(snapshot(&fx, &task), before);
        assert_eq!(fx.ctx.store.events().unwrap().len(), events);
        assert_eq!(ts::state(&fx, &target.container()).receipts.len(), 2); // hello per generation only
    }
    if let Ok(dir) = std::env::var("CORRAL_ANNOTATION_HOST_VIEWS") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            Path::new(&dir).join("host-views.json"),
            serde_json::to_vec_pretty(&views).unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn annotations_refuse_scope_roles_raw_model_events_unknown_author_and_fenced_replay() {
    let (fx, host, target, task, action) = setup(AgentRole::Worker);
    result(event(&fx, &host, &task, 1, "one", action.clone()));
    let other = ts::worker_task(&fx, "other");
    let before = snapshot(&fx, &task);
    let event_count = fx.ctx.store.events().unwrap().len();
    assert_eq!(
        ts::code(&read(&fx, &host, &other, None, None)),
        Some("out_of_scope")
    );
    assert_eq!(
        ts::code(&event(&fx, &host, &other, 2, "cross", action.clone())),
        Some("out_of_scope")
    );
    let payload =
        json!({"kind":"owner_annotation","task":task,"request_key":"raw","action":action});
    let raw = ts::send(
        &fx,
        &ts::envelope(
            &host,
            "event",
            "rq-00000000000000000000000000000002",
            payload.clone(),
            false,
        ),
        "event",
    );
    assert_eq!(ts::code(&raw), Some("bad_request"));
    let mut nested = host.clone();
    nested.identity.parent = Some(corral_core::agent::CallParent {
        tool_call_id: "model-cell".into(),
        cell: "cell-1".into(),
        call: "cell-1.1".into(),
    });
    let nested = ts::send(
        &fx,
        &ts::envelope(
            &nested,
            "event",
            "rq-00000000000000000000000000000003",
            payload.clone(),
            false,
        ),
        "event",
    );
    assert_eq!(ts::code(&nested), Some("bad_request"));
    let mut forged = payload;
    forged["author"] = json!("owner");
    assert_eq!(
        ts::code(&ts::send(
            &fx,
            &ts::envelope(&host, "event", "ev-1-2", forged, false),
            "event"
        )),
        Some("bad_request")
    );
    for role in [AgentRole::Planner, AgentRole::PlanReviewer] {
        let mut planner = host.clone();
        planner.identity.role = role;
        planner.identity.run = Some(
            if role == AgentRole::Planner {
                "D1"
            } else {
                "P1"
            }
            .into(),
        );
        planner.identity.review = (role == AgentRole::PlanReviewer).then(|| "PR1".into());
        for response in [
            read(&fx, &planner, &task, None, None),
            event(&fx, &planner, &task, 3, "denied", action.clone()),
        ] {
            assert_eq!(ts::code(&response), Some("annotation_refused"));
            assert_eq!(
                response.to_json().unwrap()["result"]["detail"]["code"],
                "role_not_permitted"
            );
        }
    }
    assert_eq!(snapshot(&fx, &task), before);
    assert_eq!(fx.ctx.store.events().unwrap().len(), event_count);
    crate::agent::fence_generation(&fx.ctx.store, &target.container(), 1, "test").unwrap();
    let fenced = snapshot(&fx, &task);
    assert_eq!(
        ts::code(&event(&fx, &host, &task, 4, "one", action)),
        Some("agent_fenced")
    );
    assert_eq!(snapshot(&fx, &task), fenced);
    assert_eq!(ts::events(&fx, "annotation_opened"), 1);
}

#[test]
fn annotations_project_scope_unavailable_and_strict_action_errors() {
    let (fx, host, _, task, mut action) = setup(AgentRole::Pm);
    let other = fx
        .ctx
        .store
        .create("foreign", Some("other"), corral_core::Status::Backlog, "")
        .unwrap();
    assert_eq!(
        ts::code(&read(&fx, &host, &other.id, None, None)),
        Some("out_of_scope")
    );
    assert_eq!(
        ts::code(&event(&fx, &host, &other.id, 1, "foreign", action.clone())),
        Some("out_of_scope")
    );
    action["expected_run"] = json!("R9");
    let stale = event(&fx, &host, &task, 2, "stale", action.clone());
    assert_eq!(
        stale.to_json().unwrap()["result"]["detail"]["code"],
        "stale_candidate"
    );
    action["expected_run"] = json!("R1");
    action["author"] = json!("owner");
    assert_eq!(
        ts::code(&event(&fx, &host, &task, 3, "forged", action)),
        Some("bad_request")
    );
    fx.ctx
        .store
        .update(&task, |t| {
            t.runs[0].worktree = fx.ctx.store.root().join("removed");
            Ok(())
        })
        .unwrap();
    let view = result(read(&fx, &host, &task, None, None));
    assert_eq!(view["candidate"]["status"], "unavailable");
    if let Ok(dir) = std::env::var("CORRAL_ANNOTATION_HOST_VIEWS") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            Path::new(&dir).join("unavailable.json"),
            serde_json::to_vec_pretty(&view).unwrap(),
        )
        .unwrap();
    }
    assert!(matches!(
        admission::container_of(&host.identity),
        ContainerRef::Pm
    ));
}
