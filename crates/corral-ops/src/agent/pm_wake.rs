//! The keyed PM wake outbox: one `question_wake` per question transition into the Pi-native PM
//! inbox, reposted after a crash and deduplicated by key. Owner: T-252 child pm-role.
use anyhow::{Context, Result, ensure};
use corral_core::agent::{
    self, AgentPm, AgentQuestion, HostHealth, OutboxState, QuestionWakeRoute, pm_state,
};
use corral_core::inbox::{
    Envelope, Inbox, Message, MessageBinding, ObserveCursor, Post, ReceiptLevel, Recipient,
    SendOutcome, SessionBinding, TerminalState, Urgency,
};
use serde_json::json;

use crate::Ctx;
use crate::delivery::{
    BindOutcome, Capabilities, DeliveryAdapter, Observation, Readiness, RecipientRef,
};

use super::delivery::PiNativeAdapter;

/// The T-252 child that lands the outbox.
pub const OWNER: &str = "pm-role";

fn healthy(pm: &AgentPm) -> bool {
    pm.state.binding.health == HostHealth::Healthy
        && super::admission::recheck_pm(pm, pm.state.binding.generation).is_ok()
}

/// Reposts pending entries, acknowledging only after durable publication. Sync owns this
/// sweep; no PM tool calls it. Delivery is attempted even when the outbox was already acked.
pub fn repost(ctx: &Ctx) -> Result<usize> {
    let Some(_outbox) = ctx.store.try_lock("pm-question-wake-outbox")? else {
        return Ok(0);
    };
    let Some(pm) = pm_state::load(&ctx.store)?.filter(healthy) else {
        return Ok(0);
    };
    let inbox = Inbox::new(&ctx.store, Recipient::Pm)?;
    let mut posted = 0;
    for task in ctx.store.list()? {
        if task.project.as_deref() != Some(pm.state.binding.project.as_str()) {
            continue;
        }
        for view in crate::questions::views(&task).into_iter().filter(|v| {
            v.pending
                && v.question.wake.route == QuestionWakeRoute::PmInbox
                && v.question.wake.state == OutboxState::Pending
        }) {
            posted += usize::from(post_one(ctx, &inbox, &pm, &view)?);
        }
    }
    // Pm posts enqueue only. The registered Pi adapter's submit_input actually starts the
    // idle turn; owner::kick chooses OmO for Pm, so it is deliberately not used here.
    if let Err(error) = crate::delivery::deliver(ctx, &Recipient::Pm, &ScopedPm(pm), None) {
        // Missing control credentials or a racing host loss belong to normal sync recovery.
        // The published message stays queued, independently of its acknowledged outbox.
        crate::messages::report(
            None,
            "PM question wake",
            format!("delivery deferred: {error:#}"),
        );
    }
    Ok(posted)
}

fn post_one(
    ctx: &Ctx,
    inbox: &Inbox<'_>,
    pm: &AgentPm,
    view: &crate::questions::QuestionView,
) -> Result<bool> {
    let Some(current_pm) = pm_state::load(&ctx.store)?.filter(healthy) else {
        return Ok(false);
    };
    let task = ctx.store.get(&view.task)?;
    if !same_pm(pm, &current_pm)
        || task.project.as_deref() != Some(current_pm.state.binding.project.as_str())
        || !crate::questions::views(&task).iter().any(|fresh| {
            fresh.pending
                && fresh.question.key == view.question.key
                && fresh.question.revision == view.question.revision
                && fresh.question.wake == view.question.wake
        })
    {
        return Ok(false);
    }
    // The question's outbox was committed before this post. A PM/project change here may
    // leave a stale queued wake, which ScopedPm refuses; it must never ack a new transition.
    let message = publish(inbox, &view.question, &current_pm.state.binding.project)?;
    acknowledge(ctx, pm, view, &message)
}

fn acknowledge(
    ctx: &Ctx,
    pm: &AgentPm,
    view: &crate::questions::QuestionView,
    message: &Message,
) -> Result<bool> {
    let saved = ctx.store.try_update(&view.task, |task| {
        let Some(current_pm) = pm_state::load(&ctx.store)?.filter(healthy) else {
            return Ok(false);
        };
        if !same_pm(pm, &current_pm)
            || task.project.as_deref() != Some(current_pm.state.binding.project.as_str())
        {
            return Ok(false);
        }
        let container = crate::questions::QuestionKey::parse(&view.question.key)?.container;
        if agent::container_closure(task, &container).is_some() {
            return Ok(false);
        }
        let Some(question) = agent::container_state_mut(task, &container)
            .and_then(|s| s.questions.iter_mut().find(|q| q.key == view.question.key))
        else {
            return Ok(false);
        };
        // A delayed snapshot cannot acknowledge a newer transition's outbox.
        if !question.recorded_pending()
            || question.revision != view.question.revision
            || question.wake != view.question.wake
        {
            return Ok(false);
        }
        question.wake.state = OutboxState::Posted;
        question.wake.message_id = Some(message.id.clone());
        Ok(true)
    })?;
    Ok(saved.is_some())
}

fn same_pm(expected: &AgentPm, current: &AgentPm) -> bool {
    expected.pane == current.pane
        && expected.state.binding == current.state.binding
        && expected
            .state
            .current_generation()
            .and_then(|g| g.bound.as_ref())
            == current
                .state
                .current_generation()
                .and_then(|g| g.bound.as_ref())
}

/// Caller serializes outbox reposts; Inbox owns publication and its own lock. The durable
/// subject is the dedupe record if the executor dies before acknowledging the question.
fn publish(inbox: &Inbox<'_>, question: &AgentQuestion, project: &str) -> Result<Message> {
    if let Some(message) = inbox.messages()?.into_iter().find(|m| {
        m.kind == "question_wake"
            && m.subject
                .as_ref()
                .is_some_and(|s| s["key"] == question.wake.key)
    }) {
        return Ok(message);
    }
    let body = format!(
        "Question {} (revision {}) needs triage in project {project}.\n\
         Read the question by its exact key before acting. A wake grants no owner authority.",
        question.key, question.revision
    );
    inbox.post(Post {
        kind: "question_wake",
        urgency: Urgency::Wake,
        author: "corral",
        body: &body,
        binding: MessageBinding::default(),
        subject: Some(json!({
            "key": question.wake.key, "question": question.key,
            "revision": question.revision, "project": project,
        })),
        max_payload: super::delivery::INPUT_MAX_BYTES,
    })
}

/// The inbox outlives its PM binding. Check identity and every wake's project at the last
/// send boundary, under dispatch, so a rebind cannot deliver an old project's question to
/// its successor. The existing adapter still owns framing, retries and observation.
struct ScopedPm(AgentPm);

fn in_scope(ctx: &Ctx, message: &Message, project: &str) -> Result<bool> {
    if message.kind != "question_wake" {
        return Ok(true);
    }
    let subject = message
        .subject
        .as_ref()
        .context("question wake has no scope")?;
    if subject["project"] != project {
        return Ok(false);
    }
    let key = subject["question"]
        .as_str()
        .context("question wake has no key")?;
    let parsed = crate::questions::QuestionKey::parse(key)?;
    Ok(ctx.store.get(parsed.task())?.project.as_deref() == Some(project))
}

impl DeliveryAdapter for ScopedPm {
    fn harness(&self) -> &'static str {
        PiNativeAdapter.harness()
    }
    fn capabilities(&self) -> Capabilities {
        PiNativeAdapter.capabilities()
    }
    fn readiness(&self, ctx: &Ctx, recipient: &RecipientRef) -> Result<Readiness> {
        // Retire stale members before the delivery engine packs frozen bytes. Its existing
        // unattempted-envelope retirement also repacks publications made before replacement.
        let inbox = Inbox::new(&ctx.store, Recipient::Pm)?;
        let posting = inbox.lock()?;
        pm_state::update(&ctx.store, |pm| {
            let current = pm.as_ref().context("Pi-native PM is no longer bound")?;
            ensure!(
                healthy(current) && same_pm(&self.0, current),
                "PM binding changed"
            );
            for message in inbox.messages()? {
                if inbox
                    .receipt(message.seq, ReceiptLevel::Observed)?
                    .is_none()
                    && inbox.terminal(message.seq)?.is_none()
                    && !in_scope(ctx, &message, &current.state.binding.project)?
                {
                    inbox.mark_terminal_held(
                        &posting,
                        message.seq,
                        TerminalState::Cancelled,
                        "question wake is outside the current PM project",
                        PiNativeAdapter.harness(),
                    )?;
                }
            }
            Ok(((), false))
        })?;
        drop(posting);
        PiNativeAdapter.readiness(ctx, recipient)
    }
    fn bind(&self, ctx: &Ctx, recipient: &RecipientRef) -> Result<BindOutcome> {
        PiNativeAdapter.bind(ctx, recipient)
    }
    fn observe(
        &self,
        ctx: &Ctx,
        binding: &SessionBinding,
        cursor: &mut ObserveCursor,
    ) -> Result<Vec<Observation>> {
        PiNativeAdapter.observe(ctx, binding, cursor)
    }
    fn send(&self, ctx: &Ctx, binding: &SessionBinding, envelope: &Envelope) -> SendOutcome {
        let checked = pm_state::update(&ctx.store, |pm| {
            let current = pm.as_ref().context("Pi-native PM is no longer bound")?;
            ensure!(
                healthy(current) && same_pm(&self.0, current),
                "PM binding changed"
            );
            let BindOutcome::Bound(expected) = PiNativeAdapter.bind(ctx, &Recipient::Pm)? else {
                anyhow::bail!("PM session is no longer bound");
            };
            ensure!(*binding == expected, "PM delivery session changed");
            let inbox = Inbox::new(&ctx.store, Recipient::Pm)?;
            for seq in &envelope.members {
                let message = inbox.message(*seq)?;
                ensure!(
                    in_scope(ctx, &message, &current.state.binding.project)?,
                    "question wake is out of scope"
                );
            }
            Ok((PiNativeAdapter.send(ctx, binding, envelope), false))
        });
        match checked {
            Ok(outcome) => outcome,
            Err(error) => SendOutcome::Rejected {
                reason: format!("PM wake delivery refused: {error:#}"),
                retryable_after_fix: false,
            },
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::thread::JoinHandle;

    use corral_core::agent::{ContainerRef, QuestionState};
    use corral_core::inbox::ReceiptLevel;
    use serde_json::Value;

    use super::*;
    use crate::agent::BindTarget;
    use crate::agent::test_support::{self as ts, Fx};

    fn pm(fx: &Fx) -> (ts::Host, AgentPm) {
        let host = ts::bind(
            fx,
            &BindTarget::Pm {
                project: "corral".into(),
            },
            "pm-pane",
            "s-pm",
        );
        ts::committed(&ts::hello(fx, &host));
        (host, pm_state::load(&fx.ctx.store).unwrap().unwrap())
    }

    fn question(fx: &Fx) -> crate::questions::QuestionView {
        let id = ts::worker_task(fx, "worker");
        let host = ts::bind(
            fx,
            &BindTarget::Worker {
                task: id.clone(),
                run: "R1".into(),
            },
            "worker",
            "s-worker",
        );
        ts::committed(&ts::hello(fx, &host));
        let input = ts::envelope(
            &host,
            "ask",
            &crate::agent::tool_request_id("s-worker", "ask"),
            json!({"kind": "clarification", "blocking": true, "text": "Which scope?"}),
            false,
        );
        ts::committed(&ts::send(fx, &input, "ask"));
        crate::questions::views(&fx.ctx.store.get(&id).unwrap()).remove(0)
    }

    fn inbox(fx: &Fx) -> Inbox<'_> {
        Inbox::new(&fx.ctx.store, Recipient::Pm).unwrap()
    }

    fn current(fx: &Fx, view: &crate::questions::QuestionView) -> AgentQuestion {
        crate::questions::show(&fx.ctx, &view.question.key)
            .unwrap()
            .question
    }

    #[test]
    fn repost_deduplicates_a_published_wake_after_the_acknowledgement_is_lost() {
        // Given a durable PM message but no task acknowledgement, as after a crash.
        let fx = ts::fx();
        let (_, pm) = pm(&fx);
        let view = question(&fx);
        let inbox = inbox(&fx);
        publish(&inbox, &view.question, "corral").unwrap();
        let first = inbox.messages().unwrap().remove(0);
        assert_eq!(current(&fx, &view).wake.state, OutboxState::Pending);
        // When a fresh context replays and then sees the same stale snapshot again.
        let ctx = Ctx::from_store(fx.ctx.store.clone());
        assert!(post_one(&ctx, &inbox, &pm, &view).unwrap());
        assert!(!post_one(&ctx, &inbox, &pm, &view).unwrap());
        // Then the one logical message is reused and the exact outbox is acknowledged.
        assert_eq!(inbox.messages().unwrap(), vec![first.clone()]);
        let wake = current(&fx, &view).wake;
        assert_eq!(wake.state, OutboxState::Posted);
        assert_eq!(wake.message_id, Some(first.id));
        assert_eq!(first.subject.unwrap()["key"], view.question.wake.key);
    }

    #[test]
    fn stale_question_snapshot_never_acknowledges_a_new_transition() {
        // Given a snapshot whose question has since entered a new outbox transition.
        let fx = ts::fx();
        let (_, pm) = pm(&fx);
        let view = question(&fx);
        fx.ctx
            .store
            .update(&view.task, |task| {
                let state = agent::container_state_mut(
                    task,
                    &ContainerRef::Worker {
                        task: view.task.clone(),
                        run: "R1".into(),
                    },
                )
                .unwrap();
                let question = &mut state.questions[0];
                question.revision = 2;
                question.state = QuestionState::Escalated;
                question.wake.key = format!("qwake:{}:2", question.key);
                Ok(())
            })
            .unwrap();
        // When the old snapshot attempts to publish.
        assert!(!post_one(&fx.ctx, &inbox(&fx), &pm, &view).unwrap());
        // Then no message or acknowledgement is committed for the newer transition.
        assert!(inbox(&fx).messages().unwrap().is_empty());
        assert_eq!(current(&fx, &view).wake.state, OutboxState::Pending);
    }

    #[test]
    fn acknowledgement_refuses_scope_and_transition_changes_after_publication() {
        for change in 0..3 {
            // Given a published message whose outbox has not yet been acknowledged.
            let fx = ts::fx();
            let (_, pm) = pm(&fx);
            let view = question(&fx);
            let message = publish(&inbox(&fx), &view.question, "corral").unwrap();
            match change {
                0 => {
                    pm_state::update(&fx.ctx.store, |state| {
                        state.as_mut().unwrap().state.binding.project = "other".into();
                        Ok(((), true))
                    })
                    .unwrap();
                }
                1 => {
                    fx.ctx
                        .store
                        .update(&view.task, |task| {
                            task.project = Some("other".into());
                            Ok(())
                        })
                        .unwrap();
                }
                _ => {
                    fx.ctx
                        .store
                        .update(&view.task, |task| {
                            let question =
                                &mut task.runs[0].agent_host.as_mut().unwrap().questions[0];
                            question.revision = 2;
                            question.wake.key = format!("qwake:{}:2", question.key);
                            Ok(())
                        })
                        .unwrap();
                }
            }
            // When the old publication tries to acknowledge after the competing write.
            assert!(!acknowledge(&fx.ctx, &pm, &view, &message).unwrap());
            // Then only the stale message remains; no current transition is consumed.
            assert_eq!(inbox(&fx).messages().unwrap().len(), 1);
            assert_eq!(current(&fx, &view).wake.state, OutboxState::Pending);
        }
    }

    #[test]
    fn concurrent_reposter_defers_to_the_outbox_lock() {
        // Given another executor holding the keyed outbox serialization lock.
        let fx = ts::fx();
        pm(&fx);
        let view = question(&fx);
        let _lock = fx
            .ctx
            .store
            .try_lock("pm-question-wake-outbox")
            .unwrap()
            .unwrap();
        // When sync attempts to repost concurrently.
        assert_eq!(repost(&fx.ctx).unwrap(), 0);
        // Then the owning executor retains the pending transition without a duplicate.
        assert!(inbox(&fx).messages().unwrap().is_empty());
        assert_eq!(current(&fx, &view).wake.state, OutboxState::Pending);
    }

    #[test]
    fn missing_control_credentials_leave_the_published_wake_queued_for_recovery() {
        // Given a binding still recorded healthy whose control credential is gone.
        let fx = ts::fx();
        pm(&fx);
        let view = question(&fx);
        // When repeated sync passes publish and attempt delivery.
        assert_eq!(repost(&fx.ctx).unwrap(), 1);
        assert_eq!(repost(&fx.ctx).unwrap(), 0);
        // Then sync succeeds and the one message remains available to host recovery.
        assert_eq!(inbox(&fx).messages().unwrap().len(), 1);
        assert_eq!(current(&fx, &view).wake.state, OutboxState::Posted);
        assert!(
            inbox(&fx)
                .receipt(1, ReceiptLevel::Observed)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn changed_pm_identity_or_task_project_prevents_publication() {
        for change_pm in [true, false] {
            // Given a healthy PM and a question snapshotted inside its scope.
            let fx = ts::fx();
            let (_, pm) = pm(&fx);
            let view = question(&fx);
            if change_pm {
                pm_state::update(&fx.ctx.store, |state| {
                    state.as_mut().unwrap().state.binding.project = "other".into();
                    Ok(((), true))
                })
                .unwrap();
            } else {
                fx.ctx
                    .store
                    .update(&view.task, |task| {
                        task.project = Some("other".into());
                        Ok(())
                    })
                    .unwrap();
            }
            // When publication races the changed identity or project.
            assert!(!post_one(&fx.ctx, &inbox(&fx), &pm, &view).unwrap());
            // Then the shared PM inbox receives no wrong-project data.
            assert!(inbox(&fx).messages().unwrap().is_empty());
            assert_eq!(current(&fx, &view).wake.state, OutboxState::Pending);
        }
    }

    #[test]
    fn no_pm_preserves_omo_push_and_unattended_routes() {
        // Given no bound PM, including an explicitly enabled OmO question push.
        let mut fx = ts::fx();
        fx.ctx.store.config.pm.push_events = true;
        fx.ctx
            .store
            .config
            .pm
            .push_kinds
            .push("question_opened".into());
        assert_eq!(
            crate::questions::wake_route(&fx.ctx).unwrap(),
            QuestionWakeRoute::OmoPush
        );
        let view = question(&fx);
        // When the Pi wake sweep runs.
        assert_eq!(repost(&fx.ctx).unwrap(), 0);
        // Then it does not rewrite OmO routing or suppress unattended handling.
        assert_eq!(current(&fx, &view).wake.route, QuestionWakeRoute::OmoPush);
        assert!(inbox(&fx).messages().unwrap().is_empty());
        fx.ctx.store.config.pm.push_events = false;
        assert_eq!(
            crate::questions::wake_route(&fx.ctx).unwrap(),
            QuestionWakeRoute::None
        );
        let later = view.question.asked.at + jiff::SignedDuration::from_secs(601);
        crate::questions::unattended(&fx.ctx, &view.task, later).unwrap();
        assert!(current(&fx, &view).unattended_at.is_some());
    }

    #[test]
    fn replacement_generation_and_lost_pm_leave_the_outbox_pending() {
        // Given a healthy PM snapshot replaced by another generation in the same project.
        let fx = ts::fx();
        let (_, old) = pm(&fx);
        let view = question(&fx);
        let (_, current_pm) = pm(&fx);
        assert_ne!(
            old.state.binding.generation,
            current_pm.state.binding.generation
        );
        // When the stale snapshot posts, or a lost replacement gets a sync pass.
        assert!(!post_one(&fx.ctx, &inbox(&fx), &old, &view).unwrap());
        pm_state::update(&fx.ctx.store, |state| {
            state.as_mut().unwrap().state.binding.health = HostHealth::Lost;
            Ok(((), true))
        })
        .unwrap();
        assert_eq!(repost(&fx.ctx).unwrap(), 0);
        // Then no message is posted and no acknowledgement consumes the pending wake.
        assert!(inbox(&fx).messages().unwrap().is_empty());
        assert_eq!(current(&fx, &view).wake.state, OutboxState::Pending);
    }

    /// A private bridge fixture with the production delivery protocol, never a provider.
    struct Host {
        dir: PathBuf,
        socket: PathBuf,
        thread: Option<JoinHandle<Vec<Value>>>,
    }

    impl Host {
        fn start(fx: &Fx, host: &ts::Host) -> Self {
            use crate::agent::launch::{
                ensure_private_dir, generation_dir, runtime_root, write_ctl_cap,
            };
            let dir = generation_dir(&fx.ctx.store, &Recipient::Pm, 1).unwrap();
            ensure_private_dir(&runtime_root(), &dir).unwrap();
            write_ctl_cap(&dir, &host.ctl_cap).unwrap();
            let socket = dir.join("host.sock");
            let listener = UnixListener::bind(&socket).unwrap();
            std::fs::create_dir_all(&host.sessions).unwrap();
            let session_file = host.sessions.join("session.jsonl");
            let session = host.identity.session_id.clone();
            let thread = std::thread::spawn(move || {
                let mut submits = Vec::new();
                for stream in listener.incoming() {
                    let stream = stream.unwrap();
                    stream
                        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                        .unwrap();
                    let mut writer = stream.try_clone().unwrap();
                    for line in BufReader::new(stream).lines() {
                        let frame: Value = serde_json::from_str(&line.unwrap()).unwrap();
                        if frame["stop"] == true {
                            return submits;
                        }
                        let result = match frame["command"].as_str().unwrap() {
                            "hello" => json!({"bound": true}),
                            "submit_input" => {
                                let args = &frame["args"];
                                submits.push(args.clone());
                                let entry = json!({"type":"custom", "customType":"corral_delivery",
                                    "id":"e1", "timestamp":"2026-10-25T00:00:00Z",
                                    "data":{"version":1, "idempotency_key":args["idempotency_key"],
                                        "request_id":frame["id"], "origin":args["origin"],
                                        "busy_policy":args["busy_policy"], "generation":1,
                                        "session_id":session,
                                        "text_sha256":agent::paths::sha256_hex(args["text"].as_str().unwrap().as_bytes()),
                                        "text_bytes":args["text"].as_str().unwrap().len()}});
                                let mut file = std::fs::OpenOptions::new()
                                    .create(true)
                                    .append(true)
                                    .open(&session_file)
                                    .unwrap();
                                writeln!(file, "{entry}").unwrap();
                                json!({"request_id":frame["id"], "idempotency_key":args["idempotency_key"],
                                    "disposition":"started", "turn":1, "replayed":false})
                            }
                            other => panic!("unexpected bridge command {other}"),
                        };
                        writeln!(
                            writer,
                            "{}",
                            json!({"version":1, "id":frame["id"],
                            "generation":1, "ok":true, "result":result})
                        )
                        .unwrap();
                    }
                }
                submits
            });
            Self {
                dir,
                socket,
                thread: Some(thread),
            }
        }

        fn stop(&mut self) -> Vec<Value> {
            writeln!(
                UnixStream::connect(&self.socket).unwrap(),
                "{{\"stop\":true}}"
            )
            .unwrap();
            self.thread.take().unwrap().join().unwrap()
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            if self.thread.is_some() {
                self.stop();
            }
            std::fs::remove_dir_all(&self.dir).unwrap();
        }
    }

    #[test]
    fn sync_repost_starts_one_pm_turn_and_observes_the_delivery() {
        // Given an idle PM answering through a private, provider-free bridge.
        let fx = ts::fx();
        let (host, _) = pm(&fx);
        let view = question(&fx);
        let mut bridge = Host::start(&fx, &host);
        // When repeated sync passes repost and deliver the question wake.
        assert_eq!(repost(&fx.ctx).unwrap(), 1);
        assert_eq!(repost(&fx.ctx).unwrap(), 0);
        // Then one submit_input starts a turn and the adapter observes its delivery entry.
        let submits = bridge.stop();
        assert_eq!(submits.len(), 1);
        assert_eq!(submits[0]["busy_policy"], "follow_up");
        assert!(
            submits[0]["text"]
                .as_str()
                .unwrap()
                .contains(&view.question.key)
        );
        let inbox = inbox(&fx);
        let envelope = inbox.envelopes().unwrap().remove(0);
        assert_eq!(envelope.adapter, super::super::delivery::ADAPTER);
        let SendOutcome::Admitted { detail } = envelope.outcome.unwrap() else {
            panic!("wake did not start the turn")
        };
        assert_eq!(detail["disposition"], "started");
        assert_eq!(detail["turn"], 1);
        assert!(inbox.receipt(1, ReceiptLevel::Observed).unwrap().is_some());
    }

    #[test]
    fn stale_wake_does_not_consume_a_current_wake_or_recovery_notice() {
        for frozen in [false, true] {
            // Given a foreign wake sharing the durable inbox with current work and recovery.
            let fx = ts::fx();
            let (host, _) = pm(&fx);
            let old = question(&fx);
            let inbox = inbox(&fx);
            let stale = publish(&inbox, &old.question, "corral").unwrap();
            fx.ctx
                .store
                .update(&old.task, |task| {
                    task.project = Some("other".into());
                    Ok(())
                })
                .unwrap();
            let view = question(&fx);
            let valid = publish(&inbox, &view.question, "corral").unwrap();
            let notice = inbox
                .post(Post {
                    kind: "recovery_notice",
                    urgency: Urgency::Wake,
                    author: "corral",
                    body: "Resume the current PM generation.",
                    binding: MessageBinding::default(),
                    subject: None,
                    max_payload: super::super::delivery::INPUT_MAX_BYTES,
                })
                .unwrap();
            let mut bridge = Host::start(&fx, &host);
            if frozen {
                let BindOutcome::Bound(binding) =
                    PiNativeAdapter.bind(&fx.ctx, &Recipient::Pm).unwrap()
                else {
                    panic!("fixture unbound")
                };
                inbox
                    .publish(
                        &binding,
                        PiNativeAdapter.harness(),
                        PiNativeAdapter.capabilities().max_payload,
                        false,
                    )
                    .unwrap()
                    .unwrap();
            }
            // When sync handles either queued messages or an unattempted frozen envelope.
            assert_eq!(repost(&fx.ctx).unwrap(), 1);
            assert_eq!(repost(&fx.ctx).unwrap(), 0);
            // Then only stale work is retired; the current wake and notice start one turn.
            let submits = bridge.stop();
            assert_eq!(submits.len(), 1);
            let text = submits[0]["text"].as_str().unwrap();
            assert!(!text.contains(&old.question.key));
            assert!(text.contains(&view.question.key));
            assert!(text.find("recovery_notice").unwrap() < text.find("question_wake").unwrap());
            assert!(inbox.terminal(stale.seq).unwrap().is_some());
            for message in [valid, notice] {
                assert!(
                    inbox
                        .receipt(message.seq, ReceiptLevel::Observed)
                        .unwrap()
                        .is_some()
                );
                assert!(inbox.terminal(message.seq).unwrap().is_none());
            }
            assert_eq!(current(&fx, &view).wake.state, OutboxState::Posted);
        }
    }

    #[test]
    fn pending_wake_is_not_delivered_after_pm_scope_replacement() {
        // Given a queued question for the old scope and a new PM project binding.
        let fx = ts::fx();
        let (host, pm) = pm(&fx);
        let view = question(&fx);
        assert!(post_one(&fx.ctx, &inbox(&fx), &pm, &view).unwrap());
        pm_state::update(&fx.ctx.store, |state| {
            state.as_mut().unwrap().state.binding.project = "other".into();
            Ok(((), true))
        })
        .unwrap();
        let mut bridge = Host::start(&fx, &host);
        // When the new PM's sync pass tries to drain the shared inbox.
        assert_eq!(repost(&fx.ctx).unwrap(), 0);
        // Then no question bytes cross the bridge into the wrong project.
        assert!(bridge.stop().is_empty());
        assert_eq!(inbox(&fx).messages().unwrap().len(), 1);
    }
}
