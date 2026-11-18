//! Daemon-mode reducers. Every chat control returns the canonical send, session operation,
//! session control, question answer or engine operation as an `Effect`; nothing completes in
//! this store. An operation's identity is allocated once per intent and kept.

use std::collections::{BTreeMap, BTreeSet};

use corral_protocol::assistant::{
    self as wire, AssistantSend, AssistantSessionOp, AssistantWarm, ContextRef, MESSAGE_TEXT_MAX,
};
use corral_protocol::rows::AssistantRow;
use corral_types::{NewTask, Op, Status};

use super::{Action, State, live};
use crate::model::act::{self, Act, ActStatus};
use crate::model::daemon::Feed;
use crate::model::effect::{Effect, Env};
use crate::model::id::Id;
use crate::model::live::Live;
use crate::model::nav::Form;

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
    /// The named session whose End is being confirmed on this device.
    pub confirm_end: Option<String>,
    /// Ends sent, by session.
    pub ends: BTreeMap<String, Ending>,
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
    /// Owner messages filed as an Idea from here: entry → the engine operation's key.
    pub filed: BTreeMap<String, Id>,
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

impl State {
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
        self.model_menu = false;
        let message = AssistantSend {
            cmd_id: stamp.cmd_id,
            thread: scope.to_owned(),
            target: wire::AskTarget::Assistant,
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
                | Action::Control { .. }
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
            Action::OpenQuickThread => vec![
                Effect::CloseAsk,
                Effect::OpenLive(Live::Thread(Id::from(QUICK))),
            ],
            Action::OpenLive(live @ Live::Thread(_)) => {
                vec![Effect::CloseAsk, Effect::OpenLive(live)]
            }
            Action::OpenLive(live) => vec![Effect::OpenLive(live)],
            Action::PickProfile { thread, profile } => self.pick(thread, profile, feed),
            Action::Control { session, control } => vec![Effect::Control {
                session: Id::from(session),
                key: self.ids.next_id(),
                control,
                confirmed: false,
            }],
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
            Action::AnswerQuestion {
                session,
                question,
                option,
                text,
            } => crate::model::agents::answer_prompt(&session, question, option, text)
                .into_iter()
                .collect(),
            Action::FileEntry(entry) => self.file(entry, feed),
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

    /// Files an owner message as an Idea in its words, with no model. Sent once while the
    /// filing is in flight or done; a refused or failed one may be sent again.
    fn file(&mut self, entry: String, feed: &Feed) -> Vec<Effect> {
        let sent = self
            .live
            .filed
            .get(&entry)
            .and_then(|key| act::status(feed, key.as_str()));
        if sent
            .is_some_and(|status| !matches!(status, ActStatus::Failed(_) | ActStatus::Refused(_)))
        {
            return Vec::new();
        }
        let Some(task) = idea_from(feed, &entry) else {
            return vec![Effect::Toast(
                "This message is not loaded here; nothing was filed".into(),
            )];
        };
        let act = Act::new(&mut self.ids, Op::New { task }, false);
        self.live.filed.insert(entry, act.key.clone());
        vec![Effect::Act(act)]
    }
}

/// A message filed as an Idea in its owner's words: the first non-blank line, cut at 120
/// characters, as the title, and the whole message as the body.
fn idea_from(feed: &Feed, entry: &str) -> Option<NewTask> {
    let words = feed
        .threads
        .values()
        .find_map(|thread| thread.entries.get(entry))?
        .text
        .clone()?;
    let title = words
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?
        .chars()
        .take(120)
        .collect();
    Some(NewTask {
        title,
        body: words,
        status: Status::Idea,
        ..NewTask::default()
    })
}
