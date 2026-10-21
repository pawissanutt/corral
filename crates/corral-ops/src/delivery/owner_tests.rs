use std::cell::{Cell, RefCell};

use corral_core::inbox::{EffectStatus, Recipient};
use corral_core::{RunOutcome, Status};
use serde_json::Value;

use super::*;
use crate::delivery::omo_fixture::OmoWorld;

thread_local! {
    /// Runs once between `done`'s first unobserved check and its final admission.
    pub(super) static BEFORE_DONE_ADMISSION: RefCell<Option<Box<dyn FnOnce()>>> =
        const { RefCell::new(None) };
}

fn messages(world: &OmoWorld) -> Vec<Message> {
    Inbox::new(&world.ctx.store, world.recipient())
        .unwrap()
        .messages()
        .unwrap()
}

/// Caller paths resolve the production adapter, which test builds never find: posting
/// must still succeed and the composer must stay untouched.
#[test]
fn reply_posts_to_the_inbox_and_never_pastes() {
    let world = OmoWorld::new(Status::NeedsMe);
    let registry = world.temp.path().join("agent/rpc-host-daemon");
    let hidden = world.temp.path().join("agent/registry-hidden");
    std::fs::rename(&registry, &hidden).unwrap();
    let task = crate::reply(&world.ctx, &world.task.id, "use the second option").unwrap();
    std::fs::rename(&hidden, &registry).unwrap();
    assert_eq!(
        task.runs[0].pending_reply.as_deref(),
        Some("use the second option")
    );
    let posted = messages(&world);
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0].kind, "owner_reply");
    assert!(world.terminal_writes().is_empty());
    let replied = world.events("replied");
    assert_eq!(replied[0]["message"], posted[0].id.as_str());
    assert_eq!(replied[0]["submitted"], false);
    // A second reply is accepted: inbox runs have no one-reply guard.
    std::fs::rename(&registry, &hidden).unwrap();
    crate::reply(&world.ctx, &world.task.id, "and keep the tests").unwrap();
    std::fs::rename(&hidden, &registry).unwrap();
    assert_eq!(messages(&world).len(), 2);
    // The next delivery pass sends both in one envelope and applies effects.
    kick_with(&world.ctx, &world.recipient(), &world.adapter()).unwrap();
    assert_eq!(world.sends().len(), 1);
    let task = world.ctx.store.get(&world.task.id).unwrap();
    assert_eq!(task.runs[0].pending_reply, None);
    assert_eq!(task.status, Status::Running);
}

#[test]
fn request_changes_reopens_the_run_and_posts_the_request() {
    let world = OmoWorld::new(Status::Review);
    world
        .ctx
        .store
        .update(&world.task.id, |task| {
            task.runs[0].outcome = Some(RunOutcome::Done);
            task.runs[0].ended = Some(jiff::Timestamp::now());
            Ok(())
        })
        .unwrap();
    let task = crate::request_changes(&world.ctx, &world.task.id, "split the module").unwrap();
    assert_eq!(task.status, Status::Running);
    assert_eq!(task.runs[0].outcome, None);
    assert_eq!(
        task.runs[0].pending_reply.as_deref(),
        Some("split the module")
    );
    let posted = messages(&world);
    assert_eq!(posted[0].kind, "changes_requested");
    assert_eq!(
        world.events("changes_requested")[0]["message"],
        posted[0].id.as_str()
    );
    assert!(world.terminal_writes().is_empty());
}

#[test]
fn comment_send_posts_an_owner_note() {
    let world = OmoWorld::new(Status::Running);
    crate::comment(
        &world.ctx,
        &world.task.id,
        "prefer the smaller diff",
        corral_core::CommentAuthor::Owner,
        true,
    )
    .unwrap();
    let posted = messages(&world);
    assert_eq!(posted[0].kind, "owner_note");
    assert!(posted[0].body.contains("prefer the smaller diff"));
    assert_eq!(
        world.ctx.store.get(&world.task.id).unwrap().runs[0].pending_reply,
        None
    );
    assert!(world.terminal_writes().is_empty());
}

#[test]
fn checkpoint_request_is_one_control_message_per_text() {
    let world = OmoWorld::new(Status::Running);
    world
        .ctx
        .store
        .update(&world.task.id, |task| {
            task.runs[0].checkpoint_required = true;
            Ok(())
        })
        .unwrap();
    for _ in 0..2 {
        let task = world.ctx.store.get(&world.task.id).unwrap();
        let error = crate::resume::retry_checkpoint(&world.ctx, &task, &task.runs[0])
            .unwrap_err()
            .to_string();
        assert!(error.contains("posted to the agent inbox"), "{error}");
    }
    let posted = messages(&world);
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0].kind, "control_request");
    assert_eq!(world.events("checkpoint_requested").len(), 2);
}

#[test]
fn done_refuses_while_an_owner_message_is_unobserved() {
    let world = OmoWorld::new(Status::Running);
    let message = post(
        &world.ctx,
        &world.recipient(),
        "owner_reply",
        "one more thing",
        MessageBinding::default(),
        |_, _| Ok(()),
    )
    .unwrap();
    let error = crate::done(&world.ctx, &world.task.id, "finished")
        .unwrap_err()
        .to_string();
    assert!(error.contains("owner_messages_unobserved"), "{error}");
    assert!(error.contains(&message.id), "{error}");
    Inbox::new(&world.ctx.store, world.recipient())
        .unwrap()
        .mark_observed(message.seq, "owner verified in the pane")
        .unwrap();
    let error = format!(
        "{:#}",
        crate::done(&world.ctx, &world.task.id, "finished").unwrap_err()
    );
    assert!(!error.contains("owner_messages_unobserved"), "{error}");
}

struct Interrupting {
    interrupt: Cell<bool>,
}

impl EffectHandler for Interrupting {
    fn prepare_decision(&self, m: &Message, r: &Receipt) -> Result<Option<ResumeReceipt>> {
        OwnerEffects.prepare_decision(m, r)
    }
    fn apply_locked(
        &self,
        ctx: &Ctx,
        m: &Message,
        r: &Receipt,
        d: Option<&ResumeReceipt>,
    ) -> Result<()> {
        OwnerEffects.apply_locked(ctx, m, r, d)
    }
    fn apply_task(&self, task: &mut Task, m: &Message, r: &Receipt) -> Result<()> {
        if self.interrupt.replace(false) {
            anyhow::bail!("process died after the answer was committed");
        }
        OwnerEffects.apply_task(task, m, r)
    }
}

#[test]
fn decision_answer_replays_its_journaled_receipt_after_a_crash() {
    let world = OmoWorld::new(Status::NeedsMe);
    world
        .ctx
        .store
        .update(&world.task.id, |task| {
            task.runs[0].obligations.decisions.push(serde_json::from_value(json!({
                "request": {"version":1,"id":"D1","run":"R1","question":"Continue?",
                    "requested_action":"answer","owner":"owner","source":"corral","deadline":null},
                "registered_at":"2026-10-19T00:00:00Z",
                "observation":{"at":"2026-10-19T00:00:00Z","state":"pending","detail":""},
                "receipt":null
            }))?);
            Ok(())
        })
        .unwrap();
    let recipient = world.recipient();
    let message = post(
        &world.ctx,
        &recipient,
        "owner_reply",
        "yes, continue",
        MessageBinding {
            run: Some("R1".into()),
            decision: Some("D1".into()),
            ..Default::default()
        },
        |_, _| Ok(()),
    )
    .unwrap();
    super::super::deliver(&world.ctx, &recipient, &world.adapter(), None).unwrap();
    super::super::deliver(&world.ctx, &recipient, &world.adapter(), None).unwrap();
    assert!(is_observed(&world.ctx, &recipient, message.seq).unwrap());
    let handler = Interrupting {
        interrupt: Cell::new(true),
    };
    assert!(effects::reconcile(&world.ctx, &recipient, &handler).is_err());
    let answered = world.ctx.store.get(&world.task.id).unwrap().runs[0]
        .obligations
        .decisions[0]
        .receipt
        .clone()
        .unwrap();
    assert_eq!(answered.data["inbox_message"], message.id.as_str());
    assert_eq!(answered.answer.as_deref(), Some("yes, continue"));
    effects::reconcile(&world.ctx, &recipient, &handler).unwrap();
    let task = world.ctx.store.get(&world.task.id).unwrap();
    assert_eq!(
        task.runs[0].obligations.decisions[0].receipt,
        Some(answered)
    );
    assert_eq!(world.events("run_resumed").len(), 1);
    let entry = Inbox::new(&world.ctx.store, recipient)
        .unwrap()
        .entry(message.seq)
        .unwrap();
    assert_eq!(entry.effect.unwrap().status, EffectStatus::Applied);
}

#[test]
fn run_end_cancels_unsent_and_orphans_attempted_messages_once() {
    let world = OmoWorld::new(Status::Running);
    world.script(&["0 send-busy.exit0.json 0"]);
    let recipient = world.recipient();
    let attempted = post(
        &world.ctx,
        &recipient,
        "owner_reply",
        "first",
        MessageBinding::default(),
        |_, _| Ok(()),
    )
    .unwrap();
    kick_with(&world.ctx, &recipient, &world.adapter()).unwrap();
    let unsent = post(
        &world.ctx,
        &recipient,
        "owner_reply",
        "second",
        MessageBinding::default(),
        |_, _| Ok(()),
    )
    .unwrap();
    world
        .ctx
        .store
        .update(&world.task.id, |task| {
            task.runs[0].outcome = Some(RunOutcome::Superseded);
            task.runs[0].ended = Some(jiff::Timestamp::now());
            Ok(())
        })
        .unwrap();
    for _ in 0..2 {
        retire(&world.ctx, &recipient, "run superseded").unwrap();
    }
    let inbox = Inbox::new(&world.ctx.store, recipient).unwrap();
    assert_eq!(
        inbox.terminal(unsent.seq).unwrap(),
        Some(TerminalState::Cancelled)
    );
    assert_eq!(inbox.terminal(attempted.seq).unwrap(), None);
    assert_eq!(world.events("delivery_orphaned").len(), 1);
    assert_eq!(world.events("message_cancelled").len(), 1);
}

#[test]
fn legacy_runs_never_reach_the_inbox() {
    assert_eq!(launch_mode("omo"), DeliveryMode::Inbox);
    for harness in ["claude", "omp", "codex", "generic"] {
        assert_eq!(launch_mode(harness), DeliveryMode::Legacy);
    }
    let world = OmoWorld::new(Status::Running);
    world
        .ctx
        .store
        .update(&world.task.id, |task| {
            task.runs[0].delivery = DeliveryMode::Legacy;
            Ok(())
        })
        .unwrap();
    let task = world.ctx.store.get(&world.task.id).unwrap();
    assert_eq!(live_run(&task), None);
    reconcile_all(&world.ctx).unwrap();
    assert!(!world.ctx.store.dir().join("inbox").exists());
    assert!(world.sends().is_empty());
}

#[test]
fn digest_counts_an_inbox_change_request_once_observed() {
    let event = |kind: &str, detail: Value| -> corral_core::Event {
        serde_json::from_value(json!({
            "ts":"2026-10-19T00:00:10Z","task":"T-001","kind":kind,"detail":detail
        }))
        .unwrap()
    };
    let posted = event(
        "changes_requested",
        json!({"by":"owner","submitted":false,"message":"T-001_R1#1","run":"R1"}),
    );
    let since = "2026-10-19T00:00:00Z".parse().unwrap();
    let at = "2026-10-19T01:00:00Z".parse().unwrap();
    let digest = crate::collect_digest(&[], std::slice::from_ref(&posted), since, at);
    assert!(digest.sent_back.is_empty());
    let observed = event("message_observed", json!({"id":"T-001_R1#1"}));
    let digest = crate::collect_digest(&[], &[posted, observed], since, at);
    assert_eq!(digest.sent_back.len(), 1);
}

#[test]
fn unobserved_wake_message_alerts_once() {
    let world = OmoWorld::new(Status::Running);
    world.set_list(&[]);
    post(
        &world.ctx,
        &world.recipient(),
        "owner_reply",
        "waiting",
        MessageBinding::default(),
        |_, _| Ok(()),
    )
    .unwrap();
    let later = jiff::Timestamp::now() + jiff::SignedDuration::from_mins(11);
    for _ in 0..2 {
        super::super::deliver_at(
            &world.ctx,
            &world.recipient(),
            &world.adapter(),
            None,
            later,
        )
        .unwrap();
    }
    assert_eq!(world.events("message_unobserved").len(), 1);
    assert_eq!(world.events("delivery_blocked").len(), 1);
}

fn post_reply(world: &OmoWorld, text: &str, binding: MessageBinding) -> Message {
    let recipient = world.recipient();
    post(
        &world.ctx,
        &recipient,
        "owner_reply",
        text,
        binding,
        |task, _| {
            let pending = match &recipient {
                Recipient::PlanRun { .. } => &mut task.plan_runs[0].pending_reply,
                _ => &mut task.runs[0].pending_reply,
            };
            *pending = Some(text.into());
            Ok(())
        },
    )
    .unwrap()
}

#[test]
fn sync_finishes_effects_interrupted_after_the_receipt() {
    for planner in [false, true] {
        let world = if planner {
            OmoWorld::planner(Status::NeedsMe)
        } else {
            OmoWorld::new(Status::NeedsMe)
        };
        let recipient = world.recipient();
        // Given an observed message whose process died before (or during) its effect.
        let first = post_reply(&world, "first", MessageBinding::default());
        for _ in 0..2 {
            super::super::deliver(&world.ctx, &recipient, &world.adapter(), None).unwrap();
        }
        assert!(is_observed(&world.ctx, &recipient, first.seq).unwrap());
        let work = work(&world.ctx, &recipient).unwrap();
        assert!(work.effects && !work.unsettled, "{work:?}");
        // When the production sync pass runs after restart.
        reconcile_all_with(&world.ctx, &world.adapter()).unwrap();
        // Then the effect and the derived pending view complete.
        assert_eq!(world.pending(), None, "planner={planner}");
        let inbox = Inbox::new(&world.ctx.store, recipient.clone()).unwrap();
        let effect = inbox.entry(first.seq).unwrap().effect.unwrap();
        assert_eq!(effect.status, EffectStatus::Applied);
        // And a Started effect (interrupted inside it) is finished the same way.
        let second = post_reply(&world, "second", MessageBinding::default());
        for _ in 0..2 {
            super::super::deliver(&world.ctx, &recipient, &world.adapter(), None).unwrap();
        }
        let handler = Interrupting {
            interrupt: Cell::new(true),
        };
        assert!(effects::reconcile(&world.ctx, &recipient, &handler).is_err());
        reconcile_all_with(&world.ctx, &world.adapter()).unwrap();
        let effect = inbox.entry(second.seq).unwrap().effect.unwrap();
        assert_eq!(effect.status, EffectStatus::Applied);
        assert_eq!(world.pending(), None);
    }
}

#[test]
fn sync_collects_a_late_receipt_for_an_abandoned_attempt() {
    let world = OmoWorld::new(Status::Running);
    world.script(&["0 send-busy.exit0.json 0"]);
    let recipient = world.recipient();
    let message = post_reply(&world, "late", MessageBinding::default());
    kick_with(&world.ctx, &recipient, &world.adapter()).unwrap();
    let inbox = Inbox::new(&world.ctx.store, recipient.clone()).unwrap();
    inbox
        .recover(corral_core::inbox::Recovery::Abandon)
        .unwrap();
    assert_eq!(inbox.state().unwrap().open_envelope, None);
    world.deliver_entry(1, "3e184ffa-e124-4e8f-a207-c031105f758a");
    reconcile_all_with(&world.ctx, &world.adapter()).unwrap();
    assert!(is_observed(&world.ctx, &recipient, message.seq).unwrap());
    assert_eq!(inbox.terminal(message.seq).unwrap(), None);
    assert_eq!(world.sends().len(), 1);
}

#[test]
fn finished_runs_never_resend_an_uncertain_attempt() {
    let world = OmoWorld::new(Status::Running);
    world.script(&["5 send-internal-error.exit5.json 0"; 3]);
    let recipient = world.recipient();
    post_reply(&world, "maybe delivered", MessageBinding::default());
    kick_with(&world.ctx, &recipient, &world.adapter()).unwrap();
    let sent = world.sends().len();
    world
        .ctx
        .store
        .update(&world.task.id, |task| {
            task.runs[0].outcome = Some(RunOutcome::Superseded);
            task.runs[0].ended = Some(jiff::Timestamp::now());
            Ok(())
        })
        .unwrap();
    for _ in 0..2 {
        reconcile_all_with(&world.ctx, &world.adapter()).unwrap();
    }
    assert_eq!(world.sends().len(), sent);
    assert_eq!(world.events("delivery_orphaned").len(), 1);
}

#[test]
fn pending_view_follows_message_identity_not_text() {
    for planner in [false, true] {
        // Repeated text: the first "continue" is observed while the second is still in flight.
        let world = if planner {
            OmoWorld::planner(Status::Running)
        } else {
            OmoWorld::new(Status::Running)
        };
        let recipient = world.recipient();
        world.script(&["0 send-busy.exit0.json 0", "0 send-busy.exit0.json 0"]);
        post_reply(&world, "continue", MessageBinding::default());
        kick_with(&world.ctx, &recipient, &world.adapter()).unwrap();
        let second = post_reply(&world, "continue", MessageBinding::default());
        world.deliver_entry(1, "3e184ffa-e124-4e8f-a207-c031105f758a");
        kick_with(&world.ctx, &recipient, &world.adapter()).unwrap();
        assert!(!is_observed(&world.ctx, &recipient, second.seq).unwrap());
        assert_eq!(
            world.pending().as_deref(),
            Some("continue"),
            "planner={planner}"
        );
        // An old terminal message with the same text never clears a newer reply.
        let world = if planner {
            OmoWorld::planner(Status::Running)
        } else {
            OmoWorld::new(Status::Running)
        };
        world.script(&[
            "1 send-scope-denied.exit1.json 0",
            "0 send-busy.exit0.json 0",
        ]);
        post_reply(&world, "x", MessageBinding::default());
        kick_with(&world.ctx, &world.recipient(), &world.adapter()).unwrap();
        assert_eq!(world.pending(), None);
        post_reply(&world, "x", MessageBinding::default());
        kick_with(&world.ctx, &world.recipient(), &world.adapter()).unwrap();
        assert_eq!(world.pending().as_deref(), Some("x"));
    }
}

#[test]
fn two_replies_to_one_decision_both_clear_the_view() {
    let world = OmoWorld::new(Status::NeedsMe);
    world
        .ctx
        .store
        .update(&world.task.id, |task| {
            task.runs[0].obligations.decisions.push(serde_json::from_value(json!({
                "request": {"version":1,"id":"D1","run":"R1","question":"Continue?",
                    "requested_action":"answer","owner":"owner","source":"corral","deadline":null},
                "registered_at":"2026-10-19T00:00:00Z",
                "observation":{"at":"2026-10-19T00:00:00Z","state":"pending","detail":""},
                "receipt":null
            }))?);
            Ok(())
        })
        .unwrap();
    let binding = MessageBinding {
        run: Some("R1".into()),
        decision: Some("D1".into()),
        ..Default::default()
    };
    post_reply(&world, "yes", binding.clone());
    let second = post_reply(&world, "yes, and also", binding);
    kick_with(&world.ctx, &world.recipient(), &world.adapter()).unwrap();
    let inbox = Inbox::new(&world.ctx.store, world.recipient()).unwrap();
    let effect = inbox.entry(second.seq).unwrap().effect.unwrap();
    assert_eq!(effect.status, EffectStatus::SkippedStale);
    assert_eq!(world.pending(), None);
}

#[test]
fn foreign_server_never_receives_a_gateway_send() {
    for planner in [false, true] {
        let world = if planner {
            OmoWorld::planner(Status::Running)
        } else {
            OmoWorld::new(Status::Running)
        };
        world
            .ctx
            .store
            .update(&world.task.id, |task| {
                let herdr = match planner {
                    true => task.plan_runs[0].herdr.as_mut(),
                    false => task.runs[0].herdr.as_mut(),
                };
                herdr.unwrap().socket = Some("/tmp/another-herdr-server.sock".into());
                Ok(())
            })
            .unwrap();
        post_reply(
            &world,
            "for the original server only",
            MessageBinding::default(),
        );
        let status = kick_with(&world.ctx, &world.recipient(), &world.adapter()).unwrap();
        assert!(
            matches!(&status, DeliveryStatus::Blocked { reason } if reason.contains("does not own")),
            "{status:?}"
        );
        assert!(world.sends().is_empty());
        assert!(world.terminal_writes().is_empty());
    }
}

#[test]
fn an_owner_post_racing_done_keeps_the_run_open() {
    let world = OmoWorld::new(Status::Running);
    let worktree = world.worktree.clone();
    let git = |args: &[&str]| {
        let out = crate::test_support::command("git", &worktree)
            .arg("-C")
            .arg(&worktree)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.name", "corral test"]);
    git(&["config", "user.email", "corral@test.invalid"]);
    git(&["config", "commit.gpgsign", "false"]);
    std::fs::write(worktree.join("README.md"), "fixture\n").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "initial"]);
    let sha = git(&["rev-parse", "HEAD"]);
    world
        .ctx
        .store
        .update(&world.task.id, |task| {
            task.runs[0].base_sha = sha;
            task.runs[0].branch = "main".into();
            task.runs[0].checkpoint_required = false;
            Ok(())
        })
        .unwrap();
    let ctx = world.ctx.clone();
    let recipient = world.recipient();
    tests::BEFORE_DONE_ADMISSION.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move || {
            post(
                &ctx,
                &recipient,
                "owner_reply",
                "wait, one more thing",
                MessageBinding::default(),
                |_, _| Ok(()),
            )
            .unwrap();
        }));
    });
    let error = format!(
        "{:#}",
        crate::done(&world.ctx, &world.task.id, "finished").unwrap_err()
    );
    assert!(error.contains("owner_messages_unobserved"), "{error}");
    let task = world.ctx.store.get(&world.task.id).unwrap();
    assert_eq!(task.status, Status::Running);
    assert_eq!(task.runs[0].outcome, None);
}
