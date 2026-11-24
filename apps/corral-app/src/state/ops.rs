//! Engine operations from every surface: the one local question a destructive or forced one
//! asks, `op.submit`, the receipt toast, and the refusal sheet whose buttons are the engine's
//! typed next steps. No daemon round trip happens before a submit.

use std::collections::BTreeSet;

use corral_protocol::op::OpSubmit;
use nsh_core::prelude::*;

use super::AppState;
use crate::client::requests;
use crate::model::act::{self, Confirming, RefusalSheet};
use crate::model::daemon::Link;
use crate::model::{Act, ActStatus, Id};

const NO_SOURCE: &str = "Not connected. Choose Preview explicitly to use sample data.";

impl AppState {
    /// Submits the operation, after the confirm sheet when it is destructive or forced and the
    /// press that made it was not already its confirmation.
    pub fn dispatch(&self, act: Act) {
        if !untracked(|| self.daemon()) {
            self.toast(NO_SOURCE);
            return;
        }
        if act.asks() && !act.confirmed {
            self.refusal.set(None);
            self.confirm.set(Some(Confirming::Op(act)));
            return;
        }
        self.submit(act);
    }

    fn submit(&self, act: Act) {
        let key = act.key.clone();
        self.watching
            .update(|watching| _ = watching.insert(key.clone(), act.clone()));
        if self.fixture() {
            self.simulate(&act);
            return;
        }
        let sent = requests::submit(
            key.to_string(),
            OpSubmit {
                op: act.op,
                force: act.force,
            },
        );
        if !sent {
            self.watching.update(|watching| _ = watching.remove(&key));
            self.toast(format!(
                "{} · Not sent: the connection task is not running.",
                act.title
            ));
        }
    }

    /// Runs what the confirm sheet shows.
    pub fn confirm_yes(&self) {
        match self.confirm.get_untracked() {
            Some(Confirming::Op(mut act)) => {
                self.confirm.set(None);
                act.confirmed = true;
                self.submit(act);
            }
            Some(Confirming::Control(ask)) => {
                self.confirm.set(None);
                self.send_control(ask.session, ask.key, ask.control);
            }
            Some(Confirming::EndThread { chat, .. }) => {
                self.confirm.set(None);
                self.reduce(self.assistant, move |state, feed, _, _| {
                    state.act_live(crate::model::assistant::Action::ConfirmEnd(chat), feed)
                });
            }
            Some(Confirming::Leave(nav)) => {
                self.confirm.set(None);
                self.act(self.settings, |state, _| state.discard_config());
                self.nav.set(*nav);
            }
            None => {}
        }
    }

    /// Closes the confirm sheet; nothing was sent, and a leave stays with its draft.
    pub fn confirm_no(&self) {
        if let Some(asked) = self.confirm.get_untracked() {
            self.confirm.set(None);
            if asked.sends() {
                self.toast(format!("{} · cancelled, nothing was sent", asked.title()));
            }
        }
    }

    /// Runs next step `index` of the open refusal sheet. A forced step was described as such on
    /// the sheet, so pressing it is its one confirmation.
    pub fn run_next(&self, index: usize) {
        let Some(next) = self
            .refusal
            .with_untracked(|sheet| sheet.as_ref()?.refusal.next.get(index).cloned())
        else {
            return;
        };
        self.refusal.set(None);
        self.reduce(self.work, |state, _, _, _| state.next(&next, next.force));
    }

    pub fn close_refusal(&self) {
        self.refusal.set(None);
    }

    /// Tracked status of an operation this app submitted.
    pub fn act_status(&self, key: &Id) -> Option<ActStatus> {
        self.feed.with(|feed| act::status(feed, key)).or_else(|| {
            self.watching
                .with(|watching| watching.contains_key(key))
                .then_some(ActStatus::Pending)
        })
    }

    /// Each watched operation's outcome, once: a refusal opens its sheet, anything else toasts.
    /// Follows every operation corrald answered `running` with an id to its outcome, under its
    /// own key: one `op.result` at a time, each waiting at corrald, until the outcome replaces
    /// the running reply. Nothing is submitted again.
    fn follow_running(self) {
        // Keys sent and not yet taken up by the client, so one reply is followed once.
        let sent = stored(BTreeSet::<Id>::new());
        effect(move || {
            let (up, running, loading) = self.feed.with(|feed| {
                let running: Vec<(Id, String)> = feed
                    .operations
                    .iter()
                    .filter_map(|(key, op)| Some((key.clone(), act::running_operation(op)?)))
                    .collect();
                let loading: BTreeSet<Id> = feed
                    .operations
                    .iter()
                    .filter(|(_, op)| op.loading)
                    .map(|(key, _)| key.clone())
                    .collect();
                (feed.link == Link::Up, running, loading)
            });
            sent.with(|sent| sent.retain(|key| !loading.contains(key)));
            if !up || untracked(|| self.fixture()) {
                return;
            }
            for (key, operation) in running {
                if sent.with(|sent| sent.contains(&key)) {
                    continue;
                }
                if requests::op_result(key.to_string(), operation) {
                    sent.with(|sent| _ = sent.insert(key));
                }
            }
        });
    }

    pub(super) fn watch_receipts(self) {
        self.follow_running();
        effect(move || {
            let watched = self.watching.get();
            if watched.is_empty() {
                return;
            }
            let settled: Vec<(Act, ActStatus)> = self.feed.with(|feed| {
                watched
                    .values()
                    .filter_map(|act| {
                        act::status(feed, &act.key)
                            .filter(ActStatus::settled)
                            .map(|status| (act.clone(), status))
                    })
                    .collect()
            });
            if settled.is_empty() {
                return;
            }
            untracked(move || {
                self.watching.update(|watching| {
                    for (act, _) in &settled {
                        watching.remove(&act.key);
                    }
                });
                for (act, status) in settled {
                    match status {
                        ActStatus::Refused(refusal) => self.refusal.set(Some(RefusalSheet {
                            title: act.title,
                            refusal,
                        })),
                        status => {
                            let line = self.feed.with(|feed| act::receipt(feed, &act, &status));
                            self.toast(line);
                        }
                    }
                }
            });
        });
    }
}
