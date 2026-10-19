use std::cell::{Cell, RefCell};

use corral_core::inbox::*;
use corral_core::{Run, Store};
use serde_json::json;

use super::*;

struct Handler {
    fail: Cell<bool>,
    receipts: RefCell<Vec<Receipt>>,
    task_calls: Cell<usize>,
}
impl Handler {
    fn new() -> Self {
        Self {
            fail: Cell::new(false),
            receipts: RefCell::new(Vec::new()),
            task_calls: Cell::new(0),
        }
    }
}
impl EffectHandler for Handler {
    fn prepare_decision(
        &self,
        _: &Message,
        _: &Receipt,
    ) -> Result<Option<corral_core::ResumeReceipt>> {
        Ok(None)
    }
    fn apply_locked(
        &self,
        _: &Ctx,
        _: &Message,
        receipt: &Receipt,
        _: Option<&corral_core::ResumeReceipt>,
    ) -> Result<()> {
        self.receipts.borrow_mut().push(receipt.clone());
        if self.fail.replace(false) {
            anyhow::bail!("simulated effect interruption");
        }
        Ok(())
    }
    fn apply_task(&self, task: &mut Task, _: &Message, _: &Receipt) -> Result<()> {
        self.task_calls.set(self.task_calls.get() + 1);
        task.labels.push("effect".into());
        Ok(())
    }
}

fn fixture() -> (tempfile::TempDir, Ctx, Recipient) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::init(dir.path()).unwrap();
    let task = store
        .create("effect fixture", None, Status::NeedsMe, "")
        .unwrap();
    let run: Run = serde_json::from_value(json!({
        "id":"R1","agent":"fake","worktree":dir.path(),"branch":"fixture","base":"main",
        "base_sha":"abc","started":"2026-10-19T00:00:00Z","seen_working":true,
        "blocked_excerpt":"blocked","pending_reply":"reply","idle_since":"2026-10-19T00:00:00Z",
        "idle_no_report":true
    }))
    .unwrap();
    store
        .update(&task.id, |t| {
            t.runs.push(run);
            Ok(())
        })
        .unwrap();
    (
        dir,
        Ctx::from_store(store),
        Recipient::Run {
            task: task.id,
            run: "R1".into(),
        },
    )
}

fn observed(ctx: &Ctx, recipient: &Recipient, binding: MessageBinding) -> u64 {
    let inbox = Inbox::new(&ctx.store, recipient.clone()).unwrap();
    let message = inbox
        .post(Post {
            kind: "owner_reply",
            urgency: Urgency::Wake,
            author: "owner",
            body: "reply",
            binding,
            subject: None,
            max_payload: MAX_PAYLOAD,
        })
        .unwrap();
    inbox
        .record_receipt(
            message.seq,
            ReceiptLevel::Observed,
            &Receipt {
                envelope: None,
                binding: None,
                observed_at: "2026-10-19T00:01:00Z".parse().unwrap(),
                via: "manual".into(),
                detail: json!({"reason":"fixture"}),
            },
        )
        .unwrap();
    message.seq
}

#[test]
fn reconciler_applies_only_observed_effects_and_persists_task_view_once() {
    let (_dir, ctx, recipient) = fixture();
    let seq = observed(&ctx, &recipient, MessageBinding::default());
    let handler = Handler::new();
    reconcile(&ctx, &recipient, &handler).unwrap();
    reconcile(&ctx, &recipient, &handler).unwrap();
    let task = ctx.store.get(recipient.task().unwrap()).unwrap();
    let run = task.runs.last().unwrap();
    assert_eq!(handler.task_calls.get(), 1);
    assert_eq!(handler.receipts.borrow().len(), 1);
    assert_eq!(task.status, Status::Running);
    assert_eq!(run.blocked_excerpt, None);
    assert!(!run.seen_working);
    assert_eq!(run.idle_since, None);
    assert!(!run.idle_no_report);
    assert_eq!(run.pending_reply, None);
    assert!(run.inbox.effects_applied.contains(&seq));
    let entry = Inbox::new(&ctx.store, recipient)
        .unwrap()
        .entry(seq)
        .unwrap();
    assert_eq!(entry.effect.unwrap().status, EffectStatus::Applied);
}

#[test]
fn reconciler_reuses_started_receipt_after_interruption() {
    let (_dir, ctx, recipient) = fixture();
    let seq = observed(&ctx, &recipient, MessageBinding::default());
    let handler = Handler::new();
    handler.fail.set(true);
    assert!(reconcile(&ctx, &recipient, &handler).is_err());
    let inbox = Inbox::new(&ctx.store, recipient.clone()).unwrap();
    assert_eq!(
        inbox.entry(seq).unwrap().effect.unwrap().status,
        EffectStatus::Started
    );
    reconcile(&ctx, &recipient, &handler).unwrap();
    assert_eq!(handler.receipts.borrow().len(), 2);
    assert_eq!(handler.receipts.borrow()[0], handler.receipts.borrow()[1]);
    assert_eq!(handler.task_calls.get(), 1);
}

#[test]
fn stale_run_round_or_decision_skips_effects() {
    for change in ["ended", "round", "decision", "replacement"] {
        let (_dir, ctx, recipient) = fixture();
        let mut message_binding = MessageBinding::default();
        if change == "round" {
            message_binding.review_round = Some(1);
        }
        if change == "decision" {
            message_binding.decision = Some("obsolete".into());
        }
        let seq = observed(&ctx, &recipient, message_binding);
        ctx.store
            .update(recipient.task().unwrap(), |task| {
                if change == "ended" {
                    task.runs[0].ended = Some("2026-10-19T00:02:00Z".parse().unwrap());
                }
                if change == "replacement" {
                    let mut new = task.runs[0].clone();
                    new.id = "R2".into();
                    task.runs.push(new);
                }
                Ok(())
            })
            .unwrap();
        let handler = Handler::new();
        reconcile(&ctx, &recipient, &handler).unwrap();
        assert!(handler.receipts.borrow().is_empty(), "{change}");
        assert_eq!(
            Inbox::new(&ctx.store, recipient)
                .unwrap()
                .entry(seq)
                .unwrap()
                .effect
                .unwrap()
                .status,
            EffectStatus::SkippedStale
        );
    }
}

#[test]
fn task_effect_marker_prevents_duplicate_after_effect_record_crash() {
    let (_dir, ctx, recipient) = fixture();
    let seq = observed(&ctx, &recipient, MessageBinding::default());
    let handler = Handler::new();
    reconcile(&ctx, &recipient, &handler).unwrap();
    let inbox = Inbox::new(&ctx.store, recipient.clone()).unwrap();
    let mut effect = inbox.entry(seq).unwrap().effect.unwrap();
    effect.status = EffectStatus::Started;
    // Simulate task transaction committed, process died before effect record completion.
    std::fs::write(
        inbox.record_path(seq, "effect"),
        serde_json::to_vec(&effect).unwrap(),
    )
    .unwrap();
    reconcile(&ctx, &recipient, &handler).unwrap();
    assert_eq!(handler.task_calls.get(), 1);
    assert_eq!(
        ctx.store.get(recipient.task().unwrap()).unwrap().labels,
        vec!["effect"]
    );
    assert_eq!(
        inbox.entry(seq).unwrap().effect.unwrap().status,
        EffectStatus::Applied
    );
}

#[test]
fn planner_effect_binding_must_name_the_addressed_run() {
    let (_dir, ctx, run_recipient) = fixture();
    let task_id = run_recipient.task().unwrap().to_string();
    ctx.store
        .update(&task_id, |task| {
            task.plan_runs.push(serde_json::from_value(json!({
                "id":"PL1","profile":"fake","harness":"fake","spec_revision":"v1",
                "started":"2026-10-19T00:00:00Z","output_dir":"/unused",
                "rounds":0,"state":"planning"
            }))?);
            Ok(())
        })
        .unwrap();
    let recipient = Recipient::PlanRun {
        task: task_id,
        plan_run: "PL1".into(),
    };
    let seq = observed(
        &ctx,
        &recipient,
        MessageBinding {
            run: Some("PL2".into()),
            ..MessageBinding::default()
        },
    );
    let handler = Handler::new();
    reconcile(&ctx, &recipient, &handler).unwrap();
    assert!(handler.receipts.borrow().is_empty());
    assert_eq!(
        Inbox::new(&ctx.store, recipient)
            .unwrap()
            .entry(seq)
            .unwrap()
            .effect
            .unwrap()
            .status,
        EffectStatus::SkippedStale
    );
}

struct DecisionHandler {
    preparations: Cell<usize>,
    locked_calls: Cell<usize>,
    task_calls: Cell<usize>,
    interrupt_after_answer: Cell<bool>,
    interrupt_before_answer: Cell<bool>,
    omit_answer: Cell<bool>,
    unproven_answer: Cell<bool>,
}

impl DecisionHandler {
    fn new(interrupt: bool) -> Self {
        Self {
            preparations: Cell::new(0),
            locked_calls: Cell::new(0),
            task_calls: Cell::new(0),
            interrupt_after_answer: Cell::new(interrupt),
            interrupt_before_answer: Cell::new(false),
            omit_answer: Cell::new(false),
            unproven_answer: Cell::new(false),
        }
    }
}

fn decision_answer(message: &Message, receipt: &Receipt) -> corral_core::ResumeReceipt {
    corral_core::ResumeReceipt {
        version: 1,
        id: "D1".into(),
        run: "R1".into(),
        outcome: corral_core::ReceiptOutcome::Answered,
        delivery: corral_core::ResumeDelivery::Acknowledged,
        observed_at: receipt.observed_at,
        answer: Some("yes".into()),
        data: json!({"inbox_message":message.id}),
    }
}

impl EffectHandler for DecisionHandler {
    fn prepare_decision(
        &self,
        message: &Message,
        receipt: &Receipt,
    ) -> Result<Option<corral_core::ResumeReceipt>> {
        self.preparations.set(self.preparations.get() + 1);
        let mut answer = decision_answer(message, receipt);
        if self.unproven_answer.get() {
            answer.data = json!({});
        }
        Ok(Some(answer))
    }
    fn apply_locked(
        &self,
        ctx: &Ctx,
        message: &Message,
        _: &Receipt,
        decision_receipt: Option<&corral_core::ResumeReceipt>,
    ) -> Result<()> {
        self.locked_calls.set(self.locked_calls.get() + 1);
        let answer = decision_receipt.unwrap().clone();
        let persisted = Inbox::new(&ctx.store, message.recipient.clone())?
            .entry(message.seq)?
            .effect
            .unwrap()
            .decision_receipt;
        assert_eq!(
            persisted.as_ref(),
            Some(&answer),
            "answer proof must be durable before locked effects"
        );
        if self.interrupt_before_answer.replace(false) {
            anyhow::bail!("interrupted before locked decision answer");
        }
        if self.omit_answer.get() {
            return Ok(());
        }
        ctx.store
            .update(message.recipient.task().unwrap(), |task| {
                let decision = &mut task.runs[0].obligations.decisions[0];
                if let Some(existing) = &decision.receipt {
                    anyhow::ensure!(existing == &answer, "conflicting decision answer");
                } else {
                    decision.receipt = Some(answer);
                }
                Ok(())
            })?;
        if self.interrupt_after_answer.replace(false) {
            anyhow::bail!("interrupted after locked decision answer");
        }
        Ok(())
    }

    fn apply_task(&self, _: &mut Task, _: &Message, _: &Receipt) -> Result<()> {
        self.task_calls.set(self.task_calls.get() + 1);
        Ok(())
    }
}

fn decision_fixture() -> (tempfile::TempDir, Ctx, Recipient, u64) {
    let (dir, ctx, recipient) = fixture();
    ctx.store
        .update(recipient.task().unwrap(), |task| {
            task.runs[0].obligations.decisions.push(serde_json::from_value(json!({
            "request": {
                "version":1,"id":"D1","run":"R1","question":"Continue?",
                "requested_action":"answer","owner":"owner","source":"corral","deadline":null
            },
            "registered_at":"2026-10-19T00:00:00Z",
            "observation":{"at":"2026-10-19T00:00:00Z","state":"pending","detail":""},
            "receipt":null
        }))?);
            Ok(())
        })
        .unwrap();
    let seq = observed(
        &ctx,
        &recipient,
        MessageBinding {
            run: Some("R1".into()),
            decision: Some("D1".into()),
            review_round: Some(0),
            ..MessageBinding::default()
        },
    );
    (dir, ctx, recipient, seq)
}

#[test]
fn decision_answer_by_locked_effect_completes_task_local_effects() {
    // Given an observed reply bound to a pending decision.
    let (_dir, ctx, recipient, seq) = decision_fixture();
    let handler = DecisionHandler::new(false);
    // When the locked handler answers that same decision.
    reconcile(&ctx, &recipient, &handler).unwrap();
    // Then its answer does not invalidate its own task-local effect.
    let task = ctx.store.get(recipient.task().unwrap()).unwrap();
    assert_eq!(handler.task_calls.get(), 1);
    assert!(task.runs[0].pending_reply.is_none());
    assert_eq!(
        Inbox::new(&ctx.store, recipient)
            .unwrap()
            .entry(seq)
            .unwrap()
            .effect
            .unwrap()
            .status,
        EffectStatus::Applied
    );
}

#[test]
fn decision_answer_replays_after_interruption_before_task_effects() {
    // Given a locked effect that answers the decision before being interrupted.
    let (dir, ctx, recipient, seq) = decision_fixture();
    let handler = DecisionHandler::new(true);
    assert!(reconcile(&ctx, &recipient, &handler).is_err());
    let answer = ctx.store.get(recipient.task().unwrap()).unwrap().runs[0]
        .obligations
        .decisions[0]
        .receipt
        .clone();
    let reopened = Ctx::from_store(Store::open(dir.path()).unwrap());
    // When the started effect is replayed from durable state.
    reconcile(&reopened, &recipient, &handler).unwrap();
    // Then the original answer receipt is reused and local effects complete once.
    let task = reopened.store.get(recipient.task().unwrap()).unwrap();
    assert_eq!(task.runs[0].obligations.decisions[0].receipt, answer);
    assert_eq!(handler.task_calls.get(), 1);
    assert_eq!(
        handler.preparations.get(),
        1,
        "replay must reuse its frozen answer, not prepare another"
    );
    assert!(task.runs[0].pending_reply.is_none());
    assert_eq!(
        Inbox::new(&reopened.store, recipient)
            .unwrap()
            .entry(seq)
            .unwrap()
            .effect
            .unwrap()
            .status,
        EffectStatus::Applied
    );
}

#[test]
fn independently_answered_decision_cannot_acquire_effect_ownership() {
    // Given a decision already answered before this effect had a journal.
    let (_dir, ctx, recipient, seq) = decision_fixture();
    let inbox = Inbox::new(&ctx.store, recipient.clone()).unwrap();
    let entry = inbox.entry(seq).unwrap();
    let answer = decision_answer(&entry.message, entry.observed.as_ref().unwrap());
    ctx.store
        .update(recipient.task().unwrap(), |task| {
            task.runs[0].obligations.decisions[0].receipt = Some(answer);
            Ok(())
        })
        .unwrap();
    let handler = DecisionHandler::new(false);
    // When reconciliation sees even an identical independently existing answer.
    reconcile(&ctx, &recipient, &handler).unwrap();
    // Then preparation cannot manufacture retrospective ownership of that answer.
    assert_eq!(handler.preparations.get(), 0);
    assert_eq!(handler.locked_calls.get(), 0);
    assert_eq!(handler.task_calls.get(), 0);
    assert!(
        ctx.store.get(recipient.task().unwrap()).unwrap().runs[0]
            .pending_reply
            .is_some()
    );
    assert_eq!(
        inbox.entry(seq).unwrap().effect.unwrap().status,
        EffectStatus::SkippedStale
    );
}

#[test]
fn interrupted_decision_effect_fences_different_answers_and_supersession() {
    for change in [
        "answer",
        "provenance",
        "observed_at",
        "ended",
        "outcome",
        "round",
    ] {
        // Given an interrupted locked answer with its exact receipt already journaled.
        let (_dir, ctx, recipient, seq) = decision_fixture();
        let handler = DecisionHandler::new(true);
        assert!(reconcile(&ctx, &recipient, &handler).is_err());
        let inbox = Inbox::new(&ctx.store, recipient.clone()).unwrap();
        let original = inbox.entry(seq).unwrap().effect.unwrap().decision_receipt;
        ctx.store
            .update(recipient.task().unwrap(), |task| {
                let run = &mut task.runs[0];
                let receipt = run.obligations.decisions[0].receipt.as_mut().unwrap();
                match change {
                    "answer" => receipt.answer = Some("independent answer".into()),
                    "provenance" => receipt.data = json!({"inbox_message":"another-effect"}),
                    "observed_at" => receipt.observed_at = "2026-10-19T00:02:00Z".parse().unwrap(),
                    "ended" => run.ended = Some("2026-10-19T00:02:00Z".parse().unwrap()),
                    "outcome" => run.outcome = Some(corral_core::RunOutcome::Interrupted),
                    "round" => task.rounds = 1,
                    _ => unreachable!(),
                }
                Ok(())
            })
            .unwrap();
        // When another answer or a superseded binding is encountered on replay.
        reconcile(&ctx, &recipient, &handler).unwrap();
        // Then the original proof is retained, but no further locked/local effect runs.
        assert_eq!(handler.locked_calls.get(), 1, "{change}");
        assert_eq!(handler.task_calls.get(), 0, "{change}");
        let effect = inbox.entry(seq).unwrap().effect.unwrap();
        assert_eq!(effect.status, EffectStatus::SkippedStale, "{change}");
        assert_eq!(effect.decision_receipt, original);
    }
}

#[test]
fn decision_effect_replays_original_preparation_before_answer_was_written() {
    let (dir, ctx, recipient, seq) = decision_fixture();
    let handler = DecisionHandler::new(false);
    handler.interrupt_before_answer.set(true);
    assert!(reconcile(&ctx, &recipient, &handler).is_err());
    let inbox = Inbox::new(&ctx.store, recipient.clone()).unwrap();
    let prepared = inbox.entry(seq).unwrap().effect.unwrap().decision_receipt;
    assert!(
        ctx.store.get(recipient.task().unwrap()).unwrap().runs[0]
            .obligations
            .decisions[0]
            .pending()
    );
    let reopened = Ctx::from_store(Store::open(dir.path()).unwrap());
    reconcile(&reopened, &recipient, &handler).unwrap();
    assert_eq!(handler.preparations.get(), 1);
    assert_eq!(handler.task_calls.get(), 1);
    let effect = inbox.entry(seq).unwrap().effect.unwrap();
    assert_eq!(effect.decision_receipt, prepared);
    assert_eq!(effect.status, EffectStatus::Applied);
}

#[test]
fn decision_effect_task_marker_prevents_duplicate_after_journal_interruption() {
    let (_dir, ctx, recipient, seq) = decision_fixture();
    let handler = DecisionHandler::new(false);
    reconcile(&ctx, &recipient, &handler).unwrap();
    let inbox = Inbox::new(&ctx.store, recipient.clone()).unwrap();
    let mut effect = inbox.entry(seq).unwrap().effect.unwrap();
    effect.status = EffectStatus::Started;
    std::fs::write(
        inbox.record_path(seq, "effect"),
        serde_json::to_vec(&effect).unwrap(),
    )
    .unwrap();
    reconcile(&ctx, &recipient, &handler).unwrap();
    assert_eq!(handler.preparations.get(), 1);
    assert_eq!(handler.task_calls.get(), 1);
    assert_eq!(
        inbox.entry(seq).unwrap().effect.unwrap().status,
        EffectStatus::Applied
    );
}

#[test]
fn legacy_started_decision_effect_without_proof_cannot_claim_an_answer() {
    let (_dir, ctx, recipient, seq) = decision_fixture();
    let inbox = Inbox::new(&ctx.store, recipient.clone()).unwrap();
    inbox.start_effect(seq, false, None).unwrap();
    let entry = inbox.entry(seq).unwrap();
    let answer = decision_answer(&entry.message, entry.observed.as_ref().unwrap());
    ctx.store
        .update(recipient.task().unwrap(), |task| {
            task.runs[0].obligations.decisions[0].receipt = Some(answer);
            Ok(())
        })
        .unwrap();
    let handler = DecisionHandler::new(false);
    reconcile(&ctx, &recipient, &handler).unwrap();
    assert_eq!(handler.preparations.get(), 0);
    assert_eq!(handler.locked_calls.get(), 0);
    assert_eq!(
        inbox.entry(seq).unwrap().effect.unwrap().status,
        EffectStatus::SkippedStale
    );
}

#[test]
fn decision_effect_cannot_complete_without_the_prepared_answer() {
    let (_dir, ctx, recipient, seq) = decision_fixture();
    let handler = DecisionHandler::new(false);
    handler.omit_answer.set(true);
    assert!(reconcile(&ctx, &recipient, &handler).is_err());
    assert_eq!(handler.task_calls.get(), 0);
    let task = ctx.store.get(recipient.task().unwrap()).unwrap();
    assert!(task.runs[0].obligations.decisions[0].pending());
    assert!(task.runs[0].pending_reply.is_some());
    assert_eq!(
        Inbox::new(&ctx.store, recipient)
            .unwrap()
            .entry(seq)
            .unwrap()
            .effect
            .unwrap()
            .status,
        EffectStatus::Started
    );
}

#[test]
fn prepared_decision_requires_message_specific_provenance_before_effects() {
    let (_dir, ctx, recipient, seq) = decision_fixture();
    let handler = DecisionHandler::new(false);
    handler.unproven_answer.set(true);
    assert!(reconcile(&ctx, &recipient, &handler).is_err());
    assert_eq!(handler.locked_calls.get(), 0);
    assert_eq!(handler.task_calls.get(), 0);
    assert!(
        Inbox::new(&ctx.store, recipient)
            .unwrap()
            .entry(seq)
            .unwrap()
            .effect
            .is_none()
    );
}
