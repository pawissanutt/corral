//! Delivery fixtures exercise the production reservation, attempt and receipt protocols.
use corral_core::attention::{Condition, Responsible, WaitClass};
use corral_core::inbox::{
    Envelope, Inbox, MAX_PAYLOAD, Message, MessageBinding, Post, Receipt, ReceiptLevel, Recipient,
    Recovery, SendOutcome, SessionBinding, TerminalState, Urgency,
};
use corral_core::{PlanRecord, PlanRun, Run, Status, Store, Task};
use jiff::Timestamp;
use serde_json::json;

use super::{Neighborhood, delivery};

fn at() -> Timestamp {
    "2026-10-25T00:00:00Z".parse().unwrap()
}

fn fixture() -> (tempfile::TempDir, Store, String, Vec<Recipient>) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::init(root.path()).unwrap();
    let task = store.create("Delivery", None, Status::Running, "").unwrap();
    let run = |id: &str| {
        serde_json::from_value::<Run>(json!({
            "id":id, "agent":"fixture", "delivery":"inbox",
            "worktree":"/fixture", "branch":"fixture", "base":"main", "base_sha":"abc",
            "started":at()
        }))
        .unwrap()
    };
    store
        .update(&task.id, |task| {
            let mut worker = run("R2");
            worker.review_runs.push(run("rv1"));
            task.runs.push(worker);
            task.plan_runs
                .push(serde_json::from_value::<PlanRun>(json!({
                    "id":"P1", "profile":"fixture", "harness":"fixture", "delivery":"inbox",
                    "spec_revision":"fixture", "started":at(), "output_dir":"/fixture",
                    "rounds":0, "state":"planning"
                }))?);
            task.plans.push(serde_json::from_value::<PlanRecord>(json!({
                "id":"D1", "spec_revision":"fixture",
                "artifact":{
                    "run":"P1", "kind":"report", "title":"Plan", "path":"plan.md",
                    "source":"/fixture/plan.md", "sha256":"abc", "bytes":0, "added":at()
                },
                "document":{
                    "goal":"fixture", "non_goals":[], "decisions":[], "affected_paths":[],
                    "contracts":[], "dependencies":[], "acceptance":[], "verification":[],
                    "owner_decisions":[]
                },
                "reviews":[{
                    "id":"pr1", "reviewer":"fixture", "policy_digest":"fixture",
                    "at":at(), "delivery":"inbox"
                }]
            }))?);
            Ok(())
        })
        .unwrap();
    let recipients = vec![
        Recipient::Run {
            task: task.id.clone(),
            run: "R2".into(),
        },
        Recipient::ReviewRun {
            task: task.id.clone(),
            run: "R2".into(),
            review: "rv1".into(),
        },
        Recipient::PlanRun {
            task: task.id.clone(),
            plan_run: "P1".into(),
        },
        Recipient::PlanReview {
            task: task.id.clone(),
            plan: "D1".into(),
            review: "pr1".into(),
        },
    ];
    (root, store, task.id, recipients)
}

fn post(inbox: &Inbox<'_>) -> Message {
    inbox
        .post(Post {
            kind: "owner_reply",
            urgency: Urgency::Wake,
            author: "owner",
            body: "Please act",
            binding: MessageBinding::default(),
            subject: None,
            max_payload: MAX_PAYLOAD,
        })
        .unwrap()
}

fn post_at(inbox: &Inbox<'_>, overflow: bool) -> Message {
    if overflow {
        for _ in 0..8 {
            post(inbox);
        }
    }
    post(inbox)
}

fn publish(inbox: &Inbox<'_>) -> Envelope {
    inbox
        .publish(
            &SessionBinding {
                session: "fixture".into(),
                endpoint: None,
                transcript: None,
            },
            "fixture",
            MAX_PAYLOAD,
            true,
        )
        .unwrap()
        .unwrap()
}

fn attempt(inbox: &Inbox<'_>, envelope: &mut Envelope, outcome: SendOutcome) {
    inbox.begin_attempt(envelope, at()).unwrap();
    inbox.finish_attempt(envelope, outcome).unwrap();
}

fn reject(inbox: &Inbox<'_>, envelope: &mut Envelope, retryable: bool) {
    attempt(
        inbox,
        envelope,
        SendOutcome::Rejected {
            reason: "fixture rejection".into(),
            retryable_after_fix: retryable,
        },
    );
}

fn fail(inbox: &Inbox<'_>, envelope: &Envelope) {
    for seq in &envelope.members {
        inbox
            .mark_terminal(*seq, TerminalState::Failed, "rejected", "fixture")
            .unwrap();
    }
    inbox.close_settled(envelope).unwrap();
}

fn classify(store: &Store, task: &Task, recipients: &[Recipient], known: bool) -> Vec<Condition> {
    let policy = store.effective_operating_policy().unwrap();
    let mut neighborhood = Neighborhood::new(std::slice::from_ref(task), &store.config, &policy);
    neighborhood.inventory_known = known;
    for recipient in recipients {
        let inbox = Inbox::new(store, recipient.clone()).unwrap();
        // Pass exactly the already-parsed task state to the production bounded reader.
        let mut snapshot = task.clone();
        let state = recipient.state_mut(&mut snapshot).unwrap();
        neighborhood.deliveries.insert(
            recipient.key().unwrap(),
            inbox.owner_entries(state).unwrap(),
        );
    }
    let mut out = Vec::new();
    // Future relative to fixture publications too, without sleeps or timing races.
    delivery::conditions(
        task,
        &neighborhood,
        "2099-01-01T00:00:00Z".parse().unwrap(),
        &mut out,
    );
    out
}

fn message_row<'a>(rows: &'a [Condition], id: &str) -> &'a Condition {
    rows.iter()
        .find(|row| {
            row.key
                .as_ref()
                .is_some_and(|key| key.0.ends_with(&format!("/{id}")))
        })
        .unwrap_or_else(|| panic!("missing {id}: {rows:#?}"))
}

#[test]
fn first_tracked_rejection_offers_an_executable_exact_recipient_retry() {
    for (recipient_index, overflow) in
        (0..4).flat_map(|index| [false, true].map(move |overflow| (index, overflow)))
    {
        for retryable in [true, false] {
            let (_root, store, id, recipients) = fixture();
            let inbox = Inbox::new(&store, recipients[recipient_index].clone()).unwrap();
            let message = post_at(&inbox, overflow);
            let mut envelope = publish(&inbox);
            reject(&inbox, &mut envelope, retryable);
            fail(&inbox, &envelope);
            assert!(!inbox.has_pending_delivery(true).unwrap());
            let task = store.get(&id).unwrap();
            let rows = classify(&store, &task, &recipients, false);
            let row = message_row(&rows, &message.id);
            assert_eq!(row.kind, "delivery_failed");
            assert_eq!(row.next_action.recipient, Some(inbox.key().into()));
            assert_eq!(row.since, Some(message.created));
            assert!(!row.resume.is_empty());
            if retryable {
                assert_eq!(
                    row.next_action.command,
                    Some(format!("corral inbox retry {}", message.id))
                );
                let retried = inbox.retry(message.seq).unwrap();
                assert_eq!(retried.recipient, message.recipient);
                assert_ne!(retried.id, message.id);
            } else {
                assert_eq!(row.responsible, Responsible::Owner);
                assert_eq!(
                    row.next_action.command,
                    Some(format!("corral inbox inspect-retry {}", message.id))
                );
                assert!(inbox.retry(message.seq).is_err());
            }
        }
    }
}

#[test]
fn earlier_uncertainty_survives_resend_then_rejection_on_every_recipient() {
    for (recipient_index, overflow) in
        (0..4).flat_map(|index| [false, true].map(move |overflow| (index, overflow)))
    {
        let (_root, store, id, recipients) = fixture();
        let inbox = Inbox::new(&store, recipients[recipient_index].clone()).unwrap();
        let message = post_at(&inbox, overflow);
        let mut first = publish(&inbox);
        attempt(
            &inbox,
            &mut first,
            SendOutcome::Uncertain {
                reason: "lost ack".into(),
            },
        );
        inbox.recover(Recovery::Resend).unwrap();
        let mut second = publish(&inbox);
        reject(&inbox, &mut second, true);
        fail(&inbox, &second);
        let rows = classify(&store, &store.get(&id).unwrap(), &recipients, false);
        let row = message_row(&rows, &message.id);
        assert_eq!(row.responsible, Responsible::Owner);
        assert!(row.reason.contains("Uncertain"));
        assert_eq!(
            row.next_action.command,
            Some(format!("corral inbox inspect-retry {}", message.id))
        );
        assert!(inbox.retry(message.seq).is_err());
    }
}

#[test]
fn terminal_only_cancelled_is_visible_and_attempted_terminal_labels_stay_manual() {
    for recipient_index in 0..4 {
        for terminal in [TerminalState::Cancelled, TerminalState::Abandoned] {
            for attempted in [false, true] {
                let (_root, store, id, recipients) = fixture();
                let inbox = Inbox::new(&store, recipients[recipient_index].clone()).unwrap();
                let message = post(&inbox);
                if attempted {
                    let mut envelope = publish(&inbox);
                    reject(&inbox, &mut envelope, true);
                }
                inbox
                    .mark_terminal(message.seq, terminal, "fixture", "fixture")
                    .unwrap();
                let rows = classify(&store, &store.get(&id).unwrap(), &recipients, false);
                let row = message_row(&rows, &message.id);
                if attempted {
                    assert_eq!(row.responsible, Responsible::Owner);
                    assert_eq!(
                        row.next_action.command,
                        Some(format!("corral inbox inspect-retry {}", message.id))
                    );
                } else {
                    assert!(!inbox.has_pending_delivery(true).unwrap());
                    assert_eq!(
                        row.next_action.command,
                        Some(format!("corral inbox retry {}", message.id))
                    );
                    assert_eq!(
                        inbox.retry(message.seq).unwrap().recipient,
                        message.recipient
                    );
                }
            }
        }
    }
}

#[test]
fn pending_or_unknown_history_never_uses_latest_rejection_as_proof() {
    for recipient_index in 0..4 {
        for pending in [false, true] {
            let (_root, store, id, recipients) = fixture();
            let recipient = &recipients[recipient_index];
            let inbox = Inbox::new(&store, recipient.clone()).unwrap();
            let message = post(&inbox);
            let mut envelope = publish(&inbox);
            reject(&inbox, &mut envelope, true);
            if pending {
                inbox.begin_attempt(&mut envelope, at()).unwrap();
            }
            fail(&inbox, &envelope);
            if !pending {
                // A legacy header can index identity but cannot certify old retry history.
                store
                    .update(&id, |task| {
                        recipient.state_mut(task)?.owner_header = None;
                        Ok(())
                    })
                    .unwrap();
                inbox.index_owner(64).unwrap();
            }
            let rows = classify(&store, &store.get(&id).unwrap(), &recipients, false);
            let row = message_row(&rows, &message.id);
            assert_eq!(row.kind, "delivery_evidence_incomplete");
            assert_eq!(row.responsible, Responsible::Pm);
            assert_eq!(
                row.next_action.command,
                Some(format!("corral inbox inspect-retry {}", message.id))
            );
        }
    }
}

#[test]
fn open_live_uncertainty_has_only_supported_owner_recovery_commands() {
    for recipient_index in 0..4 {
        for conflicted in [false, true] {
            let (_root, store, id, recipients) = fixture();
            let inbox = Inbox::new(&store, recipients[recipient_index].clone()).unwrap();
            let message = post(&inbox);
            let mut envelope = publish(&inbox);
            let outcome = if conflicted {
                SendOutcome::Conflicted {
                    reason: "conflict".into(),
                }
            } else {
                SendOutcome::Uncertain {
                    reason: "uncertain".into(),
                }
            };
            attempt(&inbox, &mut envelope, outcome);
            let task = store.get(&id).unwrap();
            let rows = classify(&store, &task, &recipients, false);
            let row = message_row(&rows, &message.id);
            assert_eq!(row.responsible, Responsible::Owner);
            assert_eq!(
                row.next_action.command,
                Some(format!("corral inbox recover {} --resend", inbox.key()))
            );
            assert!(row.next_action.description.contains("--abandon"));
            assert!(!row.next_action.description.contains("--submit"));
            inbox.recover(Recovery::Abandon).unwrap();
            let rows = classify(&store, &store.get(&id).unwrap(), &recipients, false);
            let row = message_row(&rows, &message.id);
            assert_eq!(
                row.next_action.command,
                Some(format!("corral inbox inspect-retry {}", message.id))
            );
        }
    }
}

fn observed(inbox: &Inbox<'_>, seq: u64) {
    inbox
        .record_receipt(
            seq,
            ReceiptLevel::Observed,
            &Receipt {
                envelope: None,
                binding: None,
                observed_at: at(),
                via: "fixture".into(),
                detail: json!({}),
            },
        )
        .unwrap();
}

#[test]
fn observation_without_a_report_remains_visible_when_transport_has_no_work() {
    for recipient_index in 0..4 {
        let (_root, store, id, recipients) = fixture();
        let inbox = Inbox::new(&store, recipients[recipient_index].clone()).unwrap();
        let message = post(&inbox);
        observed(&inbox, message.seq);
        assert!(!inbox.has_pending_delivery(true).unwrap());
        let mut task = store.get(&id).unwrap();
        if recipient_index == 0 {
            task.runs.last_mut().unwrap().idle_no_report = true;
        } else if recipient_index == 1 {
            task.runs
                .last_mut()
                .unwrap()
                .review_runs
                .last_mut()
                .unwrap()
                .idle_no_report = true;
        }
        let rows = classify(&store, &task, &recipients, false);
        let row = message_row(&rows, &message.id);
        assert_eq!(row.kind, "delivery_observed_unacted");
        assert_eq!(row.responsible, Responsible::Pm);
        assert!(row.next_action.command.is_none());
        assert!(!row.next_action.description.contains("--resend"));
    }
}

#[test]
fn ninth_terminal_identity_keeps_its_condition_and_age_through_restart_and_promotion() {
    for recipient_index in 0..4 {
        let (root, store, id, recipients) = fixture();
        let inbox = Inbox::new(&store, recipients[recipient_index].clone()).unwrap();
        let messages: Vec<_> = (0..9).map(|_| post(&inbox)).collect();
        let ninth = &messages[8];
        inbox
            .mark_terminal(ninth.seq, TerminalState::Cancelled, "unsent", "fixture")
            .unwrap();
        let before = classify(&store, &store.get(&id).unwrap(), &recipients, false);
        assert!(before.iter().any(|row| row.kind == "inbox_overflow"));
        let overflow = before
            .iter()
            .find(|row| row.kind == "inbox_overflow")
            .unwrap();
        assert_eq!(overflow.class, WaitClass::PmAction);
        assert!(overflow.key.is_some());
        let original = message_row(&before, &ninth.id).clone();
        for message in &messages[..8] {
            observed(&inbox, message.seq);
        }
        let reopened = Store::open(root.path()).unwrap();
        let inbox = Inbox::new(&reopened, recipients[recipient_index].clone()).unwrap();
        inbox.maintain_owner_header(at()).unwrap();
        let after = classify(&reopened, &reopened.get(&id).unwrap(), &recipients, false);
        assert!(!after.iter().any(|row| row.kind == "inbox_overflow"));
        assert_eq!(message_row(&after, &ninth.id), &original);
    }
}

#[test]
fn full_capacity_and_index_debt_suppress_safe_retry_commands() {
    for recipient_index in 0..4 {
        for debt in [false, true] {
            let (_root, store, id, recipients) = fixture();
            let recipient = &recipients[recipient_index];
            let inbox = Inbox::new(&store, recipient.clone()).unwrap();
            let message = post(&inbox);
            if debt {
                store
                    .update(&id, |task| {
                        recipient
                            .state_mut(task)?
                            .owner_header
                            .as_mut()
                            .unwrap()
                            .legacy_debt = Some(corral_core::inbox::LegacyDebt {
                            phase: corral_core::inbox::IndexPhase::Collect,
                            next_seq: 1,
                            end_seq: 1,
                            latest_observed_owner: None,
                            started: at(),
                        });
                        Ok(())
                    })
                    .unwrap();
            } else {
                for _ in 1..16 {
                    post(&inbox);
                }
            }
            inbox
                .mark_terminal(message.seq, TerminalState::Cancelled, "unsent", "fixture")
                .unwrap();
            let rows = classify(&store, &store.get(&id).unwrap(), &recipients, false);
            let row = message_row(&rows, &message.id);
            assert_eq!(
                row.next_action.command,
                Some(format!("corral inbox inspect-retry {}", message.id))
            );
            assert!(rows.iter().all(|row| {
                row.next_action
                    .command
                    .as_ref()
                    .is_none_or(|cmd| !cmd.starts_with("corral inbox retry "))
            }));
            let capacity = rows
                .iter()
                .find(|row| {
                    row.kind
                        == if debt {
                            "inbox_header_index_pending"
                        } else {
                            "inbox_capacity"
                        }
                })
                .unwrap();
            if debt {
                assert_eq!(
                    capacity.next_action.command,
                    Some(format!(
                        "corral inbox index-owner {} --limit 64",
                        inbox.key()
                    ))
                );
            } else {
                assert!(capacity.reason.contains(&format!("{}#16", inbox.key())));
                assert!(capacity.key.is_none());
                assert_eq!(capacity.class, WaitClass::Capacity);
            }
        }
    }
}

#[test]
fn legacy_pending_reply_covers_every_recipient_without_reviewer_task_reply() {
    for recipient_index in 0..4 {
        let (_root, store, id, recipients) = fixture();
        let mut task = store.get(&id).unwrap();
        match recipient_index {
            0 => task.runs.last_mut().unwrap().pending_reply = Some("pending".into()),
            1 => {
                task.runs
                    .last_mut()
                    .unwrap()
                    .review_runs
                    .last_mut()
                    .unwrap()
                    .pending_reply = Some("pending".into())
            }
            2 => task.plan_runs.last_mut().unwrap().pending_reply = Some("pending".into()),
            _ => {
                task.plans
                    .last_mut()
                    .unwrap()
                    .reviews
                    .last_mut()
                    .unwrap()
                    .pending_reply = Some("pending".into())
            }
        }
        let rows = classify(&store, &task, &recipients, false);
        let row = rows
            .iter()
            .find(|row| row.kind == "delivery_pending_reply")
            .unwrap();
        assert_eq!(
            row.next_action.recipient,
            Some(recipients[recipient_index].key().unwrap())
        );
        assert_eq!(
            row.next_action.command,
            matches!(recipient_index, 0 | 2).then(|| format!("corral reply {id} --resend"))
        );
        assert!(!row.resume.is_empty());
    }
}

#[test]
fn known_gone_is_orphaned_but_unknown_inventory_never_invents_absence() {
    let (_root, store, id, recipients) = fixture();
    let inbox = Inbox::new(&store, recipients[0].clone()).unwrap();
    let message = post(&inbox);
    let mut task = store.get(&id).unwrap();
    task.runs.last_mut().unwrap().agent_state = Some("gone".into());
    let unknown = classify(&store, &task, &recipients, false);
    assert_eq!(
        message_row(&unknown, &message.id).kind,
        "delivery_unobserved"
    );
    let known = classify(&store, &task, &recipients, true);
    let row = message_row(&known, &message.id);
    assert_eq!(row.kind, "delivery_orphaned");
    assert_eq!(
        row.next_action.command,
        Some(format!("corral inbox inspect-retry {}", message.id))
    );
}

#[test]
fn unattempted_replacement_preserves_safe_rejected_only_proof() {
    for recipient_index in 0..4 {
        let (_root, store, id, recipients) = fixture();
        let inbox = Inbox::new(&store, recipients[recipient_index].clone()).unwrap();
        let message = post(&inbox);
        let first = publish(&inbox);
        inbox.retire_unattempted(&first).unwrap();
        let mut second = publish(&inbox);
        reject(&inbox, &mut second, true);
        fail(&inbox, &second);
        let rows = classify(&store, &store.get(&id).unwrap(), &recipients, false);
        assert_eq!(
            message_row(&rows, &message.id).next_action.command,
            Some(format!("corral inbox retry {}", message.id))
        );
        assert_eq!(
            inbox.retry(message.seq).unwrap().recipient,
            message.recipient
        );
    }
}

#[test]
fn held_delivery_waits_for_transport_with_owner_only_exact_recipient_flush() {
    for recipient_index in 0..4 {
        let (_root, store, id, recipients) = fixture();
        let inbox = Inbox::new(&store, recipients[recipient_index].clone()).unwrap();
        let message = post(&inbox);
        let mut envelope = publish(&inbox);
        attempt(
            &inbox,
            &mut envelope,
            SendOutcome::Held {
                reason: "transport hold".into(),
            },
        );
        let rows = classify(&store, &store.get(&id).unwrap(), &recipients, false);
        let row = message_row(&rows, &message.id);
        assert_eq!(row.kind, "delivery_held");
        assert_eq!(row.responsible, Responsible::External);
        assert_eq!(
            row.next_action.command,
            Some(format!("corral inbox flush {}", inbox.key()))
        );
        assert!(row.next_action.requires_owner);
    }
}

#[test]
fn a_nonopen_uncertain_publication_never_advertises_recover() {
    for recipient_index in 0..4 {
        let (_root, store, id, recipients) = fixture();
        let recipient = &recipients[recipient_index];
        let inbox = Inbox::new(&store, recipient.clone()).unwrap();
        let message = post(&inbox);
        let mut envelope = publish(&inbox);
        attempt(
            &inbox,
            &mut envelope,
            SendOutcome::Uncertain {
                reason: "lost ack".into(),
            },
        );
        inbox.recover(Recovery::Resend).unwrap();
        // Recovery retires E1 without fabricating a terminal state or observation.
        assert!(inbox.state().unwrap().open_envelope.is_none());
        let rows = classify(&store, &store.get(&id).unwrap(), &recipients, false);
        let row = message_row(&rows, &message.id);
        assert_eq!(row.responsible, Responsible::Owner);
        assert_eq!(
            row.next_action.command,
            Some(format!("corral inbox inspect-retry {}", message.id))
        );
    }
}

#[test]
fn cancelled_pending_attempt_is_owner_disposition_not_pm_retry() {
    for recipient_index in 0..4 {
        let (_root, store, id, recipients) = fixture();
        let inbox = Inbox::new(&store, recipients[recipient_index].clone()).unwrap();
        let message = post(&inbox);
        let mut envelope = publish(&inbox);
        inbox.begin_attempt(&mut envelope, at()).unwrap();
        inbox
            .mark_terminal(
                message.seq,
                TerminalState::Cancelled,
                "cancelled",
                "fixture",
            )
            .unwrap();
        let rows = classify(&store, &store.get(&id).unwrap(), &recipients, false);
        let row = message_row(&rows, &message.id);
        assert_eq!(row.responsible, Responsible::Owner);
        assert_eq!(
            row.next_action.command,
            Some(format!("corral inbox inspect-retry {}", message.id))
        );
    }
}

#[test]
fn observed_successor_suppresses_only_settled_older_obligations_before_maintenance() {
    for recipient_index in 0..4 {
        let (_root, store, id, recipients) = fixture();
        let inbox = Inbox::new(&store, recipients[recipient_index].clone()).unwrap();
        let cancelled = post(&inbox);
        inbox
            .mark_terminal(cancelled.seq, TerminalState::Cancelled, "unsent", "fixture")
            .unwrap();
        let unresolved = post(&inbox);
        let successor = post(&inbox);
        observed(&inbox, successor.seq);
        let rows = classify(&store, &store.get(&id).unwrap(), &recipients, false);
        assert!(!rows.iter().any(|row| {
            row.key
                .as_ref()
                .is_some_and(|key| key.0.ends_with(&format!("/{}", cancelled.id)))
        }));
        assert_eq!(
            message_row(&rows, &unresolved.id).kind,
            "delivery_unobserved"
        );
    }
}

#[test]
fn classification_performs_no_io_after_the_bounded_projection_is_loaded() {
    use corral_core::work::{Work, count};
    let (_root, store, id, recipients) = fixture();
    let inbox = Inbox::new(&store, recipients[0].clone()).unwrap();
    let message = post(&inbox);
    inbox
        .mark_terminal(message.seq, TerminalState::Cancelled, "unsent", "fixture")
        .unwrap();
    let mut task = store.get(&id).unwrap();
    let policy = store.effective_operating_policy().unwrap();
    let state = recipients[0].state_mut(&mut task).unwrap().clone();
    let mut neighborhood = Neighborhood::new(std::slice::from_ref(&task), &store.config, &policy);
    neighborhood
        .deliveries
        .insert(inbox.key().into(), inbox.owner_entries(&state).unwrap());
    let before = Work::ALL.map(count);
    let mut out = Vec::new();
    delivery::conditions(&task, &neighborhood, at(), &mut out);
    assert_eq!(Work::ALL.map(count), before);
    assert_eq!(message_row(&out, &message.id).kind, "delivery_cancelled");
}

#[test]
fn retained_owner_headers_remain_authoritative_under_legacy_mode() {
    use corral_core::inbox::DeliveryMode;
    for recipient_index in 0..4 {
        let (_root, store, id, recipients) = fixture();
        let inbox = Inbox::new(&store, recipients[recipient_index].clone()).unwrap();
        let message = post(&inbox);
        inbox
            .mark_terminal(message.seq, TerminalState::Cancelled, "unsent", "fixture")
            .unwrap();
        let mut task = store.get(&id).unwrap();
        match recipient_index {
            0 => task.runs.last_mut().unwrap().delivery = DeliveryMode::Legacy,
            1 => {
                task.runs
                    .last_mut()
                    .unwrap()
                    .review_runs
                    .last_mut()
                    .unwrap()
                    .delivery = DeliveryMode::Legacy
            }
            2 => task.plan_runs.last_mut().unwrap().delivery = DeliveryMode::Legacy,
            _ => {
                task.plans
                    .last_mut()
                    .unwrap()
                    .reviews
                    .last_mut()
                    .unwrap()
                    .delivery = DeliveryMode::Legacy
            }
        }
        let rows = classify(&store, &task, &recipients, false);
        assert_eq!(message_row(&rows, &message.id).kind, "delivery_cancelled");
    }
}
