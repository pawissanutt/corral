//! Device-bound previews and command receipts. Authority revisions retire unsent proofs.

use std::time::Duration;

use corral_protocol::app::{ActPreviewRequest, ConfirmProof};
use corral_protocol::assistant::PromptPreview;
use corral_protocol::cmd::Command;
use corral_protocol::cmd::mode::ModeCommand;
use nsh_core::prelude::*;
use std::collections::BTreeMap;

use super::AppState;
use super::live::{Watch, submit};
use crate::client::requests;
use crate::model::act::{self, Confirmable, Confirmation, PreviewState};
use crate::model::{ActStatus, Authority, Id};

impl AppState {
    /// Asks corrald for the exact preview and shows the confirmation sheet while it arrives.
    pub(super) fn preview(&self, subject: Confirmable) {
        // A deferred re-confirmation must not revive a cancelled or invalidated watch.
        if !self.watching.with_untracked(|watching| {
            watching
                .get(&subject.key())
                .is_some_and(|(_, stage)| *stage == Watch::Confirming)
        }) {
            return;
        }
        if let Some(refusal) = self.confirmation_refusal(&subject) {
            self.invalidate_confirmations();
            self.toast(refusal);
            return;
        }
        if self.confirmation.with_untracked(|pending| {
            pending
                .as_ref()
                .is_some_and(|pending| pending.subject.key() != subject.key())
        }) {
            self.cancel_live();
        }
        self.confirmation_revision
            .set(self.feed.with_untracked(|feed| feed.authority_revision));
        if self.fixture() {
            match self
                .feed
                .with_untracked(|feed| crate::model::preview::confirmation(feed, &subject))
            {
                Ok(ready) => self.confirmation.set(Some(Confirmation {
                    preview: Id::from(ready.preview_id.clone()),
                    subject,
                    ready: Some(ready),
                })),
                Err(why) => {
                    self.invalidate_confirmations();
                    self.toast(format!("Preview · {why}"));
                }
            }
            return;
        }
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
            Confirmable::DeviceAuthority { change, .. } => {
                let correlation = requests::new_id();
                if !requests::send(requests::ClientRequest::DeviceAuthorityPreview {
                    request_id: correlation.clone(),
                    request: change.clone(),
                }) {
                    self.invalidate_confirmations();
                    self.toast("Preview not requested: the connection task is not running.");
                    return;
                }
                correlation
            }
        };
        self.confirmation.set(Some(Confirmation {
            subject,
            preview: Id::from(correlation),
            ready: None,
        }));
    }
    /// Confirms the live command on screen with corrald's fresh preview proof.
    pub(super) fn confirm_live(&self) {
        let Some(confirmation) = self.confirmation.get_untracked() else {
            return;
        };
        let revision_changed = self.feed.with_untracked(|feed| {
            feed.authority_revision != self.confirmation_revision.get_untracked()
        });
        if revision_changed {
            self.invalidate_confirmations();
            self.toast("Device authority changed. Review the action again before confirming.");
            return;
        }
        if let Some(refusal) = self.confirmation_refusal(&confirmation.subject) {
            self.invalidate_confirmations();
            self.toast(refusal);
            return;
        }
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
        if self.fixture() {
            match confirmation.subject {
                Confirmable::Act(act) => self.simulate(&act),
                Confirmable::Prompt { answer, title } => {
                    self.run_preview(crate::model::Effect::Prompt {
                        answer,
                        title,
                        confirm: false,
                    });
                }
                Confirmable::DeviceAuthority { key, title, change } => {
                    self.run_preview(crate::model::Effect::Device(
                        crate::model::devices::DeviceOp::SetAuthority { key, title, change },
                    ));
                }
            }
            return;
        }
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
            Confirmable::DeviceAuthority { key, title, change } => {
                self.watching.update(|watching| _ = watching.remove(&key));
                self.submit_authority(
                    key,
                    title,
                    corral_protocol::device::DeviceAuthorityChange {
                        change,
                        confirm: Some(proof),
                    },
                );
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
            let stops = self
                .assistant
                .with(|state| state.live.stop_ops.keys().cloned().collect::<Vec<_>>());
            for key in stops {
                let operation = self
                    .feed
                    .with(|feed| feed.operations.get(&key).filter(|op| !op.loading).cloned());
                if let Some(operation) = operation {
                    untracked(|| {
                        self.act(self.assistant, |state, _| {
                            state.settle_stop(&key, &operation)
                        })
                    });
                }
            }
        });
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
                        set_timeout(Duration::ZERO, move || self.preview(subject));
                    });
                }
            });
        });
    }

    fn confirmation_refusal(&self, subject: &Confirmable) -> Option<&'static str> {
        match subject {
            Confirmable::Act(act) => self.refusal(act.authority(), true),
            Confirmable::Prompt { .. } => self.refusal(Authority::Assistant, true),
            Confirmable::DeviceAuthority { .. } => {
                self.feed.with_untracked(super::devices::admin_refusal)
            }
        }
    }

    /// Notifications invalidate the sheet and every unsent confirmation, never sent commands.
    pub(super) fn invalidate_confirmations(&self) {
        self.confirmation.set(None);
        self.watching.update(retire_confirming);
    }

    pub(super) fn follow_authority(self) {
        let revision = computed(move || self.feed.with(|feed| feed.authority_revision));
        let previous = stored(self.feed.with_untracked(|feed| feed.authority_revision));
        effect(move || {
            let current = revision.get();
            let old = previous.replace(current);
            if old != current {
                untracked(|| {
                    self.confirmation.update(|pending| {
                        self.watching.update(|watching| {
                            retire_authority(old, current, pending, watching);
                        });
                    });
                });
            }
        });
    }
}

type Watches = BTreeMap<Id, (Confirmable, Watch)>;

fn retire_confirming(watching: &mut Watches) {
    watching.retain(|_, (_, stage)| *stage != Watch::Confirming);
}

fn retire_authority(
    old: u64,
    current: u64,
    pending: &mut Option<Confirmation>,
    watching: &mut Watches,
) {
    if old != current {
        *pending = None;
        retire_confirming(watching);
    }
}

#[cfg(test)]
#[path = "confirmation_tests.rs"]
mod tests;
