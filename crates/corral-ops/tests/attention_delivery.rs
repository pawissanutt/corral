//! Recipient-bound delivery recovery (T-426): actual begin_attempt -> Rejected -> fail on every
//! task recipient kind, exact-ID retry, and the installed open-envelope recovery choices.
use std::cell::RefCell;

use corral_core::inbox::{
    Inbox, MAX_PAYLOAD, Message, MessageBinding, Post, Recipient, Recovery, TerminalState, Urgency,
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
                    Recipient::Pm => unreachable!(),
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
