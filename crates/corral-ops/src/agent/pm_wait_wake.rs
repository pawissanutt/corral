//! Deadline wakes for a session ledger's waiting steps: when a wait's deadline passes, sync
//! publishes one keyed wake so an all-waiting plan cannot stay silent.
use std::collections::HashSet;

use anyhow::{Context, Result};
use corral_core::WaitOn;
use corral_core::agent::pm_state;
use corral_core::inbox::{Inbox, MessageBinding, Post, Recipient, Urgency};
use serde_json::json;

use crate::Ctx;

/// Publish each expired PM wait once per plan revision, then deliver newly posted wakes.
/// The ledger remains unchanged: the PM must clear or renew the wait after reviewing it.
pub fn sweep(ctx: &Ctx) -> Result<usize> {
    let Some(pm) = pm_state::load_current(&ctx.store)?.filter(super::pm_wake::healthy) else {
        return Ok(0);
    };
    let Some(progress) = &pm.state.todo else {
        return Ok(0);
    };
    if progress.waits.is_empty() {
        return Ok(0);
    }
    let now = corral_core::clock::now();
    let mut due = progress
        .waits
        .iter()
        .filter(|wait| wait.deadline.is_none_or(|deadline| deadline <= now))
        .peekable();
    if due.peek().is_none() {
        return Ok(0);
    }
    let Some(_outbox) = ctx.store.try_lock("pm-wait-wake-outbox")? else {
        return Ok(0);
    };
    let inbox = Inbox::new(&ctx.store, Recipient::Pm)?;
    let mut posted = 0;
    // RunProgress has no revision timestamp. Legacy waits are therefore due immediately,
    // with this same durable subject-key dedupe, rather than an invented review date.
    let mut published: HashSet<String> = inbox
        .messages()?
        .into_iter()
        .filter(|message| message.kind == "pm_wait_wake")
        .filter_map(|message| message.subject?.get("key")?.as_str().map(str::to_owned))
        .collect();
    let plan = progress.plan.clone().unwrap_or_else(|| {
        format!(
            "P{}",
            pm.state
                .todo_requests
                .as_ref()
                .map_or(1, |requests| requests.plans.max(1))
        )
    });
    for wait in due {
        let key = format!("wait:{plan}:{}:{}", wait.step, progress.revision);
        if published.contains(&key) {
            continue;
        }
        let condition = match &wait.on {
            WaitOn::Event { event } => format!("event {event}"),
            WaitOn::Owner { question } => format!("owner answer to {question}"),
        };
        let deadline = match wait.deadline {
            Some(deadline) => format!("review deadline {deadline} has passed"),
            None => "review deadline has passed (legacy wait without a stored deadline)".into(),
        };
        let body = format!(
            "Plan {plan}, step {} waited on {condition}; its {deadline}.\n\
             Read the current plan and condition, then clear or renew the wait. \
             A wake is a pointer, not authority, and grants no owner authority.",
            wait.step
        );
        inbox
            .post(Post {
                kind: "pm_wait_wake",
                urgency: Urgency::Wake,
                author: "corral",
                body: &body,
                binding: MessageBinding::default(),
                subject: Some(json!({
                    "key": key, "plan": plan, "step": wait.step,
                    "revision": progress.revision, "project": pm.state.binding.project,
                })),
                max_payload: super::delivery::INPUT_MAX_BYTES,
            })
            .with_context(|| format!("publishing PM wait wake {key}"))?;
        published.insert(key);
        posted += 1;
    }
    if posted == 0 {
        return Ok(0);
    }
    // pm_wake::repost already retries the shared inbox before this sweep. Only newly
    // published wakes need another delivery attempt during this sync pass.
    if let Err(error) =
        crate::delivery::deliver(ctx, &Recipient::Pm, &super::pm_wake::ScopedPm(pm), None)
    {
        crate::messages::report(
            None,
            "PM wait wake",
            format!("delivery deferred: {error:#}"),
        );
    }
    Ok(posted)
}

#[cfg(all(test, unix))]
mod tests {
    use corral_core::RunProgress;
    use jiff::SignedDuration;

    use super::*;
    use crate::agent::BindTarget;
    use crate::agent::test_support as ts;

    #[test]
    fn sweep_posts_expired_and_legacy_waits_once_but_not_future_waits() {
        let fx = ts::fx();
        let host = ts::bind(
            &fx,
            &BindTarget::Pm {
                project: "corral".into(),
            },
            "pm-pane",
            "s-pm",
        );
        ts::committed(&ts::hello(&fx, &host));
        let now = corral_core::clock::now();
        let progress: RunProgress = serde_json::from_value(json!({
            "revision": 3, "spec_revision": "pm:corral", "plan": "P1",
            "phases": [{"id": "work", "label": "Work", "steps": [
                {"id": "gate", "label": "Check gate", "status": {"state": "pending"}},
                {"id": "owner", "label": "Ask owner", "status": {"state": "pending"}}
            ]}],
            "waits": [
                {"step": "gate", "on": {"kind": "event", "event": "gate finishes"},
                    "deadline": now.checked_sub(SignedDuration::from_hours(1)).unwrap()},
                {"step": "owner", "on": {"kind": "owner", "question": "Which scope?"},
                    "deadline": now.checked_add(SignedDuration::from_hours(1)).unwrap()}
            ]
        }))
        .unwrap();
        pm_state::update(&fx.ctx.store, |pm| {
            pm.as_mut().unwrap().state.todo = Some(progress.clone());
            Ok(((), true))
        })
        .unwrap();

        assert_eq!(sweep(&fx.ctx).unwrap(), 1);
        assert_eq!(sweep(&fx.ctx).unwrap(), 0);
        let inbox = Inbox::new(&fx.ctx.store, Recipient::Pm).unwrap();
        let messages = inbox.messages().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].kind, "pm_wait_wake");
        assert_eq!(
            messages[0].subject.as_ref().unwrap()["key"],
            "wait:P1:gate:3"
        );
        assert_eq!(
            pm_state::load_current(&fx.ctx.store)
                .unwrap()
                .unwrap()
                .state
                .todo,
            Some(progress)
        );

        // Renewing the plan revision gives a legacy wait its own single review wake.
        pm_state::update(&fx.ctx.store, |pm| {
            let todo = pm.as_mut().unwrap().state.todo.as_mut().unwrap();
            todo.revision += 1;
            todo.waits[0].deadline = None;
            Ok(((), true))
        })
        .unwrap();
        assert_eq!(sweep(&fx.ctx).unwrap(), 1);
        assert_eq!(sweep(&fx.ctx).unwrap(), 0);
        let messages = inbox.messages().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(
            messages[1].subject.as_ref().unwrap()["key"],
            "wait:P1:gate:4"
        );
    }
}
