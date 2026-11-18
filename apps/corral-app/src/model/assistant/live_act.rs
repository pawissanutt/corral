//! Daemon-mode reducers. Every chat control returns the canonical send, session operation,
//! prompt answer or engine operation as an `Effect`; nothing completes in this store. An
//! operation's identity is allocated once per intent and kept.

use std::collections::{BTreeMap, BTreeSet};

use corral_protocol::assistant::{
    self as wire, AssistantSend, AssistantSessionOp, AssistantStop, AssistantWarm, ContextRef,
    MESSAGE_TEXT_MAX, PromptAnswer, PromptChoice,
};
use corral_protocol::rows::AssistantRow;
use corral_types::{NewTask, Op, Status, TaskEdit};

use super::{Action, State, live};
use crate::model::act::{self, Act, ActStatus};
use crate::model::daemon::Feed;
use crate::model::effect::{Effect, Env};
use crate::model::id::{Id, IdSupply};
use crate::model::live::Live;
use crate::model::nav::{AskTarget, Form};

/// The shared quick thread's scope; named sessions are scoped by their A-n id.
pub const QUICK: &str = wire::QUICK_THREAD;

/// A profile change sent for a thread, shown until the thread's row reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub profile: String,
    /// The `assistant.session` operation carrying it.
    pub key: Id,
    /// corrald's refusal, once the operation replied with one.
    pub failed: Option<String>,
}

/// An End sent for a named session, shown until its row reports it ended or corrald refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ending {
    /// The `assistant.session` operation carrying it.
    pub key: Id,
    /// corrald's refusal, once the operation replied with one.
    pub failed: Option<String>,
}

/// The reply to an `assistant.session` operation, as the app reads it by the operation's key.
#[derive(Debug, Clone, PartialEq)]
pub enum OpOutcome {
    /// No reply yet.
    Pending,
    /// Refused or lost.
    Failed(String),
    /// The session row it created or changed.
    Row(AssistantRow),
}

/// Daemon-mode requests in flight and edits in progress. Thread content itself lives in the
/// feed; this holds only what the owner asked for and is waiting on.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LiveState {
    /// Profile changes by thread scope.
    pub picks: BTreeMap<String, Pick>,
    /// The `assistant.session` New operation, until it replies.
    pub new_session: Option<Id>,
    /// Why the last New was not started.
    pub new_session_failed: Option<String>,
    /// Turns asked to stop, by session.
    pub stopping: BTreeMap<String, u64>,
    /// Stop request correlation and exact turn; acceptance alone never proves an abort.
    pub stop_ops: BTreeMap<Id, (String, u64)>,
    /// The named session whose End is being confirmed on this device.
    pub confirm_end: Option<String>,
    /// Ends sent, by session.
    pub ends: BTreeMap<String, Ending>,
    /// Operations sent from thread cards, by subject (`file:<entry>`, `discard:<task>`, …).
    pub acts: BTreeMap<String, Id>,
    /// The filed Idea whose title is being edited.
    pub editing: Option<String>,
    /// Open thread views by scope; the subscription closes with the last one.
    pub subs: BTreeMap<String, u32>,
    /// Threads already asked to warm their host in this app session.
    pub warmed: BTreeSet<String>,
    /// The saved draft of each scope whose words went out as a message the client took: until
    /// the client retires (or something replaces) that saved draft, no composer of the scope,
    /// including one mounted meanwhile, brings it back.
    pub handed: BTreeMap<String, String>,
    /// The attachment whose readback is open under its message: (entry, attachment id).
    pub attachment: Option<(String, String)>,
}

impl LiveState {
    /// Session operations still waiting for their reply.
    pub fn waiting_ops(&self) -> Vec<Id> {
        self.picks
            .values()
            .filter(|pick| pick.failed.is_none())
            .map(|pick| pick.key.clone())
            .chain(
                self.ends
                    .values()
                    .filter(|end| end.failed.is_none())
                    .map(|end| end.key.clone()),
            )
            .chain(self.new_session.clone())
            .collect()
    }
}

/// One send intent's identity and typing time, allocated once by the composer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    pub cmd_id: String,
    /// RFC 3339 time the owner pressed Send.
    pub typed_at: String,
}

/// The subject a card command is remembered under.
pub fn subject(kind: &str, id: &str) -> String {
    format!("{kind}:{id}")
}

impl State {
    /// A failed Stop releases only its own turn, so the current answer can be retried.
    pub fn stop_failed(&mut self, session: &str, turn: u64) {
        if self.live.stopping.get(session) == Some(&turn) {
            self.live.stopping.remove(session);
        }
    }

    pub fn settle_stop(
        &mut self,
        key: &Id,
        operation: &crate::model::daemon::Operation,
    ) -> Vec<Effect> {
        use corral_protocol::assistant::{StopAck, StopOutcome};
        if operation.loading {
            return Vec::new();
        }
        let Some((session, turn)) = self.live.stop_ops.remove(key) else {
            return Vec::new();
        };
        let failure = if let Some(error) = &operation.error {
            Some(error.message.clone())
        } else {
            match operation.decode::<StopAck>() {
                Some(ack) if ack.session == session && ack.turn == turn => match ack.outcome {
                    StopOutcome::Accepted | StopOutcome::AlreadyStopped => None,
                    StopOutcome::Failed | StopOutcome::Other => Some(
                        ack.reason
                            .unwrap_or_else(|| "Stop was not accepted.".into()),
                    ),
                },
                _ => Some("Stop replied without a matching acknowledgement.".into()),
            }
        };
        if let Some(why) = failure {
            self.stop_failed(&session, turn);
            vec![Effect::Toast(why)]
        } else {
            Vec::new()
        }
    }

    /// Records a session operation's outcome, opening a created session once it arrives.
    pub fn settle(&mut self, key: &Id, outcome: OpOutcome) -> Vec<Effect> {
        if self.live.new_session.as_ref() == Some(key) {
            return match outcome {
                OpOutcome::Pending => Vec::new(),
                OpOutcome::Failed(why) => {
                    self.live.new_session = None;
                    self.live.new_session_failed = Some(why);
                    Vec::new()
                }
                OpOutcome::Row(row) => {
                    self.live.new_session = None;
                    vec![Effect::OpenLive(Live::Thread(Id::from(row.id)))]
                }
            };
        }
        if let OpOutcome::Failed(why) = outcome {
            if let Some(pick) = self.live.picks.values_mut().find(|pick| pick.key == *key)
                && pick.failed.as_ref() != Some(&why)
            {
                pick.failed = Some(why);
            } else if let Some(end) = self.live.ends.values_mut().find(|end| end.key == *key)
                && end.failed.as_ref() != Some(&why)
            {
                end.failed = Some(why);
            }
        }
        Vec::new()
    }

    /// A thread view opened: the first view of a scope opens its subscription, and Ask warms a
    /// cold quick-thread host so the first answer does not wait for it.
    pub fn subscribe(&mut self, scope: &str, warm: bool, feed: &Feed) -> Vec<Effect> {
        let count = self.live.subs.entry(scope.to_owned()).or_insert(0);
        *count += 1;
        let mut effects = Vec::new();
        if *count == 1 {
            effects.push(Effect::Thread {
                scope: scope.to_owned(),
                open: true,
            });
        }
        let cold = live::row(feed, scope)
            .is_none_or(|row| row.state == corral_protocol::rows::AssistantState::Cold);
        if warm && cold && self.live.warmed.insert(scope.to_owned()) {
            effects.push(Effect::Warm(AssistantWarm {
                thread: scope.to_owned(),
            }));
        }
        effects
    }

    /// A thread view closed; the last one closes the subscription.
    pub fn unsubscribe(&mut self, scope: &str) -> Vec<Effect> {
        let Some(count) = self.live.subs.get_mut(scope) else {
            return Vec::new();
        };
        *count = count.saturating_sub(1);
        if *count > 0 {
            return Vec::new();
        }
        self.live.subs.remove(scope);
        vec![Effect::Thread {
            scope: scope.to_owned(),
            open: false,
        }]
    }

    /// Builds the message for `scope` (the quick thread with Ask's target, or a named session),
    /// with the refs of what is staged, plus the effects that follow once it is handed over.
    /// `Err` holds the refusal and nothing is sent. The app clears the draft only when the root
    /// actually handed the message to the client (which keeps it in the device outbox offline).
    /// The text is the owner's words without their surrounding blank space.
    #[allow(clippy::too_many_arguments)]
    pub fn send_live(
        &mut self,
        scope: &str,
        target: AskTarget,
        text: &str,
        context: Option<ContextRef>,
        attachments: Vec<wire::SendAttachment>,
        stamp: Stamp,
        docked: bool,
        env: Env,
    ) -> Result<(AssistantSend, Vec<Effect>), Vec<Effect>> {
        let words = text.trim();
        if words.is_empty() {
            return Err(Vec::new());
        }
        if words.len() > MESSAGE_TEXT_MAX {
            return Err(vec![Effect::Toast(format!(
                "Not sent: {} bytes is over the {MESSAGE_TEXT_MAX}-byte message limit",
                words.len()
            ))]);
        }
        let quick = scope == QUICK;
        let to_pm = quick && target == AskTarget::Pm;
        if to_pm && !attachments.is_empty() {
            return Err(vec![Effect::Toast(
                "Not sent: PM messages carry no attachments. Remove them or switch to Assistant."
                    .into(),
            )]);
        }
        self.model_menu = false;
        let message = AssistantSend {
            cmd_id: stamp.cmd_id,
            thread: scope.to_owned(),
            target: if to_pm {
                wire::AskTarget::Pm
            } else {
                wire::AskTarget::Assistant
            },
            text: words.to_owned(),
            typed_at: stamp.typed_at,
            context: context.into_iter().collect(),
            attachments,
        };
        let mut after = Vec::new();
        if env.form == Form::Phone && !docked {
            after.push(Effect::CloseAsk);
            after.push(Effect::OpenLive(Live::Thread(Id::from(QUICK))));
        }
        Ok((message, after))
    }

    /// The client took the message for `scope` while `saved` was its saved draft; the client
    /// retires that draft in the same durable write as the message.
    pub fn handed_off(&mut self, scope: &str, saved: String) {
        self.live.handed.insert(scope.to_owned(), saved);
    }

    /// Whether `saved` is the scope's draft whose words were already sent, so it must not be
    /// restored; once the saved draft is gone or different, the handoff is over.
    pub fn already_sent(&mut self, scope: &str, saved: Option<&str>) -> bool {
        match (self.live.handed.get(scope), saved) {
            (Some(sent), Some(saved)) if sent == saved => true,
            (Some(_), _) => {
                self.live.handed.remove(scope);
                false
            }
            (None, _) => false,
        }
    }

    /// Selects the next configured profile for the thread the keys address.
    pub fn cycle_model_live(
        &mut self,
        top: Option<&Live>,
        ask_open: bool,
        feed: &Feed,
        _env: Env,
    ) -> Vec<Effect> {
        let scope = match top {
            Some(Live::Thread(id)) if !ask_open => id.to_string(),
            _ => QUICK.to_owned(),
        };
        let models = live::models(feed);
        if models.is_empty() || !live::pickable(feed, &scope) {
            return Vec::new();
        }
        let current = live::profile(self, feed, &scope);
        let next = models
            .iter()
            .position(|model| Some(model.name.as_str()) == current.as_deref())
            .map_or(0, |at| (at + 1) % models.len());
        let profile = models[next].name.clone();
        self.act_live(
            Action::PickProfile {
                thread: scope,
                profile,
            },
            feed,
        )
    }

    /// Runs one daemon-mode action from any builder.
    pub fn act_live(&mut self, action: Action, feed: &Feed) -> Vec<Effect> {
        if matches!(
            &action,
            Action::NewSession
                | Action::ConfirmEnd(_)
                | Action::PickProfile { .. }
                | Action::Stop { .. }
        ) && !feed.ready()
        {
            self.live.confirm_end = None;
            return vec![Effect::Toast(
                "Not sent: needs an authenticated, synchronized connection to corrald.".into(),
            )];
        }
        match action {
            Action::ToggleModelMenu => {
                self.model_menu = !self.model_menu;
                Vec::new()
            }
            Action::CloseAsk => {
                self.model_menu = false;
                vec![Effect::CloseAsk]
            }
            Action::OpenQuickThread => {
                self.clear_anchor();
                vec![
                    Effect::CloseAsk,
                    Effect::OpenLive(Live::Thread(Id::from(QUICK))),
                ]
            }
            Action::OpenLive(live @ Live::Thread(_)) => {
                self.clear_anchor();
                vec![Effect::CloseAsk, Effect::OpenLive(live)]
            }
            Action::OpenLive(live) => vec![Effect::OpenLive(live)],
            Action::OpenPrompt {
                thread: super::ThreadRef::Live(scope),
                key,
            } => {
                self.anchor = Some((super::ThreadRef::Live(scope.clone()), key));
                vec![Effect::CloseAsk, Effect::OpenLive(Live::Thread(scope))]
            }
            Action::PickProfile { thread, profile } => self.pick(thread, profile, feed),
            Action::Stop { session, turn } => {
                self.live.stopping.insert(session.clone(), turn);
                vec![Effect::Stop(AssistantStop { session, turn })]
            }
            Action::EndSession(session) => {
                self.live.confirm_end = session;
                Vec::new()
            }
            Action::ConfirmEnd(session) => self.end(session, feed),
            Action::NewSession => {
                if self.live.new_session.is_some() {
                    return Vec::new();
                }
                let key = self.ids.next_id();
                self.live.new_session = Some(key.clone());
                self.live.new_session_failed = None;
                vec![Effect::Session {
                    key,
                    op: AssistantSessionOp::New {
                        task: None,
                        profile: None,
                    },
                }]
            }
            Action::FileEntry(entry) => match idea_from(feed, &entry) {
                Some(task) => self.once(subject("file", &entry), feed, |ids| {
                    Act::new(ids, Op::New { task }, false)
                }),
                None => vec![Effect::Toast(
                    "This message is not loaded here; nothing was filed".into(),
                )],
            },
            Action::Answer { key, rev, choice } => self.answer(key, rev, choice, feed),
            Action::EditIdea(Some(task)) => {
                self.live.editing = Some(task);
                Vec::new()
            }
            Action::EditIdea(None) => {
                self.live.editing = None;
                Vec::new()
            }
            Action::SaveIdea { task, title } => self.save_title(task, title, feed),
            Action::DiscardIdea(task) => self.once(subject("discard", &task), feed, |ids| {
                Act::new(ids, Op::Drop { id: task }, false)
            }),
            Action::RetryThread(scope) => vec![Effect::Thread { scope, open: true }],
            Action::Unattach(key) => vec![Effect::Unattach(Id::from(key))],
            Action::CancelSend(key) => vec![Effect::CancelSend(Id::from(key))],
            Action::OpenAttachment { entry, id } => self.open_attachment(entry, id, feed),
            // Picking happens at the app root, inside the press, so a browser lets it open.
            Action::Attach(_) | Action::AttachLink => Vec::new(),
            // Unchip clears the app-owned context; Send goes through `send_live`.
            Action::Unchip | Action::UnchipProject | Action::Send => Vec::new(),
        }
    }

    fn pick(&mut self, thread: String, profile: String, feed: &Feed) -> Vec<Effect> {
        self.model_menu = false;
        if live::profile(self, feed, &thread).as_deref() == Some(profile.as_str()) {
            return Vec::new();
        }
        let key = self.ids.next_id();
        self.live.picks.insert(
            thread.clone(),
            Pick {
                profile: profile.clone(),
                key: key.clone(),
                failed: None,
            },
        );
        vec![Effect::Session {
            key,
            op: AssistantSessionOp::SetProfile { thread, profile },
        }]
    }

    /// Ends a session for good once the owner confirmed it on this device: one
    /// `assistant.session` End, never twice while one waits. The quick thread ends its current
    /// generation; corrald starts the next one with the next quick message.
    fn end(&mut self, scope: String, feed: &Feed) -> Vec<Effect> {
        self.live.confirm_end = None;
        let waiting = matches!(
            live::end_state(self, feed, &scope),
            Some(live::EndState::Pending)
        );
        let Some(session) = live::end_target(feed, &scope) else {
            return Vec::new();
        };
        if live::ended(feed, &session) || waiting {
            return Vec::new();
        }
        let key = self.ids.next_id();
        self.live.ends.insert(
            session.clone(),
            Ending {
                key: key.clone(),
                failed: None,
            },
        );
        vec![Effect::Session {
            key,
            op: AssistantSessionOp::End { session },
        }]
    }

    fn answer(&mut self, key: String, rev: u32, choice: PromptChoice, feed: &Feed) -> Vec<Effect> {
        let remembered = subject("prompt", &key);
        if let Some(effects) = self.held(&remembered, feed) {
            return effects;
        }
        let cmd_id = self.ids.next_id();
        self.live.acts.insert(remembered, cmd_id.clone());
        vec![Effect::Prompt {
            title: format!("{} · {key}", live::choice_label(choice)),
            answer: PromptAnswer {
                cmd_id: cmd_id.to_string(),
                key,
                rev,
                choice,
            },
        }]
    }

    fn save_title(&mut self, task: String, title: String, feed: &Feed) -> Vec<Effect> {
        if self.live.editing.as_deref() != Some(task.as_str()) {
            return Vec::new();
        }
        let title = title.trim().to_owned();
        if title.is_empty() {
            return vec![Effect::Toast(
                "A title cannot be empty; nothing was sent".into(),
            )];
        }
        let effects = self.once(subject("edit", &task), feed, |ids| {
            Act::new(
                ids,
                Op::Edit {
                    id: task,
                    edit: TaskEdit {
                        title: Some(title),
                        ..TaskEdit::default()
                    },
                },
                false,
            )
        });
        if effects
            .iter()
            .any(|effect| matches!(effect, Effect::Act(_)))
        {
            self.live.editing = None;
        }
        effects
    }

    /// Sends a card operation once: a settled failure may be sent again under a new identity, an
    /// in-flight or done one is left alone.
    pub(super) fn once(
        &mut self,
        remembered: String,
        feed: &Feed,
        make: impl FnOnce(&mut IdSupply) -> Act,
    ) -> Vec<Effect> {
        if let Some(effects) = self.held(&remembered, feed) {
            return effects;
        }
        let act = make(&mut self.ids);
        self.live.acts.insert(remembered, act.key.clone());
        vec![Effect::Act(act)]
    }

    fn held(&self, remembered: &str, feed: &Feed) -> Option<Vec<Effect>> {
        let key = self.live.acts.get(remembered)?;
        match act::status(feed, key.as_str()) {
            // Never submitted (a confirmation the owner cancelled) or settled without running.
            None | Some(ActStatus::Failed(_) | ActStatus::Refused(_)) => None,
            // In flight or done.
            Some(ActStatus::Pending | ActStatus::Running | ActStatus::Done) => Some(Vec::new()),
        }
    }
}

/// A waiting message filed as an Idea in its owner's words: the first line, cut at 120
/// characters, as the title, and the whole message as the body.
fn idea_from(feed: &Feed, entry: &str) -> Option<NewTask> {
    let words = feed
        .threads
        .values()
        .find_map(|thread| thread.entries.get(entry))?
        .text
        .clone()?;
    let title = super::saved_as_written(entry, &words).title;
    Some(NewTask {
        title,
        body: words,
        status: Status::Idea,
        ..NewTask::default()
    })
}
