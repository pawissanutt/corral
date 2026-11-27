//! What the session page and its parts do, and the reducer over the per-session UI state: tabs,
//! opened rows, transcript pages, the draft and sends, the receiver and run controls; the dock's
//! cards and queue, the sidebar and each tab's part, each handled by its own module. Every
//! host or engine operation goes out as an effect; nothing is simulated here.

use corral_protocol::assistant::SendAttachment;
use corral_protocol::detail::{DetailGet, DetailRef};
use corral_protocol::session_host::SessionSend;
use corral_protocol::session_state::SendPolicy;

use crate::model::agents::Control;
use crate::model::assistant::{self, EndState};
use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::hue::Hue;
use crate::model::work::Record;
use crate::model::{Id, Live};

use super::board::BoardAction;
use super::changes::ChangesAction;
use super::dock::DockAction;
use super::proof::ProofAction;
use super::sidebar::SidebarAction;
use super::task::TaskAction;
use super::usage::UsageAction;
use super::{
    EMPTY_UI, EchoState, Focus, Sent, State, SubView, Tab, Ui, board, changes, dock, echo, proof,
    sidebar, tabs, task, transcript, usage,
};

/// What the session page and its parts do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Show a tab; leaving or opening Chat marks the rows read so far.
    Tab {
        session: Id,
        tab: Tab,
    },
    /// Show a view inside a tab (Usage under Chat, a child under Agents), or none.
    Sub {
        session: Id,
        sub: Option<SubView>,
    },
    Focus {
        session: Id,
        focus: Focus,
    },
    /// Open or fold a transcript row by its stable key. Opening a row whose complete content is
    /// not here yet also fetches it, by its continuation token.
    Expand {
        session: Id,
        key: String,
        fetch: Option<String>,
    },
    /// Fetch the complete item behind a transcript preview (again, after a failure).
    Fetch {
        session: Id,
        token: String,
    },
    /// Show the page of messages before the shown ones.
    Older(Id),
    /// Revalidate a session page or a log capture.
    Refresh(DetailRef),
    /// Acquire or release one watcher of the session's live topic.
    Watch {
        session: Id,
        on: bool,
    },
    /// The composer's text changed.
    Draft {
        session: Id,
        text: String,
    },
    /// Send the text: steering the running turn, or as a follow-up after it.
    Send {
        session: Id,
        text: String,
        policy: SendPolicy,
        attachments: Vec<SendAttachment>,
        /// What a refusal puts back in the draft instead of `text`, when the composer made it.
        restore: Option<String>,
    },
    /// Run the draft `!command` (`!!command`: kept out of the agent's context) as the owner's
    /// shell command on the host; its reply becomes a notice.
    Shell {
        session: Id,
        text: String,
    },
    /// Put the owner's message before (`older`) or after the recalled one in the draft.
    Recall {
        session: Id,
        older: bool,
    },
    /// The `@` file mention popup.
    Mention(super::MentionAction),
    /// The next message goes to this child, or to the session's own agent.
    Receiver {
        session: Id,
        to: Option<String>,
    },
    /// The next child that can take a message, then back to the session's own agent.
    CycleReceiver(Id),
    /// Show or hide every thinking block.
    Thinking(Id),
    /// A run control (the confirm sheet first for one that asks).
    Control {
        session: Id,
        control: Control,
    },
    SelectAgent {
        session: Id,
        child: Option<String>,
    },
    OpenLive(Live),
    /// Open the task's Actions menu.
    TaskMenu(Id),
    Back,
    /// The dock: its cards, the queue, the exit.
    Dock(DockAction),
    Sidebar(SidebarAction),
    Changes(ChangesAction),
    Files(super::FilesAction),
    Board(BoardAction),
    Usage(UsageAction),
    Task(TaskAction),
    Proof(ProofAction),
    /// The Agents tab: its jobs, conversations, confirmations and model choices.
    Fleet(super::FleetAction),
    /// The model and effort pickers.
    Picker(super::PickerAction),
    /// The session-wide catalog rows: host actions, the board, the `/` menu.
    Command(super::CommandAction),
    /// Return the transcript to its newest rows and follow them again.
    JumpEnd(Id),
    /// A first Esc on an idle, empty composer: the next one within a moment opens Rewind.
    ArmRewind(Id),
}

/// Whether a continuation should be fetched: it was never asked for, or its last read failed.
fn wanted(feed: &Feed, token: &str) -> bool {
    feed.items
        .get(token)
        .is_none_or(|fetch| matches!(fetch, transcript::ItemFetch::Failed(_)))
}

/// The page of messages before the shown ones, when the daemon offered one and none is loading.
fn older(feed: &Feed, session: &Id) -> Option<Effect> {
    let target = DetailRef::Session {
        id: session.to_string(),
    };
    let cache = feed.detail(&target)?;
    if cache.loading {
        return None;
    }
    let token = transcript::older_token(cache)?;
    Some(Effect::Detail(DetailGet {
        target,
        etag: None,
        page: Some(token),
    }))
}

/// Revalidate a detail with the etag of the page shown.
fn reread(feed: &Feed, target: DetailRef) -> Effect {
    Effect::Detail(DetailGet {
        etag: feed
            .detail(&target)
            .and_then(|cache| cache.pages.first())
            .map(|page| page.etag.clone()),
        target,
        page: None,
    })
}

/// RFC 3339 of a unix second; none before the clock's first tick.
fn stamp(now: u64) -> Option<String> {
    let second = i64::try_from(now).ok().filter(|second| *second > 0)?;
    jiff::Timestamp::from_second(second)
        .ok()
        .map(|at| at.to_string())
}

impl State {
    /// The UI state of session `id`; an untouched session's is the default.
    pub fn ui(&self, id: &str) -> &Ui {
        self.sessions.get(id).unwrap_or(&EMPTY_UI)
    }

    pub(super) fn ui_mut(&mut self, id: &Id) -> &mut Ui {
        self.sessions.entry(id.clone()).or_default()
    }

    /// Handles a page action. Every engine or host operation goes out as an effect; nothing is
    /// simulated here.
    pub fn act(&mut self, action: Action, feed: &Feed) -> Vec<Effect> {
        match action {
            Action::Tab { session, tab } => {
                self.panel.give_way(tab != Tab::Chat);
                // The explorer is a tree beside its preview: Details would be a third column
                // listing files again. It closes on entry only, so the owner can reopen it.
                if tab == Tab::Files
                    && self.ui(session.as_str()).tab != Tab::Files
                    && self.panel.shown()
                {
                    self.panel.open = Some(false);
                }
                let rows = tabs::rows(feed, session.as_str());
                let ui = self.ui_mut(&session);
                let refresh_files = tab == Tab::Files && ui.tab != tab;
                if tab == Tab::Chat {
                    dock::leave(ui, feed.now);
                }
                if ui.tab == Tab::Chat || tab == Tab::Chat {
                    ui.seen = Some(rows);
                }
                ui.tab = tab;
                ui.sub = None;
                ui.focus = if tab == Tab::Chat {
                    Focus::Composer
                } else {
                    Focus::Main
                };
                if refresh_files {
                    ui.files
                        .targets(&session)
                        .into_iter()
                        .map(|target| reread(feed, target))
                        .collect()
                } else {
                    Vec::new()
                }
            }
            Action::Sub { session, sub } => {
                self.panel.give_way(sub.is_some());
                let ui = self.ui_mut(&session);
                match &sub {
                    Some(SubView::Usage) => ui.tab = Tab::Chat,
                    Some(SubView::Agent(_) | SubView::Job(_)) => ui.tab = Tab::Agents,
                    None => {}
                }
                ui.sub = sub;
                Vec::new()
            }
            Action::Focus { session, focus } => {
                self.ui_mut(&session).focus = focus;
                Vec::new()
            }
            Action::Expand {
                session,
                key,
                fetch,
            } => {
                let ui = self.ui_mut(&session);
                let opening = !ui.expanded.remove(&key);
                if opening {
                    ui.expanded.insert(key);
                }
                match fetch {
                    Some(token) if opening && wanted(feed, &token) => {
                        vec![Effect::SessionItem { session, token }]
                    }
                    _ => Vec::new(),
                }
            }
            Action::Fetch { session, token } => {
                if wanted(feed, &token) {
                    vec![Effect::SessionItem { session, token }]
                } else {
                    Vec::new()
                }
            }
            Action::Older(session) => older(feed, &session).into_iter().collect(),
            Action::Refresh(target) => vec![reread(feed, target)],
            Action::Watch { session, on } => vec![Effect::Watch { session, on }],
            Action::Draft { session, text } => {
                let ui = self.ui_mut(&session);
                if text.is_empty() {
                    ui.recall = None;
                }
                ui.draft = text;
                ui.picker.slash_at = None;
                ui.mention.at = 0;
                Vec::new()
            }
            Action::Shell { session, text } => self.shell(session, &text, feed),
            Action::Recall { session, older } => {
                super::prompt_history::recall(self, &session, older, feed)
            }
            Action::Mention(action) => super::mention::act(self, action, feed),
            Action::Send {
                session,
                text,
                policy,
                attachments,
                restore,
            } => self.send(session, text, policy, attachments, restore, feed),
            Action::Receiver { session, to } => {
                self.receiver(&session, to, feed);
                Vec::new()
            }
            Action::CycleReceiver(session) => {
                let steerable: Vec<String> = feed
                    .live
                    .get(session.as_str())
                    .and_then(|live| live.fleet.as_ref())
                    .map(|fleet| {
                        fleet
                            .children
                            .iter()
                            .filter(|child| child.steerable)
                            .map(|child| child.id.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                let current = self.ui(session.as_str()).to.clone();
                let next = match current {
                    None => steerable.first().cloned(),
                    Some(child) => steerable
                        .iter()
                        .position(|id| *id == child)
                        .and_then(|at| steerable.get(at + 1))
                        .cloned(),
                };
                self.receiver(&session, next, feed);
                Vec::new()
            }
            Action::Thinking(session) => {
                let ui = self.ui_mut(&session);
                ui.hide_thinking = !ui.hide_thinking;
                Vec::new()
            }
            Action::Control { session, control } => {
                if control == Control::Rewinds {
                    self.ui_mut(&session).rewind_armed = None;
                }
                vec![Effect::Control {
                    session,
                    key: self.ids.next_id(),
                    control,
                    confirmed: false,
                }]
            }
            Action::ArmRewind(session) => {
                let ui = self.ui_mut(&session);
                ui.rewind_armed = Some(feed.now);
                ui.notices.push(Hue::Neutral, "Esc again to rewind", feed.now);
                Vec::new()
            }
            Action::SelectAgent { session, child } => {
                self.ui_mut(&session).agent = child;
                Vec::new()
            }
            Action::OpenLive(live) => vec![Effect::OpenLive(live)],
            Action::TaskMenu(task) => vec![Effect::VerbMenu(Record::Task(task))],
            Action::Back => vec![Effect::Back],
            Action::Dock(action) => dock::act(self, action, feed),
            Action::Sidebar(action) => sidebar::act(self, action, feed),
            Action::Changes(action) => changes::act(self, action, feed),
            Action::Files(action) => super::files::act(self, action, feed),
            Action::Board(action) => board::act(self, action, feed),
            Action::Usage(action) => usage::act(self, action, feed),
            Action::Task(action) => task::act(self, action, feed),
            Action::Proof(action) => proof::act(self, action, feed),
            Action::Fleet(action) => super::fleet::act(self, action, feed),
            Action::Picker(action) => super::actions::picker_act(self, action, feed),
            Action::Command(action) => super::actions::act(self, action, feed),
            Action::JumpEnd(session) => {
                let ui = self.ui_mut(&session);
                ui.jump = ui.jump.wrapping_add(1);
                Vec::new()
            }
        }
    }

    fn receiver(&mut self, session: &Id, to: Option<String>, feed: &Feed) {
        let now = feed.now;
        let ui = self.ui_mut(session);
        dock::leave(ui, now);
        if ui.to == to {
            return;
        }
        let words = match &to {
            Some(child) => format!("Messages now go to {child}"),
            None => "Messages go to the session's own agent again".into(),
        };
        ui.to = to;
        ui.notices.push(Hue::Neutral, words, now);
    }

    /// Ask keeps its composer mounted while an End is pending, so the draft stays editable.
    /// A press from a composer still keyed to the old chat must not make a send or clear it.
    pub fn input_block(
        &self,
        session: &Id,
        ask: bool,
        chats: &assistant::State,
        feed: &Feed,
    ) -> Option<&'static str> {
        if ask && *session != self.ask_id() {
            return Some("Ask is switching threads");
        }
        if assistant::ended(feed, session.as_str()) {
            return Some("This thread has ended");
        }
        matches!(
            assistant::end_state(chats, feed, session.as_str()),
            Some(EndState::Pending)
        )
        .then_some("Wait for End thread to finish")
    }

    /// Run the composer or slash menu action, rechecking the live handoff at activation.
    pub fn act_input(
        &mut self,
        action: Action,
        ask: bool,
        chats: &assistant::State,
        feed: &Feed,
    ) -> Vec<Effect> {
        if let Action::Send { session, .. } | Action::Shell { session, .. } = &action
            && self.input_block(session, ask, chats, feed).is_some()
        {
            return Vec::new();
        }
        self.act(action, feed)
    }

    /// A slash selection may clear the draft before running its command; fence the batch.
    pub fn act_slash(
        &mut self,
        session: &Id,
        actions: Vec<Action>,
        ask: bool,
        chats: &assistant::State,
        feed: &Feed,
    ) -> Vec<Effect> {
        if self.input_block(session, ask, chats, feed).is_some() {
            return Vec::new();
        }
        actions
            .into_iter()
            .flat_map(|action| self.act(action, feed))
            .collect()
    }

    /// Sends text under a new command id, which is also its input id; the echo shows it until
    /// the transcript serves a message under that id.
    fn send(
        &mut self,
        session: Id,
        text: String,
        policy: SendPolicy,
        attachments: Vec<SendAttachment>,
        restore: Option<String>,
        feed: &Feed,
    ) -> Vec<Effect> {
        // Served slash commands also dispatch Send without using the composer's button.
        if super::actions::model_change_pending(self.ui(session.as_str()), feed) {
            return Vec::new();
        }
        if let Some(effects) = dock::compose_send(self, &session, &text, feed) {
            return effects;
        }
        let text = text.trim().to_owned();
        if text.is_empty() && attachments.is_empty() {
            return Vec::new();
        }
        // `/rewind [n]` is the host's own command: the sheet opens here instead of a send.
        if let Some(turn) = super::keys::rewind_typed(&text)
            && crate::model::agents::rewind_offered(feed, session.as_str())
        {
            let ui = self.ui_mut(&session);
            ui.draft.clear();
            ui.recall = None;
            return vec![Effect::Control {
                session,
                key: self.ids.next_id(),
                control: turn.map_or(Control::Rewinds, Control::Rewind),
                confirmed: false,
            }];
        }
        let key = self.ids.next_id();
        let typed_at = stamp(feed.now);
        let ui = self.ui_mut(&session);
        let to = ui.to.clone();
        // A child takes a steer only; follow-ups queue for the session's own agent.
        let policy = if to.is_some() {
            SendPolicy::Steer
        } else {
            policy
        };
        ui.echoes
            .retain(|sent| !(sent.told && echo::state(sent, session.as_str(), feed).settled()));
        ui.echoes.push(Sent {
            key: key.clone(),
            text: text.clone(),
            policy,
            to: to.clone(),
            typed_at: typed_at.clone(),
            attachments: attachments.clone(),
            restore: restore.map(|words| words.trim().to_owned()),
            unsent: None,
            told: false,
        });
        ui.draft.clear();
        ui.recall = None;
        vec![Effect::SessionSend {
            key: key.clone(),
            send: SessionSend {
                id: session.to_string(),
                cmd_id: key.to_string(),
                text,
                policy,
                to,
                typed_at,
                attachments,
            },
        }]
    }

    /// Runs the draft's `!command` on the host, as the `/compact` row runs its action, and
    /// clears the draft; an empty command, or a host that does not declare now that it runs
    /// them, leaves it as it is and sends nothing.
    fn shell(&mut self, session: Id, text: &str, feed: &Feed) -> Vec<Effect> {
        let Some(action) = super::composer::shell_action(text) else {
            return Vec::new();
        };
        if !super::composer::shell_runs(feed, session.as_str()) {
            return Vec::new();
        }
        let ui = self.ui_mut(&session);
        ui.draft.clear();
        ui.recall = None;
        self.act(
            Action::Command(super::CommandAction::Host {
                session,
                action,
                what: "Shell command",
            }),
            feed,
        )
    }

    /// The send `key` to `session` never left this device, for this reason.
    pub fn unsent(&mut self, session: &Id, key: &Id, why: String) {
        if let Some(sent) = self
            .ui_mut(session)
            .echoes
            .iter_mut()
            .find(|sent| sent.key == *key)
        {
            sent.unsent = Some(why);
        }
    }

    /// Follows the feed: keys Ask to the chat its sends reached, retires echoes the transcript
    /// now serves or the host handled, and tells each refusal once: its text goes back above the
    /// draft and a notice says why.
    pub fn settle(&mut self, feed: &Feed) {
        self.key_ask(feed);
        for (scope, ui) in &mut self.sessions {
            let page = transcript::page(feed.detail(&DetailRef::Session {
                id: scope.to_string(),
            }));
            ui.echoes
                .retain(|sent| !echo::retired(sent, scope.as_str(), page, feed));
            dock::settle(ui, scope.as_str(), feed);
            super::fleet::settle(ui, feed);
            let mut refused = Vec::new();
            for sent in ui.echoes.iter_mut().filter(|sent| !sent.told) {
                match echo::state(sent, scope.as_str(), feed) {
                    EchoState::Refused(why) => {
                        sent.told = true;
                        let words = sent.restore.clone().unwrap_or_else(|| sent.text.clone());
                        refused.push((words, why));
                    }
                    EchoState::Withdrawn => sent.told = true,
                    _ => {}
                }
            }
            for (text, why) in refused {
                ui.draft = if ui.draft.is_empty() {
                    text
                } else {
                    format!("{text}\n\n{}", ui.draft)
                };
                ui.notices
                    .push(Hue::Fail, format!("Not sent: {why}"), feed.now);
            }
        }
    }

    /// Keys Ask to the chat a send to `quick` was acknowledged under (the first send with no
    /// quick chat creates one), moving what Ask showed there: its draft and its sends, which the
    /// chat's transcript then retires by their input ids. Once that chat has ended Ask is back on
    /// `quick`, so its next send starts a new chat.
    fn key_ask(&mut self, feed: &Feed) {
        let quick = Id::from(assistant::QUICK);
        if let Some(chat) = &self.ask {
            if assistant::ended(feed, chat.as_str()) {
                let draft = self
                    .sessions
                    .get_mut(chat)
                    .map(|ui| std::mem::take(&mut ui.draft));
                if let Some(draft) = draft.filter(|text| !text.is_empty()) {
                    let next = &mut self.ui_mut(&quick).draft;
                    *next = if next.is_empty() {
                        draft
                    } else {
                        format!("{draft}\n\n{next}")
                    };
                }
                self.ask = None;
            }
            return;
        }
        let Some(chat) = self.sessions.get(&quick).and_then(|ui| {
            ui.echoes.iter().find_map(|sent| {
                let session = echo::ack(feed, sent.key.as_str())?.session;
                (!session.is_empty()
                    && session != quick.as_str()
                    && !assistant::ended(feed, &session))
                .then_some(session)
            })
        }) else {
            return;
        };
        let chat = Id::from(chat.as_str());
        if let Some(moved) = self.sessions.remove(&quick) {
            match self.sessions.get_mut(&chat) {
                Some(ui) => {
                    ui.echoes.extend(moved.echoes);
                    if ui.draft.is_empty() {
                        ui.draft = moved.draft;
                    }
                }
                None => {
                    self.sessions.insert(chat.clone(), moved);
                }
            }
        }
        self.ask = Some(chat);
    }
}
