use corral_core::agent::HostHealth;
use corral_core::work::{self, Counter, HotScope, Work};
use jiff::SignedDuration;

use super::*;
use crate::agent::test_support as ts;
use crate::agent::{BindTarget, tool_request_id};

fn host(fx: &ts::Fx) -> ts::Host {
    let host = ts::bind(
        fx,
        &BindTarget::Pm {
            project: "corral".into(),
        },
        "pm-pane",
        "typed-waits",
    );
    ts::committed(&ts::hello(fx, &host));
    host
}

fn install_wait(fx: &ts::Fx, when: &[String], deadline: Timestamp) -> RunProgress {
    let mut progress: RunProgress = serde_json::from_value(json!({
        "revision": 3, "spec_revision": "pm:corral", "plan": "P1",
        "phases": [{"id": "work", "label": "Work", "steps": [
            {"id": "gate", "label": "Check gate", "status": {"state": "pending"}}
        ]}],
        "waits": [{"step": "gate", "on": {"kind": "event", "event": "task finishes"},
            "deadline": deadline}]
    }))
    .unwrap();
    progress.waits[0].checks = when
        .iter()
        .map(|value| corral_core::parse_park_check(value).unwrap())
        .collect();
    corral_core::agent::request::with_wait_publication_lock(&fx.ctx.store, || {
        pm_state::update(&fx.ctx.store, |pm| {
            save_deadlines(&fx.ctx, &ContainerRef::Pm, None, &progress)?;
            let pm = pm.as_mut().unwrap();
            pm.state.todo = Some(progress.clone());
            pm.state.binding.health = HostHealth::Lost;
            Ok(((), true))
        })
    })
    .unwrap();
    progress
}

#[test]
fn typed_wait_posts_once_after_status_is_met_without_mutating_the_ledger() {
    let fx = ts::fx();
    host(&fx);
    let task = fx
        .ctx
        .store
        .create("Dependency", None, Status::Ready, "")
        .unwrap();
    let deadline = corral_core::clock::now()
        .checked_add(SignedDuration::from_hours(1))
        .unwrap();
    let progress = install_wait(&fx, &[format!("status:{}=done", task.id)], deadline);
    assert_eq!(sweep(&fx.ctx).unwrap(), 0);
    fx.ctx
        .store
        .update(&task.id, |task| {
            task.status = Status::Done;
            Ok(())
        })
        .unwrap();
    assert_eq!(sweep(&fx.ctx).unwrap(), 1);
    let inbox = Inbox::new(&fx.ctx.store, Recipient::Pm).unwrap();
    let messages = inbox.messages().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].kind, "pm_wait_wake");
    assert_eq!(
        messages[0].subject.as_ref().unwrap()["key"],
        "wait:P1:gate:3:met"
    );
    assert_eq!(messages[0].subject.as_ref().unwrap()["condition_met"], true);

    let hot = HotScope::enter();
    let before = work::snapshot();
    assert_eq!(sweep(&fx.ctx).unwrap(), 0);
    let work = work::snapshot().since(&before);
    drop(hot);
    assert_eq!(work.total(Counter::TaskHeadDecodes), 0);
    assert_eq!(work.total(Counter::FullTaskHydrations), 0);
    assert_eq!(inbox.messages().unwrap().len(), 1);
    let mut stored = pm_state::load_current(&fx.ctx.store)
        .unwrap()
        .unwrap()
        .state
        .todo
        .unwrap();
    hydrate(&fx.ctx, &ContainerRef::Pm, &mut stored).unwrap();
    assert_eq!(stored, progress);
}

#[test]
fn unmet_typed_wait_still_publishes_its_deadline_and_can_later_publish_met() {
    let fx = ts::fx();
    host(&fx);
    let task = fx
        .ctx
        .store
        .create("Dependency", None, Status::Ready, "")
        .unwrap();
    let deadline = corral_core::clock::now()
        .checked_sub(SignedDuration::from_hours(1))
        .unwrap();
    install_wait(&fx, &[format!("status:{}=done", task.id)], deadline);
    assert_eq!(sweep(&fx.ctx).unwrap(), 1);
    assert_eq!(sweep(&fx.ctx).unwrap(), 0);
    let inbox = Inbox::new(&fx.ctx.store, Recipient::Pm).unwrap();
    let messages = inbox.messages().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].subject.as_ref().unwrap()["key"],
        "wait:P1:gate:3"
    );
    fx.ctx
        .store
        .update(&task.id, |task| {
            task.status = Status::Done;
            Ok(())
        })
        .unwrap();
    assert_eq!(sweep(&fx.ctx).unwrap(), 1);
    assert_eq!(sweep(&fx.ctx).unwrap(), 0);
    let messages = inbox.messages().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(
        messages[1].subject.as_ref().unwrap()["key"],
        "wait:P1:gate:3:met"
    );
}

#[test]
fn future_untyped_wait_sweep_has_no_writes_inbox_work_or_delivery() {
    let fx = ts::fx();
    host(&fx);
    let deadline = corral_core::clock::now()
        .checked_add(SignedDuration::from_hours(1))
        .unwrap();
    install_wait(&fx, &[], deadline);
    let seq = fx.ctx.store.view().unwrap().seq();
    let inbox_reads = work::count(Work::InboxRecordRead);
    let inbox_writes = work::count(Work::InboxHeaderWrite);
    let hot = HotScope::enter();
    let before = work::snapshot();
    assert_eq!(sweep(&fx.ctx).unwrap(), 0);
    let work = work::snapshot().since(&before);
    drop(hot);
    assert_eq!(fx.ctx.store.view().unwrap().seq(), seq);
    assert_eq!(work::count(Work::InboxRecordRead), inbox_reads);
    assert_eq!(work::count(Work::InboxHeaderWrite), inbox_writes);
    assert_eq!(work.total(Counter::Processes), 0);
}

#[test]
fn malformed_typed_wait_is_a_bad_request_and_does_not_replace_the_plan() {
    let fx = ts::fx();
    let host = host(&fx);
    let init = json!({"op": "init", "phases": [{"id": "work", "label": "Work", "steps": [
        {"id": "gate", "label": "Check gate"}
    ]}]});
    let send = |id: &str, payload| {
        ts::send(
            &fx,
            &ts::envelope(
                &host,
                "todo",
                &tool_request_id(&host.identity.session_id, id),
                payload,
                false,
            ),
            "todo",
        )
    };
    ts::committed(&send("init", init));
    let before = pm_state::load_current(&fx.ctx.store)
        .unwrap()
        .unwrap()
        .state
        .todo;
    let response = send(
        "invalid",
        json!({
            "op": "wait", "step": "gate", "on": {"kind": "event", "event": "task finishes"},
            "when": ["status:T-001=not-a-status"]
        }),
    );
    assert_eq!(response.rejection().unwrap().code.as_str(), "bad_request");
    assert_eq!(
        pm_state::load_current(&fx.ctx.store)
            .unwrap()
            .unwrap()
            .state
            .todo,
        before
    );
}
