//! Recipient-bound delivery recovery (T-426): actual begin_attempt -> Rejected -> fail on every
//! task recipient kind, exact-ID retry, and the installed open-envelope recovery choices.
use std::cell::RefCell;

use corral_core::inbox::{
    Inbox, MAX_PAYLOAD, Message, MessageBinding, Post, Receipt, ReceiptLevel, Recipient, Recovery,
    RetryCoverage, RetryHistory, TerminalState, Urgency,
};
use corral_core::{Run, Status, Store, Verdict};
use corral_ops::Ctx;
use corral_ops::delivery::{
    BindOutcome, Capabilities, DeliveryAdapter, DeliveryStatus, Observation, Readiness,
    RecipientRef, SendOutcome, SessionBinding, deliver,
};
use serde_json::{Value, json};

const PLAN: &str = "---\ngoal: Spin\nnon_goals: []\ndecisions: []\naffected_paths: [w.rs]\ncontracts: []\ndependencies: []\nacceptance:\n  - criterion: C1\n    scenario: spin\n    expected: clockwise\nverification: [cargo test]\nowner_decisions: []\n---\nDetails.\n";

/// One adapter whose next send outcome the test chooses; it records every recipient it sent to.
struct Transport {
    outcome: RefCell<SendOutcome>,
    sent: RefCell<Vec<String>>,
}

impl Transport {
    fn new(outcome: SendOutcome) -> Self {
        Self {
            outcome: RefCell::new(outcome),
            sent: RefCell::new(Vec::new()),
        }
    }
    fn set(&self, outcome: SendOutcome) {
        *self.outcome.borrow_mut() = outcome;
    }
    fn sent_to(&self, recipient: &Recipient) -> usize {
        let key = recipient.key().unwrap();
        self.sent.borrow().iter().filter(|k| **k == key).count()
    }
}

impl DeliveryAdapter for Transport {
    fn harness(&self) -> &'static str {
        "fixture"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            idempotent_retry: false,
            admission: false,
            non_waking: false,
            max_payload: MAX_PAYLOAD,
            send_budget: None,
            transcript_reader: false,
        }
    }
    fn readiness(&self, _: &Ctx, _: &RecipientRef) -> anyhow::Result<Readiness> {
        Ok(Readiness::Native)
    }
    fn bind(&self, _: &Ctx, recipient: &RecipientRef) -> anyhow::Result<BindOutcome> {
        Ok(BindOutcome::Bound(SessionBinding {
            session: recipient.key()?,
            endpoint: None,
            transcript: None,
        }))
    }
    fn send(
        &self,
        _: &Ctx,
        binding: &SessionBinding,
        _: &corral_core::inbox::Envelope,
    ) -> SendOutcome {
        self.sent.borrow_mut().push(binding.session.clone());
        self.outcome.borrow().clone()
    }
    fn observe(
        &self,
        _: &Ctx,
        _: &SessionBinding,
        _: &mut corral_core::inbox::ObserveCursor,
    ) -> anyhow::Result<Vec<Observation>> {
        Ok(Vec::new())
    }
}

fn rejected() -> SendOutcome {
    SendOutcome::Rejected {
        reason: "payload refused by fixture policy".into(),
        retryable_after_fix: true,
    }
}

fn inbox_run(id: &str) -> Run {
    serde_json::from_value(json!({
        "id": id, "agent": "fixture", "worktree": "/fixture/w", "branch": "corral/T",
        "base": "main", "base_sha": "abc", "started": "2026-10-25T00:00:00Z",
        "delivery": "inbox"
    }))
    .unwrap()
}

struct World {
    _tmp: tempfile::TempDir,
    ctx: Ctx,
    /// A coding run and its reviewer, then a planner and its plan reviewer.
    pairs: [[Recipient; 2]; 2],
}

fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let mut store = Store::init(tmp.path()).unwrap();
    store
        .edit_config(|doc| doc.set_str("projects.app.path", "."))
        .unwrap();
    store.reload_config_if_changed().unwrap();
    let code = store
        .create("Code", Some("app"), Status::Running, "")
        .unwrap();
    store
        .update(&code.id, |task| {
            let mut run = inbox_run("R1");
            run.review_runs.push(inbox_run("R1-rv1"));
            task.runs.push(run);
            Ok(())
        })
        .unwrap();
    let ctx = Ctx::from_store(store);
    let planned = ctx
        .store
        .create("Plan", Some("app"), Status::Ready, "Spin.")
        .unwrap();
    let path = tmp.path().join("plan.md");
    std::fs::write(&path, PLAN).unwrap();
    let plan = corral_ops::plan_submit(&ctx, &planned.id, &path).unwrap();
    ctx.store
        .update(&planned.id, |task| {
            task.plan_runs.push(corral_core::PlanRun {
                agent_host: None,
                execution: None,
                route: None,
                delivery: corral_core::inbox::DeliveryMode::Inbox,
                inbox: Default::default(),
                id: "PL1".into(),
                profile: "fixture".into(),
                harness: "fixture".into(),
                model: None,
                vendor: None,
                effort: None,
                spec_revision: "fixture".into(),
                started: jiff::Timestamp::now(),
                ended: None,
                herdr: None,
                output_dir: "plan-output".into(),
                submitted: None,
                rounds: 0,
                state: corral_core::PlanRunState::Planning,
                pending_reply: None,
            });
            task.plans
                .last_mut()
                .unwrap()
                .reviews
                .push(corral_core::PlanReview {
                    agent_host: None,
                    delivery: corral_core::inbox::DeliveryMode::Inbox,
                    inbox: Default::default(),
                    route: None,
                    id: "PR1".into(),
                    reviewer: "fixture".into(),
                    policy_digest: "fixture".into(),
                    verdict: None,
                    notes: None,
                    at: jiff::Timestamp::now(),
                    herdr: None,
                    pending_reply: None,
                });
            Ok(())
        })
        .unwrap();
    let pairs = [
        [
            Recipient::Run {
                task: code.id.clone(),
                run: "R1".into(),
            },
            Recipient::ReviewRun {
                task: code.id.clone(),
                run: "R1".into(),
                review: "R1-rv1".into(),
            },
        ],
        [
            Recipient::PlanRun {
                task: planned.id.clone(),
                plan_run: "PL1".into(),
            },
            Recipient::PlanReview {
                task: planned.id,
                plan: plan.id,
                review: "PR1".into(),
            },
        ],
    ];
    World {
        _tmp: tmp,
        ctx,
        pairs,
    }
}

impl World {
    fn recipients(&self) -> Vec<(Recipient, Recipient)> {
        self.pairs
            .iter()
            .flat_map(|[a, b]| [(a.clone(), b.clone()), (b.clone(), a.clone())])
            .collect()
    }
    fn inbox(&self, recipient: &Recipient) -> Inbox<'_> {
        Inbox::new(&self.ctx.store, recipient.clone()).unwrap()
    }
    fn post(&self, recipient: &Recipient, body: &str) -> Message {
        self.inbox(recipient)
            .post(Post {
                kind: "owner_reply",
                urgency: Urgency::Wake,
                author: "owner",
                body,
                binding: MessageBinding {
                    run: Some("R1".into()),
                    review_round: Some(2),
                    ..Default::default()
                },
                subject: Some(json!({"finding": 3})),
                max_payload: MAX_PAYLOAD,
            })
            .unwrap()
    }
    fn events(&self, kind: &str) -> Vec<Value> {
        std::fs::read_to_string(self.ctx.store.dir().join("events.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|event| event["kind"] == kind)
            .collect()
    }
    fn end(&self, recipient: &Recipient) {
        let id = recipient.task().unwrap().to_string();
        self.ctx
            .store
            .update(&id, |task| {
                match recipient {
                    Recipient::Run { .. } => {
                        task.runs[0].outcome = Some(corral_core::RunOutcome::Superseded)
                    }
                    Recipient::ReviewRun { .. } => {
                        task.runs[0].review_runs[0].outcome = Some(corral_core::RunOutcome::Done);
                    }
                    Recipient::PlanRun { .. } => {
                        task.plan_runs[0].ended = Some(jiff::Timestamp::now())
                    }
                    Recipient::PlanReview { .. } => {
                        task.plans[0].reviews[0].verdict = Some(Verdict::Approve);
                    }
                    Recipient::Pm | Recipient::Assistant { .. } => unreachable!(),
                }
                Ok(())
            })
            .unwrap();
    }
}

#[test]
fn a_definitive_retryable_rejection_is_retried_once_for_exactly_its_recipient() {
    for (recipient, sibling) in world().recipients() {
        // Given a coexisting sibling and an actual begin_attempt -> Rejected -> fail delivery.
        let world = world();
        let transport = Transport::new(rejected());
        let original = world.post(&recipient, "fix the second finding");
        assert_eq!(
            deliver(&world.ctx, &recipient, &transport, None).unwrap(),
            DeliveryStatus::Sent { envelope: 1 }
        );
        let inbox = world.inbox(&recipient);
        assert_eq!(
            inbox.terminal(original.seq).unwrap(),
            Some(TerminalState::Failed)
        );
        assert_eq!(inbox.envelope(1).unwrap().attempts.len(), 1);

        // When the cause is fixed and the owner retries the exact message ID.
        transport.set(SendOutcome::Accepted);
        let retried =
            corral_ops::delivery::retry(&world.ctx.store, &recipient, original.seq).unwrap();

        // Then one new message preserves body, kind, binding and subject on that recipient only.
        assert_eq!(retried.recipient, recipient);
        assert_ne!(retried.id, original.id);
        assert_eq!(
            (retried.kind.as_str(), &retried.binding, &retried.subject),
            (original.kind.as_str(), &original.binding, &original.subject)
        );
        assert_eq!(
            retried.body.split_once('\n').unwrap().1,
            original.body.split_once('\n').unwrap().1
        );
        let retries = world.events("message_retried");
        assert_eq!(retries.len(), 1);
        assert_eq!(retries[0]["detail"]["from"], original.id.as_str());
        assert_eq!(retries[0]["detail"]["to"], retried.id.as_str());
        assert!(world.inbox(&sibling).messages().unwrap().is_empty());

        // And the next delivery pass sends it to that recipient, never the sibling.
        assert_eq!(
            deliver(&world.ctx, &recipient, &transport, None).unwrap(),
            DeliveryStatus::Sent { envelope: 2 }
        );
        assert_eq!(inbox.envelope(2).unwrap().members, vec![retried.seq]);
        assert_eq!(transport.sent_to(&recipient), 2);
        assert_eq!(transport.sent_to(&sibling), 0);
        let error = corral_ops::delivery::retry(&world.ctx.store, &recipient, original.seq)
            .map(|_| ())
            .unwrap_err();
        assert!(
            format!("{error:#}").contains("retry_refused: already_retried"),
            "{error:#}"
        );
    }
}

#[test]
fn an_ended_recipient_refuses_retry_without_posting() {
    for (recipient, _) in world().recipients() {
        // Given a definitively rejected message whose container then ended.
        let world = world();
        let transport = Transport::new(rejected());
        let original = world.post(&recipient, "late");
        deliver(&world.ctx, &recipient, &transport, None).unwrap();
        world.end(&recipient);
        let before = world.inbox(&recipient).messages().unwrap();

        // When retrying it.
        let error = corral_ops::delivery::retry(&world.ctx.store, &recipient, original.seq)
            .map(|_| ())
            .unwrap_err();

        // Then the refusal names the ended recipient and an inspect disposition, and posts nothing.
        let text = format!("{error:#}");
        assert!(text.contains("retry_refused: recipient_ended"), "{text}");
        assert!(text.contains("corral inbox show"), "{text}");
        assert!(!text.contains("inbox recover"), "{text}");
        assert_eq!(world.inbox(&recipient).messages().unwrap(), before);
        assert!(world.events("message_retried").is_empty());
    }
}

#[test]
fn installed_recovery_choices_act_on_the_exact_recipient_and_unavailable_ones_change_nothing() {
    for (recipient, sibling) in world().recipients() {
        let world = world();
        let transport = Transport::new(SendOutcome::Uncertain {
            reason: "transport timeout".into(),
        });
        let message = world.post(&recipient, "maybe delivered");
        world.post(&sibling, "sibling message");
        assert_eq!(
            deliver(&world.ctx, &recipient, &transport, None).unwrap(),
            DeliveryStatus::Sent { envelope: 1 }
        );
        let inbox = world.inbox(&recipient);
        let sibling_state = world.inbox(&sibling).state().unwrap();

        // Given an open uncertain envelope, the uninstalled terminal adapter's choices refuse.
        let state = inbox.state().unwrap();
        for recovery in [Recovery::Submit, Recovery::Discard] {
            let error = inbox.recover(recovery).unwrap_err().to_string();
            assert!(
                error.starts_with("terminal_adapter_unavailable:"),
                "{error}"
            );
            assert_eq!(inbox.state().unwrap(), state);
        }

        // When the owner resends, then a new frozen envelope carries the same member.
        inbox.recover(Recovery::Resend).unwrap();
        let recovered = world.events("inbox_recovered");
        assert_eq!(
            recovered.last().unwrap()["detail"]["duplicate_possible"],
            true
        );
        transport.set(SendOutcome::Uncertain {
            reason: "transport timeout again".into(),
        });
        assert_eq!(
            deliver(&world.ctx, &recipient, &transport, None).unwrap(),
            DeliveryStatus::Sent { envelope: 2 }
        );
        assert_eq!(inbox.envelope(2).unwrap().members, vec![message.seq]);

        // And abandoning settles it without any post or send.
        let messages = inbox.messages().unwrap();
        inbox.recover(Recovery::Abandon).unwrap();
        assert_eq!(
            inbox.terminal(message.seq).unwrap(),
            Some(TerminalState::Abandoned)
        );
        assert_eq!(inbox.messages().unwrap(), messages);
        assert_eq!(transport.sent_to(&recipient), 2);

        // The abandoned uncertain history never becomes a safe retry.
        let error = corral_ops::delivery::retry(&world.ctx.store, &recipient, message.seq)
            .map(|_| ())
            .unwrap_err();
        assert!(
            format!("{error:#}").contains("uncertain_prior_attempt"),
            "{error:#}"
        );
        assert_eq!(world.inbox(&sibling).state().unwrap(), sibling_state);
        assert_eq!(transport.sent_to(&sibling), 0);
    }
}

#[test]
fn every_recipient_retains_terminal_and_observed_ninth_entries_across_fifo_restart() {
    for (recipient, sibling) in world().recipients() {
        for observed in [false, true] {
            // Given sixteen real publications, alternating post_with and reserve_in.
            let world = world();
            let inbox = world.inbox(&recipient);
            for index in 1..=16 {
                let request = Post {
                    kind: "owner_note",
                    urgency: Urgency::Wake,
                    author: "owner",
                    body: "finding",
                    binding: MessageBinding::default(),
                    subject: None,
                    max_payload: MAX_PAYLOAD,
                };
                if index % 2 == 0 {
                    let _posting = inbox.lock().unwrap();
                    world
                        .ctx
                        .store
                        .update(recipient.task().unwrap(), |task| {
                            inbox.reserve_in(task, request)?;
                            Ok(())
                        })
                        .unwrap();
                } else {
                    inbox.post_with(request, |_, _| Ok(())).unwrap();
                }
            }
            let receipt = Receipt {
                envelope: None,
                binding: None,
                observed_at: "2026-10-25T00:00:00Z".parse().unwrap(),
                via: "fixture".into(),
                detail: json!({}),
            };
            for seq in 1..=16 {
                if seq == 9 && observed {
                    inbox
                        .record_receipt(seq, ReceiptLevel::Observed, &receipt)
                        .unwrap();
                } else {
                    inbox
                        .mark_terminal(seq, TerminalState::Cancelled, "fixture", "fixture")
                        .unwrap();
                }
            }
            let original = inbox.owner_entries(&inbox.state().unwrap()).unwrap()[8].clone();
            assert_eq!(original.message.seq, 9);
            assert_eq!(original.observed.is_some(), observed);
            assert!(!inbox.has_pending_delivery(true).unwrap());
            // When a later real observation supersedes the eligible first entries and restarts.
            for seq in 1..=8 {
                inbox
                    .record_receipt(seq, ReceiptLevel::Observed, &receipt)
                    .unwrap();
            }
            let reopened = Store::open(world._tmp.path()).unwrap();
            let restarted = Inbox::new(&reopened, recipient.clone()).unwrap();
            restarted
                .maintain_owner_header(receipt.observed_at)
                .unwrap();
            let state = restarted.state().unwrap();
            let ninth = restarted
                .owner_entries(&state)
                .unwrap()
                .into_iter()
                .find(|entry| entry.message.seq == 9)
                .unwrap();
            // Then both outcome variants retain exact identity, age and proof after promotion.
            assert_eq!(ninth, original);
            assert!(
                state
                    .owner_header
                    .unwrap()
                    .entries
                    .iter()
                    .any(|entry| entry.seq == 9)
            );
            assert!(world.inbox(&sibling).messages().unwrap().is_empty());
        }
    }
}

#[test]
fn cumulative_retry_proof_survives_real_delivery_resend_overflow_and_promotion() {
    let earlier = [
        None,
        Some(SendOutcome::Uncertain {
            reason: "timeout".into(),
        }),
        Some(SendOutcome::Conflicted {
            reason: "key conflict".into(),
        }),
        Some(SendOutcome::Accepted),
        Some(SendOutcome::Admitted { detail: json!({}) }),
        Some(SendOutcome::Held {
            reason: "held".into(),
        }),
    ];
    for (recipient, sibling) in world().recipients() {
        // A: first rejection; B: unsafe E1 -> owner resend -> rejected E2;
        // C: unattempted E1 replacement -> rejected E2; D: unresolved E1.
        for variant in 0..=7 {
            let world = world();
            let inbox = world.inbox(&recipient);
            for _ in 0..8 {
                let message = world.post(&recipient, "older");
                inbox
                    .mark_terminal(message.seq, TerminalState::Cancelled, "fixture", "fixture")
                    .unwrap();
            }
            let message = world.post(&recipient, "ninth");
            assert_eq!(message.seq, 9);
            let transport = Transport::new(rejected());
            let safe = matches!(variant, 0 | 7);
            match variant {
                0 => {}
                7 => {
                    let envelope = inbox
                        .publish(
                            &SessionBinding {
                                session: recipient.key().unwrap(),
                                endpoint: None,
                                transcript: None,
                            },
                            "fixture",
                            MAX_PAYLOAD,
                            true,
                        )
                        .unwrap()
                        .unwrap();
                    inbox.retire_unattempted(&envelope).unwrap();
                }
                1..=6 => {
                    let outcome = earlier[variant - 1].clone();
                    if let Some(outcome) = outcome {
                        transport.set(outcome);
                        deliver(&world.ctx, &recipient, &transport, None).unwrap();
                    } else {
                        let mut envelope = inbox
                            .publish(
                                &SessionBinding {
                                    session: recipient.key().unwrap(),
                                    endpoint: None,
                                    transcript: None,
                                },
                                "fixture",
                                MAX_PAYLOAD,
                                true,
                            )
                            .unwrap()
                            .unwrap();
                        inbox
                            .begin_attempt(&mut envelope, "2026-10-25T00:00:00Z".parse().unwrap())
                            .unwrap();
                    }
                    inbox.recover(Recovery::Resend).unwrap();
                }
                _ => unreachable!(),
            }
            transport.set(rejected());
            deliver(&world.ctx, &recipient, &transport, None).unwrap();
            let before = inbox
                .state()
                .unwrap()
                .owner_header
                .unwrap()
                .overflow_entries[0]
                .clone();
            assert_eq!(
                before.retry_evidence.permits(TerminalState::Failed),
                safe,
                "variant {variant}"
            );
            if safe {
                assert_eq!(before.retry_evidence.coverage, RetryCoverage::Complete);
                assert!(matches!(
                    before.retry_evidence.history,
                    RetryHistory::RejectedOnly { .. }
                ));
            }
            // When the store restarts before and after promotion.
            let reopened = Store::open(world._tmp.path()).unwrap();
            let restarted = Inbox::new(&reopened, recipient.clone()).unwrap();
            let receipt = Receipt {
                envelope: None,
                binding: None,
                observed_at: "2026-10-25T00:00:00Z".parse().unwrap(),
                via: "fixture".into(),
                detail: json!({}),
            };
            for seq in 1..=8 {
                restarted
                    .record_receipt(seq, ReceiptLevel::Observed, &receipt)
                    .unwrap();
            }
            restarted
                .maintain_owner_header(receipt.observed_at)
                .unwrap();
            let again = Store::open(world._tmp.path()).unwrap();
            let promoted = Inbox::new(&again, recipient.clone()).unwrap();
            let entry = promoted
                .state()
                .unwrap()
                .owner_header
                .unwrap()
                .entries
                .into_iter()
                .find(|entry| entry.seq == 9)
                .unwrap();
            assert_eq!(entry, before);
            // Then exact inspection and the actual retry guard agree, never posting to a sibling.
            assert_eq!(
                promoted.inspect_retry(9).unwrap().eligible,
                safe,
                "variant {variant}"
            );
            let result = corral_ops::delivery::retry(&again, &recipient, 9);
            assert_eq!(result.is_ok(), safe, "variant {variant}: {result:?}");
            assert!(world.inbox(&sibling).messages().unwrap().is_empty());
        }
    }
}

#[test]
fn supersession_never_discards_rewritten_retry_proof_for_any_recipient_or_array() {
    for position in [1, 9] {
        for maintenance in ["explicit", "post", "reserve", "index"] {
            for publication in ["successful", "admitted", "observed", "generic"] {
                let world = world();
                for (recipient, sibling) in world.recipients() {
                    // Given an accepted primary or overflow owner message with truncated evidence.
                    let inbox = world.inbox(&recipient);
                    let sibling_before = world.inbox(&sibling).messages().unwrap();
                    for _ in 0..position {
                        world.post(&recipient, "must not replay");
                    }
                    let mut envelope = inbox
                        .publish(
                            &SessionBinding {
                                session: recipient.key().unwrap(),
                                endpoint: None,
                                transcript: None,
                            },
                            "fixture",
                            MAX_PAYLOAD,
                            true,
                        )
                        .unwrap()
                        .unwrap();
                    let now = "2026-10-25T00:00:00Z".parse().unwrap();
                    let receipt = Receipt {
                        envelope: None,
                        binding: None,
                        observed_at: now,
                        via: "fixture".into(),
                        detail: json!({}),
                    };
                    inbox.begin_attempt(&mut envelope, now).unwrap();
                    inbox
                        .finish_attempt(
                            &mut envelope,
                            if publication == "successful" {
                                SendOutcome::Accepted
                            } else {
                                rejected()
                            },
                        )
                        .unwrap();
                    if publication == "successful" {
                        envelope.outcome = None;
                        envelope.attempts.clear();
                        envelope.stages.clear();
                        envelope.retry_bar = None;
                        inbox.save_envelope(&envelope).unwrap();
                        inbox.save_envelope(&envelope).unwrap();
                    } else {
                        // Block proof publication after the task bar, without permission or timing assumptions.
                        let path = inbox.dir().join(format!("{position:06}.failed.json"));
                        std::fs::create_dir(&path).unwrap();
                        let result = match publication {
                            "admitted" => inbox
                                .record_receipt(position, ReceiptLevel::Admitted, &receipt)
                                .map(|_| ()),
                            "observed" => inbox
                                .record_receipt(position, ReceiptLevel::Observed, &receipt)
                                .map(|_| ()),
                            "generic" => {
                                envelope.outcome = Some(SendOutcome::Accepted);
                                inbox.save_envelope(&envelope)
                            }
                            _ => unreachable!(),
                        };
                        assert!(result.is_err(), "{publication}");
                        std::fs::remove_dir(&path).unwrap();
                    }
                    inbox
                        .mark_terminal(position, TerminalState::Failed, "fixture", "test")
                        .unwrap();
                    assert!(!inbox.inspect_retry(position).unwrap().eligible);
                    let later = world.post(&recipient, "supersedes settled obligation");
                    inbox
                        .record_receipt(later.seq, ReceiptLevel::Observed, &receipt)
                        .unwrap();
                    // When any supported bounded maintenance seam prunes it and the store reopens.
                    match maintenance {
                        "explicit" => {
                            inbox.maintain_owner_header(now).unwrap();
                        }
                        "post" => {
                            world.post(&recipient, "admission");
                        }
                        "reserve" => {
                            world
                                .ctx
                                .store
                                .update(recipient.task().unwrap(), |task| {
                                    inbox.reserve_in(
                                        task,
                                        Post {
                                            kind: "owner_note",
                                            urgency: Urgency::Wake,
                                            author: "owner",
                                            body: "admission",
                                            binding: MessageBinding::default(),
                                            subject: None,
                                            max_payload: MAX_PAYLOAD,
                                        },
                                    )?;
                                    Ok(())
                                })
                                .unwrap();
                        }
                        "index" => {
                            inbox.index_owner(64).unwrap();
                        }
                        _ => unreachable!(),
                    }
                    let reopened = Store::open(world._tmp.path()).unwrap();
                    let restarted = Inbox::new(&reopened, recipient.clone()).unwrap();
                    let state = restarted.state().unwrap();
                    assert!(
                        !state
                            .owner_header
                            .as_ref()
                            .unwrap()
                            .iter()
                            .any(|entry| entry.seq == position)
                    );
                    // Then exact inspection and actual retry still refuse without posting elsewhere.
                    let inspection = restarted.inspect_retry(position).unwrap();
                    assert!(
                        !inspection.eligible,
                        "{recipient:?} {position} {maintenance}"
                    );
                    assert_eq!(inspection.reason_code, "retained_bar");
                    assert_eq!(
                        inspection.witness.unwrap().reason,
                        match publication {
                            "successful" => corral_core::inbox::WitnessReason::Accepted,
                            "admitted" => corral_core::inbox::WitnessReason::Admitted,
                            "observed" => corral_core::inbox::WitnessReason::Observed,
                            "generic" => corral_core::inbox::WitnessReason::UntrackedMutation,
                            _ => unreachable!(),
                        }
                    );
                    assert!(corral_ops::delivery::retry(&reopened, &recipient, position).is_err());
                    assert_eq!(restarted.state().unwrap(), state);
                    assert_eq!(world.inbox(&sibling).messages().unwrap(), sibling_before);
                }
            }
        }
    }
}
