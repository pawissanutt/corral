//! Typed PM actions: revalidated execution, replay, PM parks and the PM's project scope.
#![cfg(unix)] // The fixture executes the POSIX fake terminal engine.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;

use corral_core::agent::ReceiptState;
use corral_core::{
    AgentCmd, OperatingAuthority, OperatingSelection, Park, Responsible, Status, Store,
};
use corral_herdr::Herdr;
use serde_json::{Value, json};

use crate::Ctx;
use crate::agent::test_support::{self as ts, Fx, Host, committed, hello};
use crate::agent::{BindTarget, CallResponse, tool_request_id};

fn git(repo: &Path, args: &[&str]) {
    let result = crate::test_support::command("git", repo)
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

/// Projects alpha and beta with repositories, a fake profile, the fake terminal engine and a
/// PM bound to alpha.
fn fixture() -> (Fx, Host) {
    let tmp = tempfile::tempdir().unwrap();
    for project in ["alpha", "beta"] {
        let repo = tmp.path().join(project);
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        git(&repo, &["config", "user.name", "Corral Test"]);
        git(&repo, &["config", "user.email", "test@example.invalid"]);
        git(
            &repo,
            &[
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-qm",
                "init",
            ],
        );
    }
    let mut store = Store::init_with(tmp.path(), |store| {
        std::fs::write(
            store.config_path(),
            "[projects.alpha]\npath = \"alpha\"\n[projects.beta]\npath = \"beta\"\n[pm]\nagent = \"fake\"\n",
        )?;
        Ok(())
    })
    .unwrap();
    store.config.agents.insert(
        "fake".into(),
        AgentCmd {
            command: "true".into(),
        },
    );
    for project in ["alpha", "beta"] {
        store
            .config
            .projects
            .get_mut(project)
            .unwrap()
            .default_profile = Some("fake".into());
    }
    store.config.routing.rules.clear();
    store.config.limits.running = 8;
    store.save_config().unwrap();
    let engine = tmp.path().join("engine");
    std::fs::create_dir(&engine).unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/operating_launch");
    std::fs::copy(source.join("herdr.py"), engine.join("herdr.py")).unwrap();
    let bin = engine.join("herdr");
    std::fs::copy(source.join("herdr.sh"), &bin).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    let fx = Fx {
        _tmp: tmp,
        ctx: Ctx {
            store,
            herdr: Herdr::with_bin(bin, Some("corral-test".into())),
            resource_probe: crate::resources::FixedResourceProbe::arc(),
        },
    };
    let pm = BindTarget::Pm {
        project: "alpha".into(),
    };
    let host = ts::bind(&fx, &pm, "p-pm", "pm");
    committed(&hello(&fx, &host));
    (fx, host)
}

fn fresh(ctx: &Ctx) -> Ctx {
    Ctx {
        store: Store::open(ctx.store.root()).unwrap(),
        ..ctx.clone()
    }
}

fn create(fx: &Fx, title: &str, project: &str) -> String {
    fx.ctx
        .store
        .create(
            title,
            Some(project),
            Status::Ready,
            "## Acceptance\nWorks.\n",
        )
        .unwrap()
        .id
}

/// The owner's finite grant over `tasks`, with or without dispatch authority.
fn grant(fx: &mut Fx, tasks: &[&str], dispatch: bool) {
    let preview = fx
        .ctx
        .store
        .preview_operating_policy(&OperatingSelection {
            tasks: tasks.iter().map(|id| (*id).to_owned()).collect(),
            authority: OperatingAuthority {
                dispatch,
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    fx.ctx.store.config = fx
        .ctx
        .store
        .confirm_operating_policy(&preview, "fixture owner", jiff::Timestamp::now())
        .unwrap();
}

fn action_for(fx: &Fx, task: &str) -> String {
    crate::pm_workset(&fresh(&fx.ctx))
        .unwrap()
        .actions
        .into_iter()
        .find(|action| action.action.task.as_deref() == Some(task))
        .unwrap()
        .id
}

fn request(host: &Host, call: &str, payload: Value) -> Vec<u8> {
    let id = tool_request_id(&host.identity.session_id, call);
    ts::envelope(host, "pm_action", &id, payload, false)
}

fn send(fx: &Fx, input: &[u8]) -> CallResponse {
    crate::agent::call(&fresh(&fx.ctx), "pm_action", input).unwrap()
}

/// The refusal code, whether refused before the reservation or settled as a stored rejection.
fn refused(response: &CallResponse) -> String {
    match response {
        CallResponse::Rejected { rejection, .. } => rejection.code.as_str().to_owned(),
        CallResponse::Receipt { receipt, .. } if receipt.state == ReceiptState::Rejected => {
            receipt.result["code"].as_str().unwrap().to_owned()
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn untouched(fx: &Fx, id: &str) {
    let task = fx.ctx.store.get(id).unwrap();
    assert_eq!(task.status, Status::Ready, "{id}");
    assert!(task.runs.is_empty() && task.park.is_none(), "{id}");
}

#[test]
fn act_dispatches_its_task_once_and_a_replay_executes_nothing() {
    // Given an executable dispatch action for a granted alpha task.
    let (mut fx, host) = fixture();
    let alpha = create(&fx, "Alpha work", "alpha");
    grant(&mut fx, &[&alpha], true);
    let input = request(
        &host,
        "act",
        json!({"action": "act", "id": action_for(&fx, &alpha)}),
    );
    // When the PM acts on it.
    let first = committed(&send(&fx, &input));
    // Then the task is dispatched through the scoped executor.
    assert_eq!(first.result["outcome"], "progressed");
    assert_eq!(first.result["task"], alpha.as_str());
    assert_eq!(first.result["status"], Status::Running.as_str());
    assert_eq!(fx.ctx.store.get(&alpha).unwrap().status, Status::Running);
    // And the same request replays the stored receipt without another effect.
    let before = (
        ts::task_bytes(&fx, &alpha),
        fx.ctx.store.events().unwrap().len(),
    );
    let again = send(&fx, &input);
    assert!(
        matches!(&again, CallResponse::Receipt { receipt, replayed: true } if *receipt == first)
    );
    let after = (
        ts::task_bytes(&fx, &alpha),
        fx.ctx.store.events().unwrap().len(),
    );
    assert_eq!(after, before);
}

#[test]
fn act_after_the_task_identity_changed_is_refused_without_effect() {
    // Given an action ID read before the task changed.
    let (mut fx, host) = fixture();
    let alpha = create(&fx, "Alpha work", "alpha");
    grant(&mut fx, &[&alpha], true);
    let id = action_for(&fx, &alpha);
    fx.ctx
        .store
        .update(&alpha, |task| {
            task.title = "Alpha work, revised".into();
            Ok(())
        })
        .unwrap();
    // When the PM acts on the old ID.
    let response = send(
        &fx,
        &request(&host, "act", json!({"action": "act", "id": id})),
    );
    // Then it is a stored stale refusal and nothing was dispatched.
    assert_eq!(refused(&response), "stale_revision");
    untouched(&fx, &alpha);
}

#[test]
fn act_on_an_option_that_now_needs_the_owner_is_refused_without_effect() {
    // Given an action ID read before the owner withdrew dispatch authority.
    let (mut fx, host) = fixture();
    let alpha = create(&fx, "Alpha work", "alpha");
    grant(&mut fx, &[&alpha], true);
    let id = action_for(&fx, &alpha);
    grant(&mut fx, &[&alpha], false);
    let workset = crate::pm_workset(&fresh(&fx.ctx)).unwrap();
    assert!(
        !workset
            .actions
            .iter()
            .any(|action| action.action.task.as_deref() == Some(alpha.as_str()))
    );
    // When the PM acts on it.
    let response = send(
        &fx,
        &request(&host, "act", json!({"action": "act", "id": id})),
    );
    // Then the owner-required dispatch is refused and nothing ran.
    assert_eq!(refused(&response), "stale_revision");
    untouched(&fx, &alpha);
}

#[test]
fn park_records_a_pm_park_with_checks_and_never_replaces_an_owner_park() {
    let (fx, host) = fixture();
    let waiting = create(&fx, "Waiting", "alpha");
    let owned = create(&fx, "Owner parked", "alpha");
    // Given an unsupported resume check, then the park is refused before any effect.
    let manual = json!({"action": "park", "task": waiting, "reason": "Wait", "resume_when": ["manual:later"]});
    assert_eq!(
        refused(&send(&fx, &request(&host, "park-manual", manual))),
        "bad_request"
    );
    assert!(fx.ctx.store.get(&waiting).unwrap().park.is_none());
    // When the PM parks with a machine-checkable condition.
    let payload = json!({
        "action": "park", "task": waiting, "reason": "Wait for the owner's task",
        "resume_when": [format!("status:{owned}=done")], "review_by": "2h",
    });
    let parked = committed(&send(&fx, &request(&host, "park", payload)));
    // Then a PM park with the parsed check is recorded.
    let park = fx.ctx.store.get(&waiting).unwrap().park.unwrap();
    assert_eq!(park.by, Responsible::Pm);
    assert_eq!(park.resume_when[0].kind, "status");
    assert_eq!(park.resume_when[0].target.as_deref(), Some(owned.as_str()));
    assert_eq!(parked.result["revision"], park.revision);
    // Given an owner park, when the PM parks the same task, then the owner park stays.
    let now = jiff::Timestamp::now();
    let owner = Park {
        version: 1,
        revision: 1,
        by: Responsible::Owner,
        reason: "Owner hold".into(),
        at: now,
        resume_when: Vec::new(),
        resume_action: String::new(),
        review_by: now
            .checked_add(jiff::SignedDuration::from_hours(1))
            .unwrap(),
        ready_at: None,
    };
    let stored = owner.clone();
    fx.ctx
        .store
        .update(&owned, |task| {
            task.park = Some(stored);
            Ok(())
        })
        .unwrap();
    let replace = json!({"action": "park", "task": owned, "reason": "Mine now"});
    assert_eq!(
        refused(&send(&fx, &request(&host, "park-owner", replace))),
        "op_not_allowed"
    );
    assert_eq!(fx.ctx.store.get(&owned).unwrap().park, Some(owner));
}

#[test]
fn every_action_refuses_a_task_outside_the_pm_scope() {
    // Given a beta task the owner granted, while the PM is bound to alpha.
    let (mut fx, host) = fixture();
    let beta = create(&fx, "Beta work", "beta");
    grant(&mut fx, &[&beta], true);
    let cases = [
        (
            "act",
            json!({"action": "act", "id": action_for(&fx, &beta)}),
        ),
        (
            "park",
            json!({"action": "park", "task": beta, "reason": "Not mine"}),
        ),
        (
            "attend",
            json!({"action": "attend", "key": format!("{beta}/agent_lost"), "note": "Not mine"}),
        ),
    ];
    for (call, payload) in cases {
        // When the PM targets it with each action.
        let response = send(&fx, &request(&host, call, payload));
        // Then each is refused as out of scope and nothing changes.
        assert_eq!(refused(&response), "out_of_scope", "{call}");
    }
    untouched(&fx, &beta);
    let task = fx.ctx.store.get(&beta).unwrap();
    assert!(
        task.attention
            .records
            .values()
            .all(|record| record.handling.is_none())
    );
}

#[test]
fn unknown_actions_and_fields_are_bad_requests() {
    let fx = ts::fx();
    let pm = BindTarget::Pm {
        project: "alpha".into(),
    };
    let host = ts::bind(&fx, &pm, "p-pm", "pm");
    committed(&hello(&fx, &host));
    for (call, payload) in [
        ("shell", json!({"action": "run", "command": "true"})),
        ("owner", json!({"action": "act", "id": "x", "owner": true})),
    ] {
        assert_eq!(
            refused(&send(&fx, &request(&host, call, payload))),
            "bad_request",
            "{call}"
        );
    }
}
