//! The canonical feed bridge: shared live navigation, details and reducer effects. The
//! selected driver either sends typed requests to corrald or applies an isolated synthetic
//! preview locally; a preview intent never enters the real client's queue. Engine operations
//! go through `ops`.

use corral_protocol::assistant::ContextRef;
use corral_protocol::detail::{DetailGet, DetailRef};

use nsh_core::prelude::*;

use super::AppState;
use crate::client::requests;
use crate::model::agents::Control;
use crate::model::daemon::{ClientError, DetailCache, Feed, Operation};
use crate::model::effect::QueueRequest;
use crate::model::health::{self, Availability};
use crate::model::{Destination, Effect, Env, Id, Live, Overlay, Selection};

const NO_LINK: &str = "Refused: needs a live connection to corrald. Nothing was queued.";

impl AppState {
    /// Tracked: the live record whose page is on top.
    pub fn live_page(&self) -> Option<Live> {
        self.nav.with(|nav| nav.live().cloned())
    }

    /// Tracked: the live record selected in each domain.
    pub fn selection(&self) -> Selection {
        self.nav.with(|nav| nav.selection.clone())
    }

    /// Selects a live record and pushes its page: the session's, or the task's for a task, a
    /// plan and a review candidate. A page shows its record whatever the project scope is.
    pub fn open_live(&self, live: Live) {
        if matches!(live, Live::Session(_)) {
            self.close(Overlay::Ask);
        }
        self.change(|nav| nav.open_live(live));
    }

    /// Selects a live record in place: the row a list page's inspector shows.
    pub fn select(&self, live: Live) {
        self.nav.update(|nav| nav.selection.set(&live));
    }

    /// Opens the one Ask composer with a real context and a draft when given.
    pub fn ask_about(&self, context: Option<ContextRef>, text: Option<String>) {
        self.ask_context.set(context);
        if let Some(text) = text {
            self.set_ask_draft(text);
        }
        self.open_ask();
    }

    /// Tracked: the session Ask reads and sends to (`quick` until its first send is answered).
    pub fn ask_id(&self) -> Id {
        self.session.with(|state| state.ask_id())
    }

    /// Tracked: Ask's draft, which is its session's, the same the session page shows.
    pub fn ask_draft(&self) -> String {
        self.session
            .with(|state| state.ui(state.ask_id().as_str()).draft.clone())
    }

    /// Replaces Ask's draft with `text`.
    pub fn set_ask_draft(&self, text: String) {
        self.reduce(self.session, move |state, feed, _, _| {
            let session = state.ask_id();
            state.act(crate::model::session::Action::Draft { session, text }, feed)
        });
    }

    /// Tracked: why an input or a slash command cannot run during a thread handoff.
    pub fn session_input_block(&self, session: &Id, ask: bool) -> Option<&'static str> {
        self.feed.with(|feed| {
            self.assistant.with(|chats| {
                self.session
                    .with(|state| state.input_block(session, ask, chats, feed))
            })
        })
    }

    /// Run a composer or slash-menu action. The caller may retire staged attachments only
    /// if the reducer actually emitted a send, not if it held the draft during End thread.
    pub fn session_input(&self, action: crate::model::session::Action, ask: bool) -> bool {
        let mut sent = false;
        self.reduce(self.session, |state, feed, _, _| {
            let effects = self
                .assistant
                .with_untracked(|chats| state.act_input(action, ask, chats, feed));
            sent = effects
                .iter()
                .any(|effect| matches!(effect, Effect::SessionSend { .. }));
            effects
        });
        sent
    }

    pub fn session_slash(
        &self,
        session: &Id,
        actions: Vec<crate::model::session::Action>,
        ask: bool,
    ) {
        self.reduce(self.session, |state, feed, _, _| {
            self.assistant
                .with_untracked(|chats| state.act_slash(session, actions, ask, chats, feed))
        });
    }

    /// Tracked: whether corrald serves the destination to this session.
    pub fn available(&self, destination: Destination) -> Availability {
        self.feed
            .with(|feed| health::availability(feed, destination))
    }

    /// Tracked: whether the session negotiated the capability.
    pub fn capable(&self, capability: &str) -> bool {
        self.feed.with(|feed| feed.capable(capability))
    }

    /// Tracked: the authenticated name of this device.
    pub fn device_name(&self) -> Option<String> {
        self.feed
            .with(|feed| health::device_name(feed).map(str::to_owned))
    }

    /// Tracked human attribution for an action receipt, without exposing raw device IDs.
    pub fn via(&self) -> String {
        self.feed.with(|feed| {
            let device = feed
                .session
                .as_ref()
                .map(|session| session.device.id.as_str());
            crate::model::provenance::provenance(feed, "owner", device, "app")
        })
    }

    /// Tracked: today's local date, `YYYY-MM-DD`.
    pub fn today(&self) -> String {
        self.feed.with(|feed| feed.date.clone())
    }

    /// Tracked: today as the shell shows it, "Sat 14 Nov".
    pub fn today_label(&self) -> Option<String> {
        self.feed.with(|feed| health::day_label(&feed.date))
    }

    /// Tracked: corrald's clock when this session opened.
    pub fn server_time(&self) -> Option<String> {
        self.feed.with(|feed| {
            feed.session
                .as_ref()
                .map(|session| session.server_time.clone())
        })
    }

    /// Runs a domain reducer with the live feed and selection, then its effects.
    pub fn reduce<S: Clone + 'static>(
        &self,
        state: impl Into<super::source::StateHandle<S>>,
        reduce: impl FnOnce(&mut S, &Feed, &Selection, Env) -> Vec<Effect>,
    ) {
        let env = untracked(|| self.env());
        let selection = self.nav.with_untracked(|nav| nav.selection.clone());
        let mut effects = Vec::new();
        let state = state.into();
        self.feed.with_untracked(|feed| {
            state.update(|state| effects = reduce(state, feed, &selection, env));
        });
        self.run(effects);
    }

    /// Requests the detail when it is absent, stale or failed; idempotent while it loads.
    pub fn load(&self, target: DetailRef) {
        if self.fixture() {
            if self
                .feed
                .with_untracked(|feed| feed.detail(&target).is_none())
            {
                self.preview_detail(target, None);
            }
            return;
        }
        let wanted = self.feed.with_untracked(|feed| match feed.detail(&target) {
            None => Some(None),
            Some(cache) if cache.loading => None,
            Some(cache) if cache.stale || cache.error.is_some() => Some(etag(cache)),
            Some(_) => None,
        });
        if let Some(etag) = wanted {
            requests::detail(DetailGet {
                target,
                etag,
                page: None,
            });
        }
    }

    /// Revalidates the detail with its etag, unless a request is already loading.
    pub fn refresh(&self, target: DetailRef) {
        if self.fixture() {
            self.preview_detail(target, None);
            return;
        }
        let wanted = self.feed.with_untracked(|feed| match feed.detail(&target) {
            Some(cache) if cache.loading => None,
            cache => Some(cache.and_then(etag)),
        });
        if let Some(etag) = wanted {
            requests::detail(DetailGet {
                target,
                etag,
                page: None,
            });
        }
    }

    /// Pins a mounted/selected detail against aggregate cache eviction.
    pub fn watch_detail(&self, target: DetailRef, open: bool) {
        if self.fixture() {
            return;
        }
        _ = requests::send(requests::ClientRequest::DetailWatch { target, open });
    }

    /// Keeps the mounted reader's current page from rolling past their reading position.
    pub fn transcript_reading(&self, scope: &Id, active: bool) -> bool {
        !self.fixture() && requests::transcript_reading(scope.to_string(), active)
    }

    pub fn transcript_tail(&self, scope: &Id) {
        if !self.fixture() {
            _ = requests::transcript_tail(scope.to_string());
        }
    }

    /// Requests a detail page without discarding its cursor or conditional etag.
    pub fn detail(&self, get: DetailGet) {
        self.run(vec![Effect::Detail(get)]);
    }

    /// Acquires or releases the raw terminal subscription for a mounted session view.
    pub fn terminal(&self, session: String, open: bool) {
        if self.fixture() {
            return;
        }
        _ = requests::terminal(session, open);
    }

    /// Tracked read of a detail's cache.
    pub fn with_detail<R>(
        &self,
        target: &DetailRef,
        read: impl FnOnce(Option<&DetailCache>) -> R,
    ) -> R {
        self.feed.with(|feed| read(feed.detail(target)))
    }

    /// Tracked read of the reply to an assistant session operation, by the caller's key.
    pub fn with_operation<R>(&self, key: &Id, read: impl FnOnce(Option<&Operation>) -> R) -> R {
        let correlation = self.sessions.with(|sessions| sessions.get(key).cloned());
        self.feed.with(|feed| {
            read(
                correlation
                    .as_ref()
                    .and_then(|correlation| feed.operations.get(correlation)),
            )
        })
    }

    /// Applies one live effect.
    pub(super) fn run_live(&self, effect: Effect) {
        if self.fixture()
            && !matches!(
                &effect,
                Effect::OpenLive(_)
                    | Effect::Select(_)
                    | Effect::AskAbout { .. }
                    | Effect::VerbMenu(_)
                    | Effect::Unattach(_)
                    | Effect::EndThread(_)
                    | Effect::Act(_)
                    | Effect::Device(_)
                    | Effect::Control { .. }
                    | Effect::RemoveBuild(_)
                    | Effect::Config { .. }
                    | Effect::Refused(_)
            )
        {
            self.run_preview(effect);
            return;
        }
        match effect {
            Effect::OpenLive(live) => self.open_live(live),
            Effect::Select(live) => self.select(live),
            Effect::AskAbout { context, text } => self.ask_about(context, text),
            Effect::Act(act) => self.dispatch(act),
            Effect::VerbMenu(record) => self.work.update(|state| state.menu = Some(record)),
            Effect::Device(op) => self.device(op),
            Effect::Detail(get) => {
                let loading = get.page.is_none()
                    && self.feed.with_untracked(|feed| {
                        feed.detail(&get.target).is_some_and(|cache| cache.loading)
                    });
                if !loading {
                    requests::detail(get);
                }
            }
            Effect::SessionItem { session, token } => {
                _ = requests::session_item(session.to_string(), token);
            }
            Effect::Watch { session, on } => {
                _ = requests::watch(session.to_string(), on);
            }
            Effect::RetryTopic(topic) => {
                _ = requests::send(requests::ClientRequest::RetryTopic(topic));
            }
            Effect::Changes(query) => _ = requests::changes_page(query),
            Effect::Seen(seen) => _ = requests::seen_advance(seen),
            Effect::Unattach(key) => self.unattach(None, &key),
            Effect::EndThread(chat) => {
                self.refusal.set(None);
                self.confirm
                    .set(Some(crate::model::act::Confirming::end_thread(chat)));
            }
            Effect::Control {
                session,
                key,
                control,
                confirmed,
            } => {
                if control.asks() && !confirmed {
                    let ask = self.feed.with_untracked(|feed| {
                        crate::model::agents::ask(feed, session, key, control)
                    });
                    self.refusal.set(None);
                    self.confirm
                        .set(Some(crate::model::act::Confirming::Control(ask)));
                } else {
                    self.send_control(session, key, control);
                }
            }
            Effect::Refused(sheet) => self.refusal.set(Some(sheet)),
            // The agents' reducer owns the Remove sheet; its dry run goes out as its effect,
            // to the daemon or the sample alike.
            Effect::RemoveBuild(session) => {
                let filter = untracked(|| self.scope_filter());
                self.reduce(self.agents, move |state, feed, _, _| {
                    state.act(crate::model::agents::Action::Remove(session), feed, &filter)
                });
            }
            Effect::Warm(warm) => {
                if let Some(refusal) = self.link_refusal() {
                    self.toast(refusal);
                } else {
                    _ = requests::assistant_warm(warm);
                }
            }
            Effect::Draft { thread, text } => _ = requests::save_draft(thread, text),
            Effect::Session { key, op } => {
                let correlation = requests::new_id();
                if self.link_refusal().is_none()
                    && requests::send(requests::ClientRequest::AssistantSession {
                        request_id: correlation.clone(),
                        request: op,
                    })
                {
                    self.sessions.update(|sessions| {
                        sessions.insert(key, Id::from(correlation));
                    });
                } else {
                    let why = "Not sent: no live connection to corrald.";
                    self.assistant.update(|state| {
                        state.settle(&key, crate::model::assistant::OpOutcome::Failed(why.into()));
                    });
                    self.toast(why);
                }
            }
            // A message waits in this device's outbox while corrald is unreachable and goes on
            // the next connection; only a client that is not running leaves it unsent.
            Effect::SessionSend { key, send } => {
                let session = Id::from(send.id.as_str());
                if !requests::session_send(send) {
                    let why = "the app's connection task is not running".to_owned();
                    self.session
                        .update(|state| state.unsent(&session, &key, why));
                }
            }
            Effect::CodeStart { key, start } => {
                let unsent = self.link_refusal().map(str::to_owned).or_else(|| {
                    (!requests::session_request(
                        key.to_string(),
                        corral_protocol::build::CODE_START,
                        start,
                    ))
                    .then(|| "the app's connection task is not running".to_owned())
                });
                if let Some(why) = unsent {
                    self.agents.update(|state| {
                        state.start.sending = None;
                        state.start.failed = Some(why);
                    });
                }
            }
            Effect::BuildStart { key, start } => {
                let unsent = match self.link_refusal() {
                    Some(refusal) => Some(refusal.to_owned()),
                    None => (!requests::build_start(key.to_string(), start))
                        .then(|| "the app's connection task is not running".to_owned()),
                };
                if let Some(why) = unsent {
                    self.agents.update(|state| {
                        state.start.sending = None;
                        state.start.failed = Some(why);
                    });
                }
            }
            Effect::SessionCardAnswer { key, answer } => {
                self.session_request(
                    key,
                    corral_protocol::session_host::SESSION_CARD_ANSWER,
                    answer,
                );
            }
            Effect::SessionQueue { key, request } => {
                let method = request.method();
                match request {
                    QueueRequest::Withdraw(withdraw) => self.session_request(key, method, withdraw),
                    QueueRequest::Move(step) => self.session_request(key, method, step),
                }
            }
            Effect::SessionAction { key, request } => {
                self.session_request(key, corral_protocol::session_host::SESSION_ACTION, request);
            }
            Effect::SessionModel { key, request } => {
                self.session_request(key, corral_protocol::session_host::SESSION_MODEL, request);
            }
            Effect::SessionChild { key, act } => {
                self.session_request(key, corral_protocol::session_host::SESSION_CHILD, act);
            }
            Effect::SessionJob { key, act } => {
                self.session_request(key, corral_protocol::session_host::SESSION_JOB, act);
            }
            Effect::BuildRemove { key, remove } => {
                self.session_request(key, corral_protocol::build::BUILD_REMOVE, remove);
            }
            Effect::Config { key, put } => self.send_config(key, put),
            _ => {}
        }
    }

    /// Sends a session control under its key and remembers it as the session's latest, so the
    /// session shows how it ended; one that cannot leave says why there instead.
    pub(super) fn send_control(&self, session: Id, key: Id, control: Control) {
        let unsent = if self.fixture() {
            self.feed.update(|feed| {
                crate::model::preview::control(feed, &key, session.as_str(), control)
            });
            None
        } else if let Some(refusal) = self.link_refusal() {
            Some(refusal.to_owned())
        } else {
            let sent = requests::send(requests::ClientRequest::Session {
                request_id: key.to_string(),
                method: control.method(),
                params: control.params(session.as_str()),
            });
            (!sent).then(|| "the app's connection task is not running".to_owned())
        };
        self.agents
            .update(|state| state.control_sent(session, key, control, unsent));
    }

    /// Sends a session or build request under `key`, whose reply the page reads from
    /// `feed.operations`. One that cannot leave this device is written there as failed with why,
    /// so the page says it rather than waiting on it.
    fn session_request(&self, key: Id, method: &'static str, request: impl serde::Serialize) {
        let unsent = match self.link_refusal() {
            Some(refusal) => Some(refusal.to_owned()),
            None => (!requests::session_request(key.to_string(), method, request))
                .then(|| "Not sent: the app's connection task is not running.".to_owned()),
        };
        if let Some(message) = unsent {
            self.feed.update(|feed| {
                feed.operations.insert(
                    key,
                    Operation {
                        method: method.into(),
                        loading: false,
                        value: None,
                        error: Some(ClientError {
                            code: "unsent".into(),
                            message,
                        }),
                        refusal: None,
                    },
                );
            });
        }
    }

    /// Why a call that needs the connection cannot leave now.
    pub(super) fn link_refusal(&self) -> Option<&'static str> {
        self.feed
            .with_untracked(|feed| (!feed.ready()).then_some(NO_LINK))
    }

    /// Follows the feed, through the selected source: retires each session's sends the
    /// transcript now holds and tells each refused one once, tells each session control's
    /// outcome once its reply arrives, and opens a started build's page. A state is written only
    /// when it changed, so the pages reading it redraw only then.
    pub(super) fn follow_sessions(self) {
        effect(move || {
            self.feed.with(|_| ());
            untracked(|| {
                let (sessions, agents, effects) = self.feed.with_untracked(|feed| {
                    let mut sessions = self.session.with_untracked(Clone::clone);
                    sessions.settle(feed);
                    let sessions = self
                        .session
                        .with_untracked(|now| *now != sessions)
                        .then_some(sessions);
                    let mut agents = self.agents.with_untracked(Clone::clone);
                    let effects = agents.settle(feed);
                    let agents = self
                        .agents
                        .with_untracked(|now| *now != agents)
                        .then_some(agents);
                    (sessions, agents, effects)
                });
                if let Some(sessions) = sessions {
                    self.session.update(|state| *state = sessions);
                }
                if let Some(agents) = agents {
                    self.agents.update(|state| *state = agents);
                }
                self.run(effects);
            });
        });
    }
}

fn etag(cache: &DetailCache) -> Option<String> {
    cache.pages.first().map(|page| page.etag.clone())
}
