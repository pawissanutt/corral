//! The live bridge: live navigation, capability and authority gates, typed command dispatch
//! with fresh confirmation, detail requests and assistant calls. Every call goes through the
//! shared client; nothing runs locally and nothing falls back to fixture data.

use std::time::Duration;

use corral_protocol::app::{ActPreviewRequest, ActSubmit, ConfirmProof, PREVIEW_TTL_SECS};
use corral_protocol::assistant::{
    AskTarget as WireTarget, AssistantSend, ContextRef, PromptPreview,
};
use corral_protocol::cmd::mode::ModeCommand;
use corral_protocol::cmd::{CmdResultQuery, Command};
use corral_protocol::detail::{DetailGet, DetailRef};

use nsh_core::prelude::*;

use super::AppState;
use crate::client::requests;
use crate::model::act::{self, Confirmable, Confirmation, PreviewState};
use crate::model::daemon::{DetailCache, Feed, Link, Operation};
use crate::model::health::{self, Availability};
use crate::model::{
    Act, ActStatus, AskTarget, Authority, Destination, Effect, Env, Id, Live, Overlay, Selection,
};

/// How long `act.result` may wait for a running command when recovering it.
const RECOVER_WAIT_MS: u32 = 5_000;

const NO_SESSION: &str = "Refused: no app session with corrald yet. Nothing was sent.";
const NO_LINK: &str = "Refused: needs a live connection to corrald. Nothing was queued.";
const PREVIEW_ONLY: &str = "The fixture preview sends corrald nothing.";

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
        if matches!(live, Live::Thread(_)) {
            self.close(Overlay::Ask);
        }
        let form = self.form;
        self.change(|nav| nav.open_live(live, form));
    }

    /// Selects a live record in place.
    pub fn select(&self, live: Live) {
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

    /// Hands a message in the one chat to the client, which keeps it in the device outbox when
    /// there is no session. True only when the client took it: false after a visible refusal
    /// (this device may not message the assistant) or when the client is not running. What it
    /// carries leaves the attachment tray only once the client took it.
    pub fn send_message(&self, message: AssistantSend) -> bool {
        if let Some(refusal) = self.assistant_refusal() {
            self.toast(refusal);
            return false;
        }
        let carried = crate::model::assistant::carried(&message.attachments);
        if requests::assistant_send(message) {
            self.sent_attachments(&carried);
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

    /// Tracked: `via=app:<device>` as corrald attributes this device's actions.
    pub fn via(&self) -> String {
        match self.device_name() {
            Some(name) => format!("via=app:{name}"),
            None => "via=app".into(),
        }
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
        state: Signal<S>,
        reduce: impl FnOnce(&mut S, &Feed, &Selection, Env) -> Vec<Effect>,
    ) {
        let env = untracked(|| self.env());
        let selection = self.nav.with_untracked(|nav| nav.selection.clone());
        let mut effects = Vec::new();
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
            self.toast(PREVIEW_ONLY);
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
            submit(act, None);
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
        self.feed.with(|feed| act::status(feed, key)).or_else(|| {
            self.watching
                .with(|watching| watching.contains_key(key))
                .then_some(ActStatus::Pending)
        })
    }

    /// Asks corrald for the outcome of this exact command, never resubmitting it.
    pub fn recover(&self, key: &Id) {
        requests::act_result(CmdResultQuery {
            cmd_id: key.to_string(),
            wait_ms: RECOVER_WAIT_MS,
            abandon_if_absent: true,
        });
    }

    /// Requests the detail when it is absent, stale or failed; idempotent while it loads.
    pub fn load(&self, target: DetailRef) {
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
        match effect {
            Effect::OpenLive(live) => self.open_live(live),
            Effect::Select(live) => self.select(live),
            Effect::AskAbout {
                context,
                text,
                target,
            } => self.ask_about(context, text, target),
            Effect::Act(act) => self.dispatch(act),
            Effect::Recover(key) => self.recover(&key),
            Effect::Detail(get) => {
                let loading = get.page.is_none()
                    && self.feed.with_untracked(|feed| {
                        feed.detail(&get.target).is_some_and(|cache| cache.loading)
                    });
                if !loading {
                    requests::detail(get);
                }
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
            Effect::Stop(stop) => self.assistant_call(|| _ = requests::assistant_stop(stop)),
            Effect::Warm(warm) => self.assistant_call(|| _ = requests::assistant_warm(warm)),
            Effect::Draft { thread, text } => _ = requests::save_draft(thread, text),
            Effect::Session { key, op } => self.assistant_call(|| {
                let correlation = Id::from(requests::assistant_session(op));
                self.sessions
                    .update(|sessions| _ = sessions.insert(key, correlation));
            }),
            _ => {}
        }
    }

    /// The wire recipient of the composer's current target.
    pub fn ask_wire_target(&self) -> WireTarget {
        match self.ask_target.get_untracked() {
            AskTarget::Assistant => WireTarget::Assistant,
            AskTarget::Pm => WireTarget::Pm,
        }
    }

    /// Confirms the live command on screen with corrald's fresh preview proof.
    pub(super) fn confirm_live(&self) {
        let Some(confirmation) = self.confirmation.get_untracked() else {
            return;
        };
        let state = self.feed.with_untracked(|feed| confirmation.state(feed));
        let PreviewState::Ready(preview) = state else {
            // Nothing to prove yet, or corrald refused: the sheet already says which.
            return;
        };
        self.confirmation.set(None);
        let key = confirmation.subject.key();
        self.watching.update(|watching| {
            if let Some(entry) = watching.get_mut(&key) {
                entry.1 = Watch::Confirmed;
            }
        });
        let token = preview.operation_token.clone();
        let proof = ConfirmProof {
            preview_id: preview.preview_id,
            digest: preview.digest,
        };
        match confirmation.subject {
            Confirmable::Act(mut act) => {
                // An autopilot grant confirms corrald's own operation preview: its token, never
                // one this app makes up.
                if let Command::Mode(ModeCommand::Autopilot { confirm, .. }) = &mut act.command {
                    let Some(token) = token else {
                        self.watching
                            .update(|watching| _ = watching.remove(&act.key));
                        self.toast(format!(
                            "{} · Refused: corrald returned no automation preview",
                            act.title
                        ));
                        return;
                    };
                    *confirm = token;
                }
                submit(&act, Some(proof));
            }
            Confirmable::Prompt { mut answer, .. } => {
                answer.confirm = Some(proof);
                requests::prompt_answer(answer);
            }
        }
    }

    /// Cancels the live confirmation; nothing was sent.
    pub(super) fn cancel_live(&self) {
        if let Some(confirmation) = self.confirmation.get_untracked() {
            self.confirmation.set(None);
            let key = confirmation.subject.key();
            self.watching.update(|watching| _ = watching.remove(&key));
            self.toast(format!(
                "{} · cancelled, nothing was sent",
                confirmation.subject.title()
            ));
        }
    }

    /// Keeps corrald's preview with the open sheet once it arrives, so the client's bounded
    /// operation table dropping the reply cannot turn a ready sheet back into a wait.
    pub(super) fn latch_previews(self) {
        effect(move || {
            let Some(pending) = self.confirmation.get().filter(|c| c.ready.is_none()) else {
                return;
            };
            let PreviewState::Ready(ready) =
                self.feed.with(|feed| act::preview(feed, &pending.preview))
            else {
                return;
            };
            untracked(|| {
                self.confirmation.update(|current| {
                    if let Some(current) = current
                        .as_mut()
                        .filter(|current| current.preview == pending.preview)
                    {
                        current.ready = Some(ready);
                    }
                });
            });
        });
    }

    /// Toasts each watched command's outcome once it settles or becomes unknown, and sends a
    /// command corrald refused for lack of a confirmation through one, once, under the same
    /// identity. A command whose sheet is open is not settled.
    pub(super) fn watch_receipts(self) {
        effect(move || {
            let watched = self.watching.get();
            // Tracked: a sheet closing lets a waiting re-confirmation open.
            let pending = self.confirmation.with(Option::is_some);
            if watched.is_empty() {
                return;
            }
            let mut done = Vec::new();
            let mut reconfirm = None;
            self.feed.with(|feed| {
                for (key, (subject, stage)) in &watched {
                    match stage {
                        Watch::Confirming => continue,
                        Watch::Sent if act::needs_confirmation(feed, key) => {
                            reconfirm.get_or_insert_with(|| subject.clone());
                            continue;
                        }
                        Watch::Sent | Watch::Confirmed => {}
                    }
                    match act::status(feed, key) {
                        Some(status) if status.settled() || status == ActStatus::Unknown => {
                            done.push((key.clone(), subject.title().to_owned(), status));
                        }
                        _ => {}
                    }
                }
            });
            let again = reconfirm.filter(|_| !pending);
            if done.is_empty() && again.is_none() {
                return;
            }
            untracked(move || {
                let via = self.via();
                self.watching.update(|watching| {
                    for (key, _, _) in &done {
                        watching.remove(key);
                    }
                    if let Some(entry) = again.as_ref().and_then(|s| watching.get_mut(&s.key())) {
                        entry.1 = Watch::Confirming;
                    }
                });
                if let Some((_, title, status)) = done.last() {
                    self.toast(format!("{title} · {} · {via}", status.line()));
                }
                if let Some(subject) = again {
                    // Deferred so the preview request leaves after this reaction settles, and
                    // owned by the root: this reaction re-runs on the stage change above, and
                    // its cleanup would cancel a timer it owned.
                    self.root.run(move || {
                        let _ = set_timeout(Duration::ZERO, move || self.preview(subject));
                    });
                }
            });
        });
    }

    /// The refusal a command meets on this device now, if any.
    fn refusal(&self, authority: Authority, needs_link: bool) -> Option<&'static str> {
        self.feed.with_untracked(|feed| {
            if needs_link && feed.link != Link::Up {
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
        match subject {
            subject if confirm => self.preview(subject),
            Confirmable::Prompt { answer, .. } => _ = requests::prompt_answer(answer),
            Confirmable::Act(_) => {}
        }
    }

    /// Asks corrald for the exact preview and shows the confirmation sheet while it arrives.
    fn preview(&self, subject: Confirmable) {
        let correlation = match &subject {
            Confirmable::Act(act) => requests::act_preview(ActPreviewRequest {
                command: act.command.clone(),
                expect: act.expect.clone(),
            }),
            Confirmable::Prompt { answer, .. } => requests::prompt_preview(PromptPreview {
                key: answer.key.clone(),
                rev: answer.rev,
                choice: answer.choice,
            }),
        };
        self.confirmation.set(Some(Confirmation {
            subject,
            preview: Id::from(correlation),
            ready: None,
        }));
    }
}

fn granted(feed: &Feed, authority: Authority) -> bool {
    feed.session.as_ref().is_some_and(|session| {
        let granted = &session.authority;
        match authority {
            Authority::Reply => granted.reply,
            Authority::Act => granted.act,
            Authority::Assistant => granted.assistant,
        }
    })
}

fn etag(cache: &DetailCache) -> Option<String> {
    cache.pages.first().map(|page| page.etag.clone())
}

/// Submits under the command's own identity, issued now and expiring with the preview window.
fn submit(act: &Act, confirm: Option<ConfirmProof>) {
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
