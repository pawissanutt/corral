//! The canonical feed bridge: shared live navigation, authority gates and reducer effects.
//! The selected driver either sends typed requests to corrald or applies an isolated synthetic
//! preview locally; a preview intent never enters the real client's queue.

use corral_protocol::app::{ActSubmit, ConfirmProof, PREVIEW_TTL_SECS};
use corral_protocol::assistant::{AssistantSend, ContextRef};
use corral_protocol::cmd::CmdResultQuery;
use corral_protocol::detail::{DetailGet, DetailRef};

use nsh_core::prelude::*;

use super::AppState;
use crate::client::requests;
use crate::model::act::{self, Confirmable, PreviewState};
use crate::model::daemon::{DetailCache, Feed, Operation};
use crate::model::health::{self, Availability};
use crate::model::{
    Act, ActStatus, AskTarget, Authority, Destination, Effect, Env, Id, Live, Overlay, Selection,
};

/// How long `act.result` may wait for a running command when recovering it.
const RECOVER_WAIT_MS: u32 = 5_000;

const NO_SESSION: &str = "Refused: no app session with corrald yet. Nothing was sent.";
const NO_LINK: &str = "Refused: needs a live connection to corrald. Nothing was queued.";
const NO_SOURCE: &str = "Not connected. Choose Preview explicitly to use sample data.";

/// Where a watched command stands between this device and its receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Watch {
    /// Sent without a proof; corrald may still require a confirmation for it.
    Sent,
    /// This device's confirmation sheet is open for it; nothing settles until it confirms
    /// or cancels.
    Confirming,
    /// Sent with this device's fresh proof; a refusal now is its outcome.
    Confirmed,
}

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

    /// Opens the one Ask composer with a real context, a draft and the recipient when given.
    pub fn ask_about(
        &self,
        context: Option<ContextRef>,
        text: Option<String>,
        target: Option<AskTarget>,
    ) {
        self.ask_context.set(context);
        if let Some(text) = text {
            self.ask_draft.set(text);
        }
        if let Some(target) = target {
            self.ask_target.set(target);
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

    /// Tracked: whether this device holds the authority; false without a session.
    pub fn can(&self, authority: Authority) -> bool {
        self.feed.with(|feed| granted(feed, authority))
    }

    /// Enqueues an intent for client admission, not an ownership transfer. The composer
    /// retains its words and attachments until the matching persisted acceptance arrives.
    pub fn send_message(&self, mut message: AssistantSend) -> bool {
        let project = untracked(|| self.composer_project());
        crate::model::assistant::attach_default_project(&mut message, project.as_ref());
        if self.fixture() {
            return self.preview_send(message);
        }
        if let Some(refusal) = self.assistant_refusal() {
            self.toast(refusal);
            return false;
        }
        if requests::assistant_send(message) {
            true
        } else {
            self.toast("Not sent: the app's connection to corrald is not running. Your words and attachments stay here.");
            false
        }
    }

    /// Tracked: this device may message the assistant: granted, or no session right now to say
    /// otherwise (offline, a message queues in the device outbox under its own identity).
    pub fn may_message(&self) -> bool {
        self.feed.with(|feed| {
            feed.session
                .as_ref()
                .is_none_or(|_| granted(feed, Authority::Assistant))
        })
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
            crate::model::inbox::provenance(feed, "owner", device, "app")
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

    /// Runs a typed canonical command: refused visibly without a session, the authority or a
    /// live link; previewed and confirmed on this device when Confirm-class; never resent
    /// under an identity corrald already knows.
    pub fn dispatch(&self, act: Act) {
        if !untracked(|| self.daemon()) {
            self.toast(NO_SOURCE);
            return;
        }
        if let Some(refusal) = self.refusal(act.authority(), true) {
            self.toast(refusal);
            return;
        }
        if self
            .watching
            .with_untracked(|watching| watching.contains_key(&act.key))
        {
            // Already being confirmed or awaiting its receipt: one identity, one flight.
            let status = untracked(|| self.act_status(&act.key)).unwrap_or(ActStatus::Pending);
            self.toast(format!("{} · {}", act.title, status.line()));
            return;
        }
        let known = self.feed.with_untracked(|feed| {
            act::status(feed, &act.key).filter(|_| !act::needs_confirmation(feed, &act.key))
        });
        if let Some(status) = known {
            self.toast(format!("{} · {}", act.title, status.line()));
            return;
        }
        let confirm = act.confirm;
        let subject = Confirmable::Act(act);
        let stage = if confirm {
            Watch::Confirming
        } else {
            Watch::Sent
        };
        self.watching
            .update(|watching| _ = watching.insert(subject.key(), (subject.clone(), stage)));
        if confirm {
            self.preview(subject);
        } else if let Confirmable::Act(act) = &subject {
            if self.fixture() {
                self.simulate(act);
            } else {
                submit(act, None);
            }
        }
    }

    /// Tracked status of a command this app sent or is confirming.
    pub fn act_status(&self, key: &Id) -> Option<ActStatus> {
        if let Some(confirmation) = self
            .confirmation
            .with(|pending| pending.clone().filter(|c| c.subject.key() == *key))
        {
            return Some(self.feed.with(|feed| match confirmation.state(feed) {
                PreviewState::Loading => ActStatus::Previewing,
                PreviewState::Ready(_) => ActStatus::Confirming,
                PreviewState::Failed(why) => ActStatus::Refused(why),
            }));
        }
        self.device_status(key).or_else(|| {
            self.feed.with(|feed| act::status(feed, key)).or_else(|| {
                self.watching
                    .with(|watching| watching.contains_key(key))
                    .then_some(ActStatus::Pending)
            })
        })
    }

    /// Asks corrald for the outcome of this exact command, never resubmitting it.
    pub fn recover(&self, key: &Id) {
        if self.fixture() {
            self.run_preview(Effect::Recover(key.clone()));
            return;
        }
        requests::act_result(CmdResultQuery {
            cmd_id: key.to_string(),
            wait_ms: RECOVER_WAIT_MS,
            abandon_if_absent: true,
        });
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
                    | Effect::Prompt { .. }
                    | Effect::Send(_)
            )
        {
            self.run_preview(effect);
            return;
        }
        match effect {
            Effect::OpenLive(live) => self.open_live(live),
            Effect::Select(live) => self.select(live),
            Effect::AskAbout {
                context,
                text,
                target,
            } => self.ask_about(context, text, target),
            Effect::Act(act) => self.dispatch(act),
            Effect::VerbMenu(record) => self.work.update(|state| state.menu = Some(record)),
            Effect::Recover(key) => self.recover(&key),
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
                if let Some(refusal) = self.assistant_refusal() {
                    self.toast(refusal);
                } else if !requests::assistant_cancel(key.to_string()) {
                    self.toast("Not withdrawn: the app's connection to corrald is not running. Nothing changed.");
                }
            }
            Effect::Prompt {
                answer,
                title,
                confirm,
            } => self.prompt(answer, title, confirm),
            Effect::Thread { scope, open } => _ = requests::thread(scope, open),
            Effect::Stop(stop) => {
                if let Some(refusal) = self.refusal(Authority::Assistant, true) {
                    self.assistant
                        .update(|state| state.stop_failed(&stop.session, stop.turn));
                    self.toast(refusal);
                } else {
                    let session = stop.session.clone();
                    let turn = stop.turn;
                    let correlation = requests::new_id();
                    if requests::send(requests::ClientRequest::AssistantStop {
                        request_id: correlation.clone(),
                        request: stop,
                    }) {
                        self.assistant.update(|state| {
                            state
                                .live
                                .stop_ops
                                .insert(Id::from(correlation), (session, turn));
                        });
                    } else {
                        self.assistant
                            .update(|state| state.stop_failed(&session, turn));
                        self.toast("Stop was not sent: the connection task is not running.");
                    }
                }
            }
            Effect::Warm(warm) => self.assistant_call(|| _ = requests::assistant_warm(warm)),
            Effect::Draft { thread, text } => _ = requests::save_draft(thread, text),
            Effect::Session { key, op } => {
                if let Some(refusal) = self.refusal(Authority::Assistant, true) {
                    self.assistant.update(|state| {
                        state.settle(
                            &key,
                            crate::model::assistant::OpOutcome::Failed(refusal.into()),
                        );
                    });
                    self.toast(refusal);
                } else {
                    let correlation = requests::new_id();
                    if requests::send(requests::ClientRequest::AssistantSession {
                        request_id: correlation.clone(),
                        request: op,
                    }) {
                        self.sessions.update(|sessions| {
                            sessions.insert(key, Id::from(correlation));
                        });
                    } else {
                        let why = "Not sent: the connection task is not running.";
                        self.assistant.update(|state| {
                            state.settle(
                                &key,
                                crate::model::assistant::OpOutcome::Failed(why.into()),
                            );
                        });
                        self.toast(why);
                    }
                }
            }
            _ => {}
        }
    }

    /// The refusal a command meets on this device now, if any.
    pub(super) fn refusal(&self, authority: Authority, needs_link: bool) -> Option<&'static str> {
        self.feed.with_untracked(|feed| {
            if needs_link && !feed.ready() {
                Some(NO_LINK)
            } else if feed.session.is_none() {
                Some(NO_SESSION)
            } else if !granted(feed, authority) {
                Some(authority.refusal())
            } else {
                None
            }
        })
    }

    /// Assistant calls are refused only when a session says this device may not make them.
    fn assistant_refusal(&self) -> Option<&'static str> {
        self.feed.with_untracked(|feed| {
            feed.session
                .as_ref()
                .filter(|_| !granted(feed, Authority::Assistant))
                .map(|_| Authority::Assistant.refusal())
        })
    }

    fn assistant_call(&self, call: impl FnOnce()) {
        match self.refusal(Authority::Assistant, true) {
            Some(refusal) => self.toast(refusal),
            None => call(),
        }
    }

    fn prompt(
        &self,
        answer: corral_protocol::assistant::PromptAnswer,
        title: String,
        confirm: bool,
    ) {
        if let Some(refusal) = self.refusal(Authority::Assistant, true) {
            self.toast(refusal);
            return;
        }
        let subject = Confirmable::Prompt { answer, title };
        let stage = if confirm {
            Watch::Confirming
        } else {
            Watch::Sent
        };
        self.watching
            .update(|watching| _ = watching.insert(subject.key(), (subject.clone(), stage)));
        if self.fixture() && !confirm {
            if let Confirmable::Prompt { answer, title } = subject {
                self.run_preview(Effect::Prompt {
                    answer,
                    title,
                    confirm: false,
                });
            }
            return;
        }
        match subject {
            subject if confirm => self.preview(subject),
            Confirmable::Prompt { answer, .. } => _ = requests::prompt_answer(answer),
            Confirmable::Act(_) | Confirmable::DeviceAuthority { .. } => {}
        }
    }
}

pub(super) fn granted(feed: &Feed, authority: Authority) -> bool {
    feed.session.as_ref().is_some_and(|session| {
        let granted = &session.authority;
        match authority {
            Authority::Reply => granted.reply,
            Authority::Act => granted.act,
            Authority::Assistant => granted.assistant,
            Authority::Admin => granted.admin,
        }
    })
}

fn etag(cache: &DetailCache) -> Option<String> {
    cache.pages.first().map(|page| page.etag.clone())
}

/// Submits under the command's own identity, issued now and expiring with the preview window.
pub(super) fn submit(act: &Act, confirm: Option<ConfirmProof>) {
    let now = jiff::Timestamp::now();
    let expires = now
        .checked_add(jiff::SignedDuration::from_secs(PREVIEW_TTL_SECS as i64))
        .unwrap_or(now);
    requests::act_submit(ActSubmit {
        cmd_id: act.key.to_string(),
        expect: act.expect.clone(),
        issued_at: now.to_string(),
        expires_at: expires.to_string(),
        command: act.command.clone(),
        confirm,
    });
}
