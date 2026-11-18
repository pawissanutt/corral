//! The canonical feed bridge: shared live navigation, details and reducer effects. The
//! selected driver either sends typed requests to corrald or applies an isolated synthetic
//! preview locally; a preview intent never enters the real client's queue. Engine operations
//! go through `ops`.

use corral_protocol::assistant::{AssistantSend, ContextRef};
use corral_protocol::detail::{DetailGet, DetailRef};

use nsh_core::prelude::*;

use super::AppState;
use crate::client::requests;
use crate::model::agents::{Control, PaneAsk};
use crate::model::daemon::{DetailCache, Feed, Operation};
use crate::model::effect::PaneRequest;
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

    /// Selects a live record and pushes its page where this form stacks it.
    pub fn open_live(&self, live: Live) {
        self.reveal_live(&live);
        if matches!(live, Live::Thread(_)) {
            self.close(Overlay::Ask);
        }
        let form = self.form;
        self.change(|nav| nav.open_live(live, form));
    }

    /// Selects a live record in place.
    pub fn select(&self, live: Live) {
        self.reveal_live(&live);
        self.nav.update(|nav| nav.selection.set(&live));
    }

    /// Opens the one Ask composer with a real context and a draft when given.
    pub fn ask_about(&self, context: Option<ContextRef>, text: Option<String>) {
        self.ask_context.set(context);
        if let Some(text) = text {
            self.ask_draft.set(text);
        }
        self.open_ask();
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

    /// Enqueues an intent for client admission, not an ownership transfer. The composer
    /// retains its words and attachments until the matching persisted acceptance arrives.
    pub fn send_message(&self, mut message: AssistantSend) -> bool {
        let project = untracked(|| self.composer_project());
        crate::model::assistant::attach_default_project(&mut message, project.as_ref());
        if self.fixture() {
            return self.preview_send(message);
        }
        if requests::assistant_send(message) {
            true
        } else {
            self.toast("Not sent: the app's connection to corrald is not running. Your words and attachments stay here.");
            false
        }
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
                self.preview_detail(target);
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
            self.preview_detail(target);
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
                    | Effect::Act(_)
                    | Effect::Device(_)
                    | Effect::Send(_)
                    | Effect::Pane { .. }
                    | Effect::Control { .. }
                    | Effect::Answer { .. }
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
            Effect::Send(message) => _ = self.send_message(message),
            Effect::Unattach(key) => self.unattach(&key),
            Effect::CancelSend(key) => {
                // The client owns whether it ever left: it drops a never-sent or refused one
                // here and asks corrald for the rest, now or on reconnect.
                if !requests::assistant_cancel(key.to_string()) {
                    self.toast("Not withdrawn: the app's connection to corrald is not running. Nothing changed.");
                }
            }
            Effect::Answer { key, answer } => self.answer(key, answer),
            Effect::Thread { scope, open } => _ = requests::thread(scope, open),
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
            Effect::Pane {
                session,
                key,
                what,
                request,
            } => self.pane(session, key, what, request),
            _ => {}
        }
    }

    /// Sends a pane request under its key and remembers it as the session's latest, so the
    /// session shows how it ended; one that cannot leave says why there instead.
    fn pane(&self, session: Id, key: Id, what: PaneAsk, request: PaneRequest) {
        let unsent = if self.fixture() {
            self.feed
                .update(|feed| crate::model::preview::pane(feed, &key, &request));
            None
        } else if let Some(refusal) = self.link_refusal() {
            Some(refusal.to_owned())
        } else {
            let request_id = key.to_string();
            let sent = requests::send(match request {
                PaneRequest::Focus(request) => requests::ClientRequest::PaneFocus {
                    request_id,
                    request,
                },
                PaneRequest::Keys(request) => requests::ClientRequest::PaneKeys {
                    request_id,
                    request,
                },
            });
            (!sent).then(|| "the app's connection task is not running".to_owned())
        };
        self.agents
            .update(|state| state.pane_sent(session, key, what, unsent));
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

    /// Sends an answer to an agent's question under its key; the card shows how it ended.
    fn answer(&self, key: Id, answer: corral_protocol::session::SessionPromptAnswer) {
        if self.fixture() {
            self.feed
                .update(|feed| crate::model::preview::answer(feed, &key, &answer));
        } else if let Some(refusal) = self.link_refusal() {
            self.toast(refusal);
        } else if !requests::send(requests::ClientRequest::PromptAnswer {
            request_id: key.to_string(),
            request: answer,
        }) {
            self.toast("Not sent: the app's connection task is not running.");
        }
    }

    /// Why a call that needs the connection cannot leave now.
    fn link_refusal(&self) -> Option<&'static str> {
        self.feed
            .with_untracked(|feed| (!feed.ready()).then_some(NO_LINK))
    }

    /// Tells each session control's outcome once its reply arrives.
    pub(super) fn settle_controls(self) {
        effect(move || {
            let waiting = self.agents.with(|state| {
                state
                    .controls
                    .values()
                    .filter(|sent| !sent.told && sent.unsent.is_none())
                    .map(|sent| sent.key.clone())
                    .collect::<Vec<_>>()
            });
            if waiting.is_empty() {
                return;
            }
            let settled = self.feed.with(|feed| {
                waiting.iter().any(|key| {
                    feed.operations
                        .get(key.as_str())
                        .is_some_and(|operation| !operation.loading)
                })
            });
            if settled {
                untracked(|| {
                    self.reduce(self.agents, |state, feed, _, _| state.settle_controls(feed))
                });
            }
        });
    }
}

fn etag(cache: &DetailCache) -> Option<String> {
    cache.pages.first().map(|page| page.etag.clone())
}
