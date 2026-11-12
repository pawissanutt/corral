//! Review wakes for session-ledger waits: deadline expiry or all typed conditions satisfied.
//! Publication is keyed; neither wake changes the ledger or takes a board effect.
use std::collections::HashSet;
use std::fmt::Write;

use anyhow::{Context, Result, ensure};
use corral_core::agent::{ContainerRef, paths, pm_state, request::WAIT_PUBLICATION_LOCK};
use corral_core::inbox::{Inbox, MessageBinding, Post, Recipient, Urgency};
use corral_core::{ResumeCheck, RunProgress, Status, Store, WaitOn};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::Ctx;

const DEADLINE_AREA: &str = "agent-wait-deadlines";

/// Additive storage, never embedded in the old binary's deny_unknown_fields ledger.
#[derive(Serialize, Deserialize)]
struct WaitDeadline {
    version: u32,
    container: String,
    plan: String,
    step: String,
    revision: u64,
    deadline: Timestamp,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    checks: Vec<ResumeCheck>,
}

fn plan(progress: &RunProgress) -> &str {
    progress.plan.as_deref().unwrap_or("P1")
}

fn deadline_key(container: &ContainerRef, plan: &str, revision: u64, step: &str) -> Result<String> {
    Ok(paths::sha256_hex(
        &serde_json::to_vec(&(container.key(), plan, step, revision))
            .context("encoding session wait deadline key")?,
    ))
}

fn side_record(
    ctx: &Ctx,
    container: &ContainerRef,
    plan: &str,
    revision: u64,
    step: &str,
) -> Result<Option<WaitDeadline>> {
    let key = deadline_key(container, plan, revision, step)?;
    let Some(record) = ctx.store.record::<WaitDeadline>(DEADLINE_AREA, &key)? else {
        return Ok(None);
    };
    ensure!(
        record.version == 1
            && record.container == container.key()
            && record.plan == plan
            && record.step == step
            && record.revision == revision,
        "session wait deadline identity mismatch"
    );
    Ok(Some(record))
}

#[cfg(all(test, unix))]
fn deadline(
    ctx: &Ctx,
    container: &ContainerRef,
    plan: &str,
    revision: u64,
    step: &str,
) -> Result<Option<Timestamp>> {
    Ok(side_record(ctx, container, plan, revision, step)?.map(|record| record.deadline))
}

/// Restore ephemeral side-record fields before applying another session-ledger command.
pub(crate) fn hydrate(
    ctx: &Ctx,
    container: &ContainerRef,
    progress: &mut RunProgress,
) -> Result<()> {
    let plan = plan(progress).to_owned();
    for wait in &mut progress.waits {
        if let Some(record) = side_record(ctx, container, &plan, progress.revision, &wait.step)? {
            wait.deadline = Some(record.deadline);
            wait.checks = record.checks;
        }
    }
    Ok(())
}

fn publication_key(progress: &RunProgress, step: &str) -> String {
    format!("wait:{}:{step}:{}", plan(progress), progress.revision)
}

/// Called inside the ledger/receipt transaction while holding WAIT_PUBLICATION_LOCK.
/// Carry retained waits to the new revision and remove replaced or cleared side records.
pub(crate) fn save_deadlines(
    ctx: &Ctx,
    container: &ContainerRef,
    previous: Option<&RunProgress>,
    progress: &RunProgress,
) -> Result<()> {
    if let Some(previous) = previous {
        for wait in &previous.waits {
            ctx.store.delete_record(
                DEADLINE_AREA,
                &deadline_key(container, plan(previous), previous.revision, &wait.step)?,
            )?;
        }
    }
    for wait in &progress.waits {
        if let Some(deadline) = wait.deadline {
            ctx.store.save_record(
                DEADLINE_AREA,
                &deadline_key(container, plan(progress), progress.revision, &wait.step)?,
                &WaitDeadline {
                    version: 1,
                    container: container.key(),
                    plan: plan(progress).into(),
                    step: wait.step.clone(),
                    revision: progress.revision,
                    deadline,
                    checks: wait.checks.clone(),
                },
            )?;
        }
    }
    if matches!(container, ContainerRef::Pm) {
        let keep: HashSet<String> = progress
            .waits
            .iter()
            .flat_map(|wait| {
                let key = publication_key(progress, &wait.step);
                [format!("{key}:met"), key]
            })
            .collect();
        super::pm_wake_keys::retain(ctx, "wait", &|key| keep.contains(key))?;
    }
    Ok(())
}

/// Park's neighborhood evaluator needs a task park and an already-populated neighborhood.
/// Resolve the same predicates directly on keyed heads instead; never discover a workspace.
fn check_met(store: &Store, check: &ResumeCheck) -> Result<bool> {
    use corral_core::backend::TaskPart;

    let Some(target) = check.target.as_deref() else {
        return Ok(false);
    };
    match check.kind.as_str() {
        "answered" | "decision" => {
            let (part, token) = if check.kind == "answered" {
                (TaskPart::AgentRecord, format!("key:{target}"))
            } else {
                (TaskPart::RunObligation, format!("decision:id:{target}"))
            };
            let Some(owner) = store
                .view()?
                .token_owners(part, &token, 1)?
                .first()
                .copied()
            else {
                return Ok(false);
            };
            let head = store.head(&format!("T-{:03}", owner.0))?;
            if check.kind == "answered" {
                Ok(head.question_by_key(target)?.is_some_and(|question| {
                    Some(question.revision.to_string()).as_deref() == check.expected.as_deref()
                        && question.state == corral_core::agent::QuestionState::Answered
                        && question.answer.is_some()
                }))
            } else {
                Ok(head.decision_by_id(target)?.is_some_and(|decision| {
                    decision.receipt.as_ref().is_some_and(|receipt| {
                        receipt.outcome == corral_core::ReceiptOutcome::Answered
                    })
                }))
            }
        }
        "status" | "landed" | "hold-lifted" => {
            let head = if let Ok(id) = Store::normalize_id(target) {
                store.head_if_exists(&id)?
            } else {
                None
            };
            match check.kind.as_str() {
                "status" => Ok(head
                    .is_some_and(|head| Some(head.status.as_str()) == check.expected.as_deref())),
                "landed" => Ok(head.is_some_and(|head| {
                    head.status == Status::Done
                        && head.current_run.as_deref().is_some_and(|run| {
                            head.landed.as_ref().is_some_and(|receipt| {
                                receipt.run == run.id && !receipt.sha.is_empty()
                            }) || run.autopilot.as_ref().is_some_and(|state| {
                                state.stage == corral_core::AutoStage::Done && state.sha.is_some()
                            })
                        })
                })),
                _ => Ok(head.map_or_else(
                    || {
                        store.config.projects.contains_key(target)
                            && !store.config.dispatch.paused
                            && !store.config.automation.paused
                            && store.config.ready_project_allowed(Some(target))
                    },
                    |head| !head.dispatch_hold,
                )),
            }
        }
        _ => Ok(false),
    }
}

fn checks_met(store: &Store, checks: &[ResumeCheck]) -> Result<bool> {
    if checks.is_empty() {
        return Ok(false);
    }
    for check in checks {
        if !check_met(store, check)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Publish satisfied typed conditions or expired PM waits once per plan revision.
/// The ledger remains unchanged: the PM must clear or renew the wait after reviewing it.
pub fn sweep(ctx: &Ctx) -> Result<usize> {
    // A native PM's durable work must survive a dead host and trigger supervisor recovery.
    let Some(pm) = pm_state::load_current(&ctx.store)? else {
        return Ok(0);
    };
    let Some(progress) = &pm.state.todo else {
        return Ok(0);
    };
    if progress.waits.is_empty() {
        return Ok(0);
    }
    let now = corral_core::clock::now();
    let mut publisher = None;
    let mut condition_store = None;
    let mut posted = 0;
    for wait in &progress.waits {
        // Once met, one keyed read suffices: do not evaluate again or send a later deadline.
        let deadline_key = publication_key(progress, &wait.step);
        let met_key = format!("{deadline_key}:met");
        if super::pm_wake_keys::contains(ctx, "wait", &met_key)? {
            continue;
        }
        let deadline_published = super::pm_wake_keys::contains(ctx, "wait", &deadline_key)?;
        // RunProgress has no wait timestamp: a missing side record is due once immediately.
        let record = side_record(
            ctx,
            &ContainerRef::Pm,
            plan(progress),
            progress.revision,
            &wait.step,
        )?;
        let deadline = record
            .as_ref()
            .map(|record| record.deadline)
            .or(wait.deadline);
        let checks = record
            .as_ref()
            .map_or(wait.checks.as_slice(), |record| &record.checks);
        let met = if checks.is_empty() {
            false
        } else {
            if condition_store.is_none() {
                condition_store = Some(ctx.store.read_only_snapshot()?);
            }
            checks_met(
                condition_store
                    .as_ref()
                    .context("PM wait condition snapshot missing")?,
                checks,
            )?
        };
        if !met && (deadline_published || deadline.is_some_and(|deadline| deadline > now)) {
            continue;
        }
        let key = if met { met_key } else { deadline_key };
        if publisher.is_none() {
            let Some(lease) = ctx.store.try_lock(WAIT_PUBLICATION_LOCK)? else {
                return Ok(0);
            };
            // A ledger update may have pruned its publication keys since our initial read.
            let unchanged = pm_state::load_current(&ctx.store)?.is_some_and(|current| {
                current.state.binding == pm.state.binding
                    && current.state.todo.as_ref().is_some_and(|todo| {
                        todo.revision == progress.revision
                            && todo.plan == progress.plan
                            && todo.spec_revision == progress.spec_revision
                    })
            });
            if !unchanged {
                return Ok(0);
            }
            publisher = Some((lease, Inbox::new(&ctx.store, Recipient::Pm)?));
        }
        let inbox = &publisher.as_ref().context("PM wait publisher missing")?.1;
        let condition = match &wait.on {
            WaitOn::Event { event } => format!("event {event}"),
            WaitOn::Owner { question } => format!("owner answer to {question}"),
        };
        let body = if met {
            let mut body = format!(
                "Plan {}, step {} condition met: {condition}.\nSatisfied checks:\n",
                plan(progress),
                wait.step,
            );
            // Canonical targets/revisions are bounded by the park parser; raw descriptions
            // can retain padding (for example revision zeroes) that would exceed the wire cap.
            for check in checks {
                write!(
                    body,
                    "{}:{}",
                    check.kind,
                    check.target.as_deref().unwrap_or_default()
                )?;
                if let Some(expected) = &check.expected {
                    let separator = if check.kind == "status" { '=' } else { '@' };
                    write!(body, "{separator}{expected}")?;
                }
                body.push('\n');
            }
            body.push_str(
                "Read the current plan, then clear or renew the wait. \
                 A wake is a pointer, not authority, and grants no owner authority.",
            );
            body
        } else {
            let deadline = match deadline {
                Some(deadline) => format!("review deadline {deadline} has passed"),
                None => "review deadline has passed (legacy wait without a stored deadline)".into(),
            };
            format!(
                "Plan {}, step {} waited on {condition}; its {deadline}.\n\
                 Read the current plan and condition, then clear or renew the wait. \
                 A wake is a pointer, not authority, and grants no owner authority.",
                plan(progress),
                wait.step
            )
        };
        posted += usize::from(ctx.store.records_transaction(|| {
            if !super::pm_wake_keys::claim(ctx, "wait", &key)? {
                return Ok(false);
            }
            inbox
                .post(Post {
                    kind: "pm_wait_wake",
                    urgency: Urgency::Wake,
                    author: "corral",
                    body: &body,
                    binding: MessageBinding::default(),
                    subject: Some(json!({
                        "key": key, "plan": plan(progress), "step": wait.step,
                        "revision": progress.revision, "project": pm.state.binding.project,
                        "condition_met": met,
                    })),
                    max_payload: super::delivery::INPUT_MAX_BYTES,
                })
                .with_context(|| format!("publishing PM wait wake {key}"))?;
            Ok(true)
        })?);
    }
    // Release the family lease before delivery takes dispatch; todo mutations take both.
    drop(publisher);
    drop(condition_store);
    if posted == 0 || !super::pm_wake::healthy(&pm) {
        return Ok(posted);
    }
    // pm_wake::repost retries the shared inbox. Only new wakes need another attempt here.
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
#[path = "pm_wait_wake_tests.rs"]
mod typed_tests;

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
            save_deadlines(&fx.ctx, &ContainerRef::Pm, None, &progress)?;
            let pm = pm.as_mut().unwrap();
            pm.state.todo = Some(progress.clone());
            pm.state.binding.health = corral_core::agent::HostHealth::Lost;
            Ok(((), true))
        })
        .unwrap();

        assert_eq!(sweep(&fx.ctx).unwrap(), 1);
        // An already-woken wait must not decode even one historical inbox message.
        let inbox = Inbox::new(&fx.ctx.store, Recipient::Pm).unwrap();
        let first = inbox.messages().unwrap().remove(0);
        let record_key = format!("{:020}", first.seq);
        let stored: serde_json::Value = inbox.record("messages", &record_key).unwrap().unwrap();
        inbox
            .save_record("messages", &record_key, &json!({"unreadable": "history"}))
            .unwrap();
        assert_eq!(sweep(&fx.ctx).unwrap(), 0);
        inbox.save_record("messages", &record_key, &stored).unwrap();
        let messages = inbox.messages().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].kind, "pm_wait_wake");
        assert_eq!(
            messages[0].subject.as_ref().unwrap()["key"],
            "wait:P1:gate:3"
        );
        let mut stored = pm_state::load_current(&fx.ctx.store)
            .unwrap()
            .unwrap()
            .state
            .todo
            .unwrap();
        assert!(stored.waits.iter().all(|wait| wait.deadline.is_none()));
        hydrate(&fx.ctx, &ContainerRef::Pm, &mut stored).unwrap();
        assert_eq!(stored, progress);

        // Renewing the plan revision gives a legacy wait its own single review wake.
        pm_state::update(&fx.ctx.store, |pm| {
            let previous = pm.as_ref().unwrap().state.todo.clone().unwrap();
            let mut next = previous.clone();
            hydrate(&fx.ctx, &ContainerRef::Pm, &mut next)?;
            next.revision += 1;
            next.waits[0].deadline = None;
            save_deadlines(&fx.ctx, &ContainerRef::Pm, Some(&previous), &next)?;
            pm.as_mut().unwrap().state.todo = Some(next);
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

    #[test]
    fn deadline_side_records_carry_forward_and_clear_atomically() {
        let fx = ts::fx();
        let at: Timestamp = "2026-11-11T14:00:00Z".parse().unwrap();
        let mut progress: RunProgress = serde_json::from_value(json!({
            "revision": 2, "spec_revision": "pm:corral", "plan": "P1",
            "phases": [], "waits": [
                {"step": "gate", "on": {"kind": "event", "event": "gate finishes"},
                 "deadline": at}
            ]
        }))
        .unwrap();
        progress.waits[0].checks =
            vec![corral_core::parse_park_check("status:T-001=done").unwrap()];
        let container = ContainerRef::Pm;
        let _lease = fx
            .ctx
            .store
            .try_lock(WAIT_PUBLICATION_LOCK)
            .unwrap()
            .unwrap();
        fx.ctx
            .store
            .records_transaction(|| save_deadlines(&fx.ctx, &container, None, &progress))
            .unwrap();
        let mut next: RunProgress =
            serde_json::from_value(serde_json::to_value(&progress).unwrap()).unwrap();
        assert_eq!(next.waits[0].deadline, None);
        assert!(next.waits[0].checks.is_empty());
        hydrate(&fx.ctx, &container, &mut next).unwrap();
        assert_eq!(next.waits[0].deadline, Some(at));
        assert_eq!(next.waits[0].checks, progress.waits[0].checks);
        next.revision += 1;
        fx.ctx
            .store
            .records_transaction(|| save_deadlines(&fx.ctx, &container, Some(&progress), &next))
            .unwrap();
        assert_eq!(
            deadline(&fx.ctx, &container, "P1", 2, "gate").unwrap(),
            None
        );
        assert_eq!(
            deadline(&fx.ctx, &container, "P1", 3, "gate").unwrap(),
            Some(at)
        );

        let mut cleared = next.clone();
        cleared.revision += 1;
        cleared.waits.clear();
        let aborted: Result<()> = fx.ctx.store.records_transaction(|| {
            save_deadlines(&fx.ctx, &container, Some(&next), &cleared)?;
            anyhow::bail!("simulated ledger mutation failure")
        });
        assert!(aborted.is_err());
        assert_eq!(
            deadline(&fx.ctx, &container, "P1", 3, "gate").unwrap(),
            Some(at)
        );
        fx.ctx
            .store
            .records_transaction(|| save_deadlines(&fx.ctx, &container, Some(&next), &cleared))
            .unwrap();
        assert_eq!(
            deadline(&fx.ctx, &container, "P1", 3, "gate").unwrap(),
            None
        );
    }
}
