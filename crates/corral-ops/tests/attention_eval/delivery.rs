//! S12, S20 and S21: recipient-bound delivery conditions, exact-ID retry and action joins.
use std::cell::RefCell;

use corral_core::inbox::{
    Inbox, MAX_PAYLOAD, Message, MessageBinding, Post, Recipient, SendOutcome, TerminalState,
    Urgency,
};
use corral_core::{Run, Status};
use corral_ops::Ctx;
use corral_ops::attention;
use corral_ops::delivery::{
    BindOutcome, Capabilities, DeliveryAdapter, Observation, Readiness, RecipientRef,
    SessionBinding, deliver,
};
use serde_json::json;

use crate::fixture::{Fx, at};
use crate::support::{invariants, stats, sync};

const PLAN: &str = "---\ngoal: Spin\nnon_goals: []\ndecisions: []\naffected_paths: [w.rs]\ncontracts: []\ndependencies: []\nacceptance:\n  - criterion: C1\n    scenario: spin\n    expected: clockwise\nverification: [cargo test]\nowner_decisions: []\n---\nDetails.\n";

pub(super) struct Transport {
    outcome: RefCell<SendOutcome>,
    sent: RefCell<Vec<String>>,
    pub(super) idempotent_retry: bool,
}

impl Transport {
    pub(super) fn new(outcome: SendOutcome) -> Self {
        Self {
            outcome: RefCell::new(outcome),
            sent: RefCell::new(Vec::new()),
            idempotent_retry: false,
        }
    }
    pub(super) fn set(&self, outcome: SendOutcome) {
        *self.outcome.borrow_mut() = outcome;
    }
    pub(super) fn sent_to(&self, recipient: &Recipient) -> usize {
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
            idempotent_retry: self.idempotent_retry,
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

pub(super) fn rejected() -> SendOutcome {
    SendOutcome::Rejected {
        reason: "payload refused by fixture policy".into(),
        retryable_after_fix: true,
    }
}

fn inbox_run(id: &str) -> Run {
    serde_json::from_value(json!({
        "id": id, "agent": "fixture", "worktree": "/fixture/w", "branch": "corral/T",
        "base": "main", "base_sha": "abc", "started": at(0), "delivery": "inbox"
    }))
    .unwrap()
}

/// A coding run, its reviewer, a planner and its plan reviewer on the fake-engine store.
pub(super) fn world() -> (Fx, [Recipient; 4]) {
    let fx = Fx::new();
    let store = &fx.ctx.store;
    store
        .edit_config(|doc| doc.set_str("projects.app.path", "."))
        .unwrap();
    let ctx = fx.reopen();
    // Every saved pane belongs to this fake client, including after reopening the store.
    let pane = corral_core::HerdrRef {
        workspace_id: "worker".into(),
        pane_id: "worker:p1".into(),
        socket: Some(ctx.herdr.socket_path().unwrap()),
    };
    let code = ctx
        .store
        .create("Code", Some("app"), Status::Running, "")
        .unwrap();
    ctx.store
        .update(&code.id, |task| {
            let mut run = inbox_run("R1");
            run.review_runs.push(inbox_run("R1-rv1"));
            task.runs.push(run);
            Ok(())
        })
        .unwrap();
    let planned = ctx
        .store
        .create("Plan", Some("app"), Status::Ready, "Spin.")
        .unwrap();
    let path = fx.tmp.path().join("plan.md");
    std::fs::write(&path, PLAN).unwrap();
    let plan = corral_ops::plan_submit(&ctx, &planned.id, &path).unwrap();
    ctx.store
        .update(&planned.id, |task| {
            task.plan_runs.push(corral_core::PlanRun {
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
                started: at(0),
                ended: None,
                herdr: Some(pane.clone()),
                output_dir: fx.tmp.path().join("plan-output"),
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
                    delivery: corral_core::inbox::DeliveryMode::Inbox,
                    inbox: Default::default(),
                    route: None,
                    id: "PR1".into(),
                    reviewer: "fixture".into(),
                    policy_digest: "fixture".into(),
                    verdict: None,
                    notes: None,
                    at: at(0),
                    herdr: Some(pane.clone()),
                    pending_reply: None,
                });
            Ok(())
        })
        .unwrap();
    let recipients = [
        Recipient::Run {
            task: code.id.clone(),
            run: "R1".into(),
        },
        Recipient::ReviewRun {
            task: code.id.clone(),
            run: "R1".into(),
            review: "R1-rv1".into(),
        },
        Recipient::PlanRun {
            task: planned.id.clone(),
            plan_run: "PL1".into(),
        },
        Recipient::PlanReview {
            task: planned.id,
            plan: plan.id,
            review: "PR1".into(),
        },
    ];
    (fx, recipients)
}

pub(super) fn post(ctx: &Ctx, recipient: &Recipient, body: &str) -> Message {
    Inbox::new(&ctx.store, recipient.clone())
        .unwrap()
        .post(Post {
            kind: "owner_reply",
            urgency: Urgency::Wake,
            author: "owner",
            body,
            binding: MessageBinding {
                run: Some("R1".into()),
                ..Default::default()
            },
            subject: None,
            max_payload: MAX_PAYLOAD,
        })
        .unwrap()
}

pub(super) fn condition(
    fx: &Fx,
    recipient: &Recipient,
    message: &Message,
) -> corral_core::attention::Condition {
    let task = recipient.task().unwrap().to_string();
    attention::board(&fx.reopen(), at(0))
        .unwrap()
        .tasks
        .into_iter()
        .find(|row| row.task == task)
        .unwrap()
        .assessment
        .conditions
        .into_iter()
        .find(|c| {
            c.kind.starts_with("delivery_")
                && c.key.as_ref().is_some_and(|k| k.0.contains(&message.id))
        })
        .unwrap_or_else(|| panic!("no delivery condition for {}", message.id))
}

#[test]
fn s12_t128_cancel_sequence_fenced_delivers_and_unfenced_replay_retries_its_recipient() {
    // Given the T-128 sequence: an owner post, then a changes request on the same run.
    let (fx, [run, ..]) = world();
    let ctx = fx.reopen();
    let transport = Transport::new(SendOutcome::Accepted);
    // When the retire fence holds, the post is delivered and no delivery condition opens.
    let fenced = post(&ctx, &run, "Original instruction");
    deliver(&ctx, &run, &transport, None).unwrap();
    sync(&fx, 0);
    assert_eq!(transport.sent_to(&run), 1);
    assert!(
        attention::board(&fx.reopen(), at(0))
            .unwrap()
            .tasks
            .iter()
            .flat_map(|row| &row.assessment.conditions)
            .filter(|c| c.key.as_ref().is_some_and(|k| k.0.contains(&fenced.id)))
            .all(|c| c.kind != "delivery_cancelled")
    );
    // When the unfenced replay cancels the post before any attempt.
    let (fx, [run, ..]) = world();
    let ctx = fx.reopen();
    let replay = post(&ctx, &run, "Original instruction");
    Inbox::new(&ctx.store, run.clone())
        .unwrap()
        .mark_terminal(
            replay.seq,
            TerminalState::Cancelled,
            "T-128-style stale retire",
            "fixture",
        )
        .unwrap();
    sync(&fx, 0);
    // Then one pass shows a cancelled-never-attempted condition with an exact-ID retry.
    let cancelled = condition(&fx, &run, &replay);
    assert_eq!(cancelled.kind, "delivery_cancelled");
    assert_eq!(
        cancelled.next_action.command.as_deref(),
        Some(format!("corral inbox retry {}", replay.id).as_str())
    );
    let retried = corral_ops::delivery::retry(&ctx.store, &run, replay.seq).unwrap();
    deliver(&ctx, &run, &transport, None).unwrap();
    assert_eq!(retried.recipient, run);
    assert_eq!(
        transport.sent_to(&run),
        2,
        "the retry reached the original recipient"
    );
    sync(&fx, 60);
    let stats = stats(&fx, 60);
    let key = cancelled.key.unwrap().0;
    assert!(stats.conditions.contains_key(&key));
    assert_eq!(stats.actions.by_condition.get(&key), Some(&1));
    invariants(&stats);
}

#[test]
fn s20_retry_by_id_is_attributed_and_a_fresh_reply_over_two_conditions_is_ambiguous() {
    // Given two definitively rejected messages on one recipient, both recorded by a pass.
    let (fx, [run, ..]) = world();
    let ctx = fx.reopen();
    let transport = Transport::new(rejected());
    let first = post(&ctx, &run, "first");
    deliver(&ctx, &run, &transport, None).unwrap();
    let second = post(&ctx, &run, "second");
    deliver(&ctx, &run, &transport, None).unwrap();
    sync(&fx, 0);
    let first_key = condition(&fx, &run, &first).key.unwrap().0;
    let second_key = condition(&fx, &run, &second).key.unwrap().0;
    // When the PM retries the first by ID, then the owner posts a fresh reply to the run.
    corral_ops::delivery::retry(&ctx.store, &run, first.seq).unwrap();
    post(&ctx, &run, "fresh reply");
    transport.set(SendOutcome::Accepted);
    deliver(&ctx, &run, &transport, None).unwrap();
    sync(&fx, 60);
    // Then the retry joins its own condition, the reply is ambiguous, and nothing duplicates.
    let stats = stats(&fx, 60);
    // The second setup post is itself a fresh post on the recipient while the first
    // condition (canonically open since its message's creation) is open, so it joins it too.
    assert_eq!(stats.actions.by_condition.get(&first_key), Some(&2));
    assert_eq!(stats.actions.by_condition.get(&second_key), None);
    assert_eq!(stats.actions.attributed, 2);
    assert_eq!(stats.actions.ambiguous, 1);
    assert_eq!(stats.actions.duplicates, 0);
    invariants(&stats);
}

#[test]
fn s21_safe_rejections_retry_exactly_and_uncertain_evidence_sends_nothing_on_all_recipients() {
    let (_, recipients) = world();
    for (index, _) in recipients.iter().enumerate() {
        // Given a first definitive rejection on this recipient.
        let (fx, recipients) = world();
        let recipient = &recipients[index];
        let pass = |seconds| sync(&fx, seconds);
        let ctx = fx.reopen();
        let transport = Transport::new(rejected());
        let safe = post(&ctx, recipient, "safe");
        deliver(&ctx, recipient, &transport, None).unwrap();
        pass(0);
        let safe_row = condition(&fx, recipient, &safe);
        // When the board restarts, then its key and age are unchanged.
        pass(30);
        let again = condition(&fx, recipient, &safe);
        assert_eq!(
            (again.key.clone(), again.since),
            (safe_row.key.clone(), safe_row.since)
        );
        // Then it offers exact-ID retry, and executing it sends exactly once more.
        assert_eq!(safe_row.kind, "delivery_failed", "{recipient:?}");
        assert_eq!(
            safe_row.next_action.command.as_deref(),
            Some(format!("corral inbox retry {}", safe.id).as_str())
        );
        transport.set(SendOutcome::Accepted);
        corral_ops::delivery::retry(&ctx.store, recipient, safe.seq).unwrap();
        deliver(&ctx, recipient, &transport, None).unwrap();
        assert_eq!(transport.sent_to(recipient), 2, "{recipient:?}");
        for sibling in recipients.iter().filter(|other| *other != recipient) {
            assert_eq!(transport.sent_to(sibling), 0);
        }
        pass(60);
        let stats = stats(&fx, 60);
        let safe_key = safe_row.key.unwrap().0;
        assert_eq!(stats.conditions[&safe_key].kind, "delivery_failed");
        assert_eq!(stats.actions.by_condition.get(&safe_key), Some(&1));
        assert_eq!(stats.actions.attributed, 1);
        invariants(&stats);

        // Given an uncertain first attempt on a fresh copy of the same recipient.
        let (fx, recipients) = world();
        let recipient = &recipients[index];
        let pass = |seconds| sync(&fx, seconds);
        let ctx = fx.reopen();
        let transport = Transport::new(SendOutcome::Uncertain {
            reason: "transport lost the acknowledgement".into(),
        });
        let unsafe_message = post(&ctx, recipient, "uncertain");
        deliver(&ctx, recipient, &transport, None).unwrap();
        pass(0);
        let unsafe_row = condition(&fx, recipient, &unsafe_message);
        // Then it offers owner recovery or inspection, never retry, and retry sends nothing.
        assert_eq!(unsafe_row.kind, "delivery_uncertain", "{recipient:?}");
        let offered = unsafe_row.next_action.command.clone().unwrap_or_default();
        assert!(!offered.contains("inbox retry"), "{offered}");
        assert!(unsafe_row.next_action.requires_owner || offered.contains("inspect-retry"));
        assert!(corral_ops::delivery::retry(&ctx.store, recipient, unsafe_message.seq).is_err());
        assert_eq!(
            transport.sent_to(recipient),
            1,
            "uncertain evidence sends nothing"
        );
        pass(60);
        let stats = crate::support::stats(&fx, 60);
        assert_eq!(
            stats.conditions[&unsafe_row.key.unwrap().0].kind,
            "delivery_uncertain"
        );
        assert_eq!(stats.actions.attributed, 0);
        invariants(&stats);
    }
}
