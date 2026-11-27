//! Engine operations from every surface: the one local question a destructive or forced one
//! asks, `op.submit`, the receipt toast, and the refusal sheet whose buttons are the engine's
//! typed next steps. No daemon round trip happens before a submit.

use std::collections::{BTreeMap, BTreeSet};

use corral_protocol::op::OpSubmit;
use nsh_core::prelude::*;

use super::AppState;
use crate::client::requests;
use crate::model::act::{self, Confirming, RefusalSheet};
use crate::model::daemon::Link;
use crate::model::next::Pending;
use crate::model::{Act, ActStatus, Id};

const NO_SOURCE: &str = "Not connected. Choose Preview explicitly to use sample data.";

/// The same operation, forced or not, as one already sent. Operations that add something
/// (a task, a comment, a reply, an artifact, usage) may be meant twice and never count.
fn same(sent: &Act, act: &Act) -> bool {
    use corral_types::Op;
    let adds = matches!(
        act.op,
        Op::New { .. }
            | Op::Comment { .. }
            | Op::Reply { .. }
            | Op::LensDraft { .. }
            | Op::LensRepair { .. }
            | Op::ArtifactAdd { .. }
            | Op::UsageAdd { .. }
    );
    !adds && sent.op == act.op && sent.force == act.force
}

impl AppState {
    /// A second press while corrald has not answered the first would send the operation
    /// twice; the first one's outcome is still to come, so this one is dropped with a toast.
    fn already_sent(&self, act: &Act) -> bool {
        let sent = self
            .watching
            .with_untracked(|watching| watching.values().any(|sent| same(sent, act)))
            || self
                .catching_up
                .with_untracked(|confirmed| confirmed.values().any(|(sent, _)| same(sent, act)));
        if sent {
            self.toast(format!("{} · already sent, waiting for corrald", act.title));
        }
        sent
    }

    /// Submits the operation, after the confirm sheet when it is destructive or forced and the
    /// press that made it was not already its confirmation.
    pub fn dispatch(&self, act: Act) {
        if !untracked(|| self.daemon()) {
            self.toast(NO_SOURCE);
            return;
        }
        if self.already_sent(&act) {
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
        // The confirm sheet may have stood open while the same operation went out elsewhere.
        if self.already_sent(&act) {
            return;
        }
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
                self.settle_pane_cite(true);
            }
            None => {}
        }
    }

    /// Closes the confirm sheet; nothing was sent, and a leave stays with its draft.
    pub fn confirm_no(&self) {
        if let Some(asked) = self.confirm.get_untracked() {
            self.confirm.set(None);
            self.settle_pane_cite(false);
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

    /// Where `op`, sent from this app, stands while a next-step button must not offer it
    /// again: sent and unanswered, or answered (done or unknown) while the board does not show
    /// a walk taken after the answer.
    pub fn op_pending(&self, op: &corral_types::Op) -> Option<Pending> {
        if self
            .watching
            .with(|watching| watching.values().any(|sent| sent.op == *op))
        {
            return Some(Pending::Sending);
        }
        self.catching_up.with(|waiting| {
            waiting
                .values()
                .find(|(sent, _)| sent.op == *op)
                .map(|(_, behind)| {
                    if behind.confirmed {
                        Pending::Confirmed
                    } else {
                        Pending::Unknown
                    }
                })
        })
    }

    /// An unknown outcome waits for a later Tasks subscription, which nothing else may bring:
    /// whenever the link is up and one is held, Tasks is closed and opened again in order, once
    /// per board subscription (a refresh that brings none is not repeated until the board moves
    /// on, as a reconnect does). Only the move is held back; nothing resends it.
    fn refresh_for_unknown(self) {
        let asked = stored(None::<u64>);
        effect(move || {
            let held = self
                .catching_up
                .with(|waiting| waiting.values().any(|(_, behind)| !behind.confirmed));
            let (up, subscription) = self
                .feed
                .with(|feed| (feed.link == Link::Up, feed.board.walk.0));
            if !held || !up || untracked(|| self.fixture()) {
                return;
            }
            if asked.with(|asked| *asked == Some(subscription)) {
                return;
            }
            let _ = asked.replace(Some(subscription));
            _ = requests::send(requests::ClientRequest::RefreshTasks);
        });
    }

    /// A move that may have been applied stays under way until the board shows a walk taken
    /// after its reply (`act::board_caught_up`). A stalled board keeps it waiting; nothing
    /// resends it.
    fn settle_catching_up(self) {
        self.refresh_for_unknown();
        effect(move || {
            let caught: Vec<Id> = self.catching_up.with(|confirmed| {
                self.feed.with(|feed| {
                    confirmed
                        .iter()
                        .filter(|(_, (act, behind))| act::board_caught_up(feed, &act.op, *behind))
                        .map(|(key, _)| key.clone())
                        .collect()
                })
            });
            if !caught.is_empty() {
                untracked(|| {
                    self.catching_up.update(|confirmed| {
                        for key in &caught {
                            confirmed.remove(key);
                        }
                    })
                });
            }
        });
    }

    /// Tracked status of an operation this app submitted.
    pub fn act_status(&self, key: &Id) -> Option<ActStatus> {
        // An operation still followed is pending, whatever its last reply said.
        if self.following.with(|following| following.contains_key(key)) {
            return Some(ActStatus::Pending);
        }
        self.feed.with(|feed| act::status(feed, key)).or_else(|| {
            self.watching
                .with(|watching| watching.contains_key(key))
                .then_some(ActStatus::Pending)
        })
    }

    /// Follows every operation corrald answered `running` with an id to its outcome, under its
    /// own key: one `op.result` at a time, each waiting at corrald, until the outcome replaces
    /// the running reply. Nothing is submitted again.
    fn follow_running(self) {
        // Keys asked for and not yet taken up by the client, so one reply is followed once.
        let sent = stored(BTreeSet::<Id>::new());
        effect(move || {
            let up = self.feed.with(|feed| feed.link == Link::Up);
            let steps: Vec<(Id, act::Follow)> = self.feed.with(|feed| {
                let mut following = untracked(|| self.following.get());
                for (key, op) in &feed.operations {
                    if let Some(id) = act::running_operation(op) {
                        following.insert(key.clone(), id);
                    }
                }
                if untracked(|| self.following.with(|f| *f != following)) {
                    untracked(|| self.following.set(following.clone()));
                }
                following
                    .into_iter()
                    .map(|(key, id)| {
                        let step = act::follow(feed.operations.get(&key), &id);
                        (key, step)
                    })
                    .collect()
            });
            if !up || untracked(|| self.fixture()) {
                sent.with(BTreeSet::clear);
                return;
            }
            for (key, step) in steps {
                match step {
                    act::Follow::Wait => _ = sent.with(|sent| sent.remove(&key)),
                    act::Follow::Finished => {
                        sent.with(|sent| sent.remove(&key));
                        untracked(|| self.following.update(|f| _ = f.remove(&key)));
                    }
                    act::Follow::Ask(id) => {
                        if sent.with(|sent| sent.contains(&key)) {
                            continue;
                        }
                        if requests::op_result(key.to_string(), id) {
                            sent.with(|sent| _ = sent.insert(key));
                        }
                    }
                }
            }
        });
    }

    /// Another daemon store or device identity clears the client's reply table, so an
    /// operation sent before it can never settle here. It stops counting as sent (a step
    /// button would otherwise stay pending for good); its outcome is unknown, not retried.
    fn forget_other_namespace(self) {
        // The namespace each watched operation was first seen under.
        let sent_under = stored(BTreeMap::<Id, (String, String)>::new());
        effect(move || {
            let watched = self.watching.get();
            let orphaned: Vec<Act> = self.feed.with(|feed| {
                let Some(namespace) = feed
                    .store
                    .clone()
                    .zip(feed.session.as_ref().map(|s| s.device.id.clone()))
                else {
                    return Vec::new();
                };
                sent_under.with(|sent| {
                    sent.retain(|key, _| watched.contains_key(key));
                    watched
                        .values()
                        .filter(|act| {
                            let under = sent
                                .entry(act.key.clone())
                                .or_insert_with(|| namespace.clone());
                            *under != namespace && !feed.operations.contains_key(&act.key)
                        })
                        .cloned()
                        .collect()
                })
            });
            if orphaned.is_empty() {
                return;
            }
            untracked(move || {
                self.watching.update(|watching| {
                    for act in &orphaned {
                        watching.remove(&act.key);
                    }
                });
                self.following.update(|following| {
                    for act in &orphaned {
                        following.remove(&act.key);
                    }
                });
                for act in orphaned {
                    self.toast(format!(
                        "{} · outcome unknown: corrald changed before it answered",
                        act.title
                    ));
                }
            });
        });
    }

    /// Each watched operation's outcome, once: a refusal opens its sheet, anything else toasts.
    pub(super) fn watch_receipts(self) {
        self.follow_running();
        self.forget_other_namespace();
        self.settle_catching_up();
        effect(move || {
            let mut watched = self.watching.get();
            // An operation still followed has not settled; its outcome is still to come.
            self.following
                .with(|following| watched.retain(|key, _| !following.contains_key(key)));
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
                let opens = crate::client::walks::opens();
                let behind: Vec<(Act, act::Behind)> = self.feed.with(|feed| {
                    settled
                        .iter()
                        .filter_map(|(act, status)| {
                            act::board_behind(feed, act, status, opens)
                                .map(|from| (act.clone(), from))
                        })
                        .collect()
                });
                batch(|| {
                    self.catching_up.update(|confirmed| {
                        for (act, from) in behind {
                            confirmed.insert(act.key.clone(), (act, from));
                        }
                    });
                    self.watching.update(|watching| {
                        for (act, _) in &settled {
                            watching.remove(&act.key);
                        }
                    });
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
