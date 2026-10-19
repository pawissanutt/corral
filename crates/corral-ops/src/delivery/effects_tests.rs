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
    fn apply_locked(&self, _: &Ctx, _: &Message, receipt: &Receipt) -> Result<()> {
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
