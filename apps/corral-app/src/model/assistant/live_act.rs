//! Daemon-mode reducers for the chat operations corrald answers: a new chat, ending one, the
//! profile of the next message, and warming Ask's chat as it opens. Each goes out as an
//! `Effect`; nothing completes in this store. An operation's identity is allocated once per
//! intent and kept. Messages are not here: Ask sends through `model::session` like every session.

use std::collections::{BTreeMap, BTreeSet};

use corral_protocol::assistant::{self as wire, AssistantSessionOp, AssistantWarm};
use corral_protocol::rows::{AssistantRow, AssistantState};

use super::{Action, State, live};
use crate::model::daemon::Feed;
use crate::model::effect::{Effect, Env};
use crate::model::id::Id;
use crate::model::live::Live;

/// The scope that resolves to the newest quick chat that has not ended.
pub const QUICK: &str = wire::QUICK_THREAD;

/// A profile change sent for a chat, shown until the chat's row reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub profile: String,
    /// The `assistant.session` operation carrying it.
    pub key: Id,
    /// corrald's refusal, once the operation replied with one.
    pub failed: Option<String>,
}

/// An End sent for a chat, shown until its row reports it ended or corrald refuses.
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
    /// The chat row it created or changed.
    Row(AssistantRow),
}

/// Daemon-mode requests in flight. The conversation lives in the feed and the session state;
/// this holds only what the owner asked for and is waiting on.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LiveState {
    /// Profile changes by chat scope.
    pub picks: BTreeMap<String, Pick>,
    /// The `assistant.session` New operation, until it replies.
    pub new_session: Option<Id>,
    /// Why the last New was not started.
    pub new_session_failed: Option<String>,
    /// Ends sent, by chat.
    pub ends: BTreeMap<String, Ending>,
    /// Chats already asked to warm their host in this app session.
    pub warmed: BTreeSet<String>,
    /// The attachment whose readback is open under its message: (entry, attachment id).
    pub attachment: Option<(String, String)>,
}

impl LiveState {
    /// Chat operations still waiting for their reply.
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

impl State {
    /// Records a chat operation's outcome, opening a created chat once it arrives.
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
                    vec![Effect::OpenLive(Live::Session(Id::from(row.id)))]
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

    /// Ask opened on `scope`: a cold chat's host is asked to start once, so the first answer
    /// does not wait for it. A scope with no chat yet has nothing to warm; its first send starts
    /// one.
    pub fn warm(&mut self, scope: &str, feed: &Feed) -> Vec<Effect> {
        let cold = live::row(feed, scope).is_some_and(|row| row.state == AssistantState::Cold);
        if !cold || !feed.ready() || !self.live.warmed.insert(scope.to_owned()) {
            return Vec::new();
        }
        vec![Effect::Warm(AssistantWarm {
            thread: scope.to_owned(),
        })]
    }

    /// Selects the next configured profile for the chat the keys address: the chat page on top
    /// unless Ask is open, else Ask's.
    pub fn cycle_model_live(
        &mut self,
        top: Option<&Live>,
        ask_open: bool,
        feed: &Feed,
        _env: Env,
    ) -> Vec<Effect> {
        let scope = match top {
            Some(Live::Session(id)) if !ask_open && (*id == QUICK || id.starts_with("A-")) => {
                id.to_string()
            }
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
            Action::NewSession | Action::ConfirmEnd(_) | Action::PickProfile { .. }
        ) && !feed.ready()
        {
            return vec![Effect::Toast(
                "Not sent: needs an authenticated, synchronized connection to corrald.".into(),
            )];
        }
        match action {
            Action::CloseAsk => vec![Effect::CloseAsk],
            Action::OpenLive(live @ Live::Session(_)) => {
                vec![Effect::CloseAsk, Effect::OpenLive(live)]
            }
            Action::OpenLive(live) => vec![Effect::OpenLive(live)],
            Action::PickProfile { thread, profile } => self.pick(thread, profile, feed),
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
            Action::Unattach(key) => vec![Effect::Unattach(Id::from(key))],
            Action::OpenAttachment { entry, id } => self.open_attachment(entry, id, feed),
            // Picking happens at the app root, inside the press, so a browser lets it open;
            // the chips are the app's own state.
            Action::Attach(_) | Action::AttachLink | Action::Unchip | Action::UnchipProject => {
                Vec::new()
            }
        }
    }

    fn pick(&mut self, thread: String, profile: String, feed: &Feed) -> Vec<Effect> {
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

    /// Ends a chat for good once the owner confirmed it on this device: one `assistant.session`
    /// End, never twice while one waits. It stays readable as its `A-n`; Ask's next message
    /// starts a new chat.
    fn end(&mut self, scope: String, feed: &Feed) -> Vec<Effect> {
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
}
