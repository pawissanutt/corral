//! Delivery assessment uses only retained identities and keyed receipt projections.
use corral_core::attention::{Condition, NextAction, Responsible, ResumeCheck, WaitClass};
use corral_core::inbox::{
    DeliveryMode, DeliveryState, InboxState, OwnerEntry, Recipient, RetryCoverage, RetryHistory,
    RetryRoute, TerminalState,
};
use corral_core::{Run, Task};
use jiff::Timestamp;
use sha2::{Digest, Sha256};

use super::{Neighborhood, row};

/// A change request with no delivery attempt raises its own attention kind this soon,
/// well before the generic ten-minute unobserved bound.
const CHANGES_UNDELIVERED_SECS: i64 = 60;

struct Delivery<'a> {
    recipient: Recipient,
    state: &'a InboxState,
    mode: DeliveryMode,
    pending: Option<&'a str>,
    unacted: bool,
    orphaned: bool,
    task_reply: bool,
}

pub(super) fn conditions(
    task: &Task,
    neighborhood: &Neighborhood<'_>,
    now: Timestamp,
    out: &mut Vec<Condition>,
) {
    let mut recipients = Vec::new();
    if let Some(run) = task.current_run() {
        let recipient = Recipient::Run {
            task: task.id.clone(),
            run: run.id.clone(),
        };
        let native = run.agent_host.is_some()
            || run
                .harness
                .as_deref()
                .is_some_and(corral_core::is_pi_native);
        let open = if native {
            corral_core::agent::container_closure(
                task,
                &corral_core::agent::ContainerRef::Worker {
                    task: task.id.clone(),
                    run: run.id.clone(),
                },
            )
            .is_none()
        } else {
            run.outcome.is_none() && run.ended.is_none()
        };
        if open {
            recipients.push(run_delivery(recipient, run, true, neighborhood));
        }
        for review in &run.review_runs {
            let container = corral_core::agent::ContainerRef::Reviewer {
                task: task.id.clone(),
                run: run.id.clone(),
                review: review.id.clone(),
            };
            if corral_core::agent::container_closure(task, &container).is_none() {
                recipients.push(run_delivery(
                    Recipient::ReviewRun {
                        task: task.id.clone(),
                        run: run.id.clone(),
                        review: review.id.clone(),
                    },
                    review,
                    false,
                    neighborhood,
                ));
            }
        }
    }
    if let Some(run) = task.plan_runs.last().filter(|run| run.holds_slot()) {
        recipients.push(Delivery {
            recipient: Recipient::PlanRun {
                task: task.id.clone(),
                plan_run: run.id.clone(),
            },
            state: &run.inbox,
            mode: run.delivery,
            pending: run.pending_reply.as_deref(),
            // An open planning/changes phase has not submitted its matching report.
            unacted: true,
            orphaned: false,
            task_reply: true,
        });
    }
    for plan in &task.plans {
        if let Some(review) = plan
            .reviews
            .last()
            .filter(|review| review.verdict.is_none())
        {
            recipients.push(Delivery {
                recipient: Recipient::PlanReview {
                    task: task.id.clone(),
                    plan: plan.id.clone(),
                    review: review.id.clone(),
                },
                state: &review.inbox,
                mode: review.delivery,
                pending: review.pending_reply.as_deref(),
                // An open plan review has no matching verdict.
                unacted: true,
                orphaned: false,
                task_reply: false,
            });
        }
    }
    for delivery in recipients {
        let Ok(key) = delivery.recipient.key() else {
            continue;
        };
        let entries = neighborhood.deliveries.get(&key);
        if let Some(pending) = delivery.pending
            && entries.is_none_or(Vec::is_empty)
        {
            let digest = format!("{:x}", Sha256::digest(pending.as_bytes()));
            let mut condition = row(
                task,
                now,
                "delivery_pending_reply",
                WaitClass::PmAction,
                Responsible::Pm,
                (
                    Some(format!("{key}:{digest}:{:?}", task.status_since)),
                    task.status_since,
                ),
                format!(
                    "Legacy pending reply for {key}; delivery has no retained message identity"
                ),
            );
            condition.next_action = NextAction {
                command: delivery
                    .task_reply
                    .then(|| format!("corral reply {} --resend", task.id)),
                description: if delivery.task_reply {
                    "Resend the existing pending reply to the current live worker or planner".into()
                } else {
                    "Inspect the named reviewer and choose an explicit owner disposition".into()
                },
                requires_owner: !delivery.task_reply,
                recipient: Some(key.clone()),
            };
            condition.resume.push(check(
                "delivery_disposition",
                &key,
                "Reply is delivered, explicitly disposed, or its recipient ends",
                false,
            ));
            out.push(condition);
        }
        if delivery.mode != DeliveryMode::Inbox && delivery.state.owner_header.is_none() {
            continue;
        }
        header_conditions(task, &delivery, &key, now, out);
        let blocked = delivery
            .state
            .owner_header
            .as_ref()
            .is_none_or(|header| header.len() >= 16 || header.legacy_debt.is_some());
        if let Some(entries) = entries {
            let latest_observed = entries
                .iter()
                .filter(|entry| entry.observed.is_some())
                .map(|entry| entry.message.seq)
                .max();
            for entry in entries {
                if latest_observed.is_some_and(|seq| entry.message.seq < seq)
                    && (entry.observed.is_some() || entry.terminal.is_some())
                {
                    continue;
                }
                if entry.retry_claimed_at.is_some()
                    && entry.retry_successor.as_ref().is_some_and(|id| {
                        entries.iter().any(|successor| {
                            successor.message.seq > entry.message.seq
                                && *id == format!("{key}#{}", successor.message.seq)
                                && !successor.publication_gap
                        })
                    })
                {
                    continue;
                }
                entry_condition(task, &delivery, &key, entry, blocked, now, out);
            }
        }
    }
}

fn run_delivery<'a>(
    recipient: Recipient,
    run: &'a Run,
    task_reply: bool,
    neighborhood: &Neighborhood<'_>,
) -> Delivery<'a> {
    Delivery {
        recipient,
        state: &run.inbox,
        mode: run.delivery,
        pending: run.pending_reply.as_deref(),
        unacted: run.idle_no_report && run.outcome.is_none() && run.ended.is_none(),
        orphaned: neighborhood.inventory_known && run.agent_state.as_deref() == Some("gone"),
        // Task reply never targets a reviewer, or a worker which has already reported.
        task_reply: task_reply && run.outcome.is_none() && run.ended.is_none(),
    }
}

fn header_conditions(
    task: &Task,
    delivery: &Delivery<'_>,
    key: &str,
    now: Timestamp,
    out: &mut Vec<Condition>,
) {
    let header = delivery.state.owner_header.as_ref();
    if header.is_none() && delivery.state.next_seq > 1
        || header.is_some_and(|header| header.legacy_debt.is_some())
    {
        let since = header.and_then(|header| header.legacy_debt.as_ref().map(|debt| debt.started));
        let mut condition = row(
            task,
            now,
            "inbox_header_index_pending",
            WaitClass::PmAction,
            Responsible::Pm,
            (
                Some(since.map_or_else(|| key.into(), |at| format!("{key}:{at}"))),
                since,
            ),
            format!("Owner identities for {key} require explicit bounded indexing"),
        );
        condition.next_action = action(
            format!("corral inbox index-owner {key} --limit 64"),
            "Index the next bounded page; indexing does not certify retry safety",
            key,
            false,
        );
        condition.resume.push(check(
            "inbox_header_index_complete",
            key,
            "Legacy owner identity cursor has passed its frozen end sequence",
            true,
        ));
        out.push(condition);
    }
    let Some(header) = header else {
        return;
    };
    let ids = header
        .iter()
        .map(|message| format!("{key}#{}", message.seq))
        .collect::<Vec<_>>();
    for (kind, active, since, description) in [
        (
            "inbox_overflow",
            !header.overflow_entries.is_empty(),
            header.overflow_since,
            "Primary capacity must free for FIFO promotion",
        ),
        (
            "inbox_capacity",
            header.len() >= 16,
            header.full_since,
            "A retained obligation must resolve or be superseded before new admission",
        ),
    ] {
        if !active {
            continue;
        }
        let mut condition = row(
            task,
            now,
            kind,
            if kind == "inbox_overflow" {
                WaitClass::PmAction
            } else {
                WaitClass::Capacity
            },
            Responsible::Pm,
            (
                (kind == "inbox_overflow")
                    .then(|| since.map_or_else(|| key.into(), |at| format!("{key}:{at}"))),
                since,
            ),
            format!(
                "{key} retains {}/16 owner messages: {}",
                header.len(),
                ids.join(", ")
            ),
        );
        condition.next_action = action(
            format!("corral inbox show {}", ids[0]),
            description,
            key,
            false,
        );
        condition.resume.push(check(kind, key, description, true));
        out.push(condition);
    }
}

fn entry_condition(
    task: &Task,
    delivery: &Delivery<'_>,
    key: &str,
    entry: &OwnerEntry,
    capacity_blocked: bool,
    now: Timestamp,
    out: &mut Vec<Condition>,
) {
    let id = format!("{key}#{}", entry.message.seq);
    let evidence = &entry.message.retry_evidence;
    let current = entry.message.delivery.as_ref().map(|header| header.state);
    let inspect = format!("corral inbox inspect-retry {id}");
    let mut kind = "delivery_unobserved";
    let mut responsible = Responsible::Pm;
    let mut class = WaitClass::PmAction;
    let mut command = Some("corral sync".to_string());
    let mut reason = format!("{id} has not been observed within the existing ten-minute bound");
    let mut description = "Run the existing delivery pass; do not post a duplicate".to_string();
    let mut resume = "Message is observed, semantically completed, superseded, or recipient ends";
    let mut machine_checkable = true;
    if entry.retry_claimed_at.is_some() {
        kind = "delivery_retry_pending";
        responsible = Responsible::Owner;
        class = WaitClass::OwnerDecision;
        reason = format!(
            "{id} has a retry claim; successor {:?} has no verified publication in the bounded header",
            entry.retry_successor
        );
        command = Some(inspect.clone());
        description =
            "Inspect this claimed retry and its successor; do not replay the predecessor".into();
        resume =
            "The exact successor is committed and published, or the owner disposes the obligation";
    } else if entry.publication_gap {
        kind = "delivery_publication_gap";
        reason = format!("{id} has a committed identity but its message file is missing");
        command = Some(inspect.clone());
        description =
            "Inspect this exact committed publication; missing files do not prove non-delivery"
                .into();
        resume = "Committed publication is repaired or explicitly disposed";
        machine_checkable = false;
        if let RetryHistory::Barred { first } = &evidence.history {
            responsible = Responsible::Owner;
            class = WaitClass::OwnerDecision;
            reason.push_str(&format!("; retry barred by first witness {first:?}"));
        }
    } else if entry.observed.is_some() {
        if !delivery.unacted {
            return;
        }
        kind = "delivery_observed_unacted";
        reason = format!("{id} was observed but its recipient has no matching report or verdict");
        command = None;
        description = if delivery.task_reply && !capacity_blocked {
            "Send fresh task reply text if needed; do not replay the observed message".into()
        } else if delivery.task_reply {
            "Wait for the matching report; clear capacity or indexing debt before new messages"
                .into()
        } else {
            "Use the reviewer watchdog or explicit owner recovery; do not replay this message"
                .into()
        };
        resume = "Recipient produces the matching report or verdict, or ends";
    } else if let Some(terminal) = entry.terminal {
        kind = match terminal {
            TerminalState::Failed => "delivery_failed",
            TerminalState::Cancelled => "delivery_cancelled",
            TerminalState::Abandoned => "delivery_abandoned",
        };
        reason = format!("{id} is {terminal:?}, not observed or semantically completed");
        command = Some(inspect.clone());
        description = "Inspect the exact message and choose an explicit disposition".into();
        resume = "Obligation is safely retried, explicitly disposed, superseded, completed, or recipient ends";
        if let RetryHistory::Barred { first } = &evidence.history {
            responsible = Responsible::Owner;
            class = WaitClass::OwnerDecision;
            reason.push_str(&format!("; retry barred by first witness {first:?}"));
        } else if terminal != TerminalState::Failed
            && (matches!(evidence.history, RetryHistory::RejectedOnly { .. })
                || evidence.pending.is_some()
                || entry
                    .message
                    .delivery
                    .as_ref()
                    .is_some_and(|delivery| delivery.attempt.is_some()))
        {
            responsible = Responsible::Owner;
            class = WaitClass::OwnerDecision;
            reason.push_str("; attempted Cancelled/Abandoned messages require owner disposition");
        } else if incomplete(entry) {
            kind = "delivery_evidence_incomplete";
            reason.push_str("; cumulative delivery evidence is incomplete");
            machine_checkable = false;
        } else if entry.retry_eligible() && !delivery.orphaned {
            if capacity_blocked {
                description = "Free owner-header capacity or finish indexing before retrying this exact message".into();
            } else {
                command = Some(format!("corral inbox retry {id}"));
                description =
                    "Fix the known cause, then retry this exact message to its original recipient"
                        .into();
            }
        } else {
            responsible = Responsible::Owner;
            class = WaitClass::OwnerDecision;
        }
    } else if matches!(
        current,
        Some(DeliveryState::Uncertain | DeliveryState::Conflicted)
    ) {
        kind = if current == Some(DeliveryState::Conflicted) {
            "delivery_conflicted"
        } else {
            "delivery_uncertain"
        };
        responsible = Responsible::Owner;
        class = WaitClass::OwnerDecision;
        reason = format!(
            "{id} has {current:?} delivery; cumulative history is {:?}",
            evidence.history
        );
        let open = entry.message.envelope.is_some()
            && entry.message.envelope == delivery.state.open_envelope
            && delivery.state.binding.is_some()
            && !delivery.orphaned;
        if open {
            command = Some(format!("corral inbox recover {key} --resend"));
            description = format!(
                "Owner acknowledges duplicate_possible with --resend, or chooses corral inbox recover {key} --abandon"
            );
        } else {
            command = Some(inspect.clone());
            description = "Inspect the exact message and choose owner recovery; no open live envelope permits recover".into();
        }
        machine_checkable = false;
        resume = "Owner chooses an explicit disposition, message is observed, completed, or recipient ends";
    } else if current == Some(DeliveryState::Held) && !delivery.orphaned {
        kind = "delivery_held";
        responsible = Responsible::External;
        class = WaitClass::ExternalOperation;
        reason = format!("{id} is held by the external transport");
        command = Some(format!("corral inbox flush {key}"));
        description = "Wait for observation, or the owner may flush this exact recipient".into();
    } else if delivery.orphaned {
        kind = "delivery_orphaned";
        responsible = Responsible::Owner;
        class = WaitClass::OwnerDecision;
        reason = format!("{id} belongs to a live recipient whose agent is known gone");
        command = Some(inspect.clone());
        description = "Inspect the exact message and existing run recovery choices".into();
        machine_checkable = false;
    } else if entry.message.kind == "changes_requested"
        && entry.message.envelope.is_none()
        && now.duration_since(entry.message.created).as_secs() >= CHANGES_UNDELIVERED_SECS
    {
        // The worker is waiting on a change request no delivery pass has even attempted;
        // a generic silent_turn would hide that the request never reached it.
        kind = "changes_not_delivered";
        reason = format!(
            "Changes requested on {} ({id}) were never delivered to the worker: no delivery attempt has been made",
            task.id
        );
        command = Some("corral sync".to_string());
        description = format!(
            "Run the delivery pass and inspect `corral inbox show {id}` for the blocked reason; do not post a duplicate"
        );
        resume = "Change request is observed, superseded, or its recipient ends";
    } else if incomplete(entry) {
        kind = "delivery_evidence_incomplete";
        reason =
            format!("{id} has unknown, pending, or contradictory cumulative delivery evidence");
        command = Some(inspect.clone());
        description = "Inspect this exact message; latest delivery state is not retry proof".into();
        resume = "Manual evidence disposition, actual observation, completion, supersession, or recipient ends";
        machine_checkable = false;
    } else if now.duration_since(entry.message.created).as_secs() < 600 {
        return;
    }
    let mut condition = row(
        task,
        now,
        kind,
        class,
        responsible,
        (Some(id.clone()), Some(entry.message.created)),
        reason,
    );
    condition.next_action = NextAction {
        command,
        description,
        requires_owner: responsible == Responsible::Owner || kind == "delivery_held",
        recipient: Some(key.into()),
    };
    condition.resume.push(check(
        "delivery_disposition",
        &id,
        resume,
        machine_checkable,
    ));
    if !machine_checkable {
        condition.resume.push(check(
            "delivery_semantic_completion",
            &id,
            "Message is observed, semantically completed, superseded, or recipient ends",
            true,
        ));
    }
    out.push(condition);
}

fn incomplete(entry: &OwnerEntry) -> bool {
    let evidence = &entry.message.retry_evidence;
    evidence.coverage != RetryCoverage::Complete
        || !entry.message.claim_mirrored
        || evidence.route != RetryRoute::EnvelopeOnly
        || evidence.pending.is_some()
        || entry.message.envelope.is_some() && entry.message.delivery.is_none()
        || matches!(&evidence.history, RetryHistory::RejectedOnly { last, .. }
            if entry.terminal == Some(TerminalState::Failed)
                && (entry.message.envelope != Some(last.envelope)
                    || entry.message.delivery.as_ref().is_none_or(|delivery|
                        delivery.state != DeliveryState::Rejected
                            || delivery.attempt != Some(*last))))
        || entry.message.delivery.as_ref().is_some_and(|delivery| {
            matches!(
                delivery.state,
                DeliveryState::Unknown | DeliveryState::Dispatching
            ) || entry.terminal.is_some()
                && matches!(
                    delivery.state,
                    DeliveryState::Accepted
                        | DeliveryState::Admitted
                        | DeliveryState::Held
                        | DeliveryState::Uncertain
                        | DeliveryState::Conflicted
                )
        })
        || entry.terminal.is_some() && entry.admitted.is_some()
}

fn check(kind: &str, target: &str, description: &str, machine_checkable: bool) -> ResumeCheck {
    ResumeCheck {
        kind: kind.into(),
        target: Some(target.into()),
        expected: Some("resolved".into()),
        description: description.into(),
        machine_checkable,
        satisfied: machine_checkable.then_some(false),
    }
}

fn action(command: String, description: &str, recipient: &str, requires_owner: bool) -> NextAction {
    NextAction {
        command: Some(command),
        description: description.into(),
        requires_owner,
        recipient: Some(recipient.into()),
    }
}
