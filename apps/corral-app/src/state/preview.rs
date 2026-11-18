//! The synthetic feed is a local driver, never a transport fallback. The real connection keeps
//! its own signal, so replies arriving after a source change cannot overwrite sample records.

use nsh_core::prelude::*;

use super::AppState;
use crate::model::{Act, DataState, Effect, preview};

impl AppState {
    pub(super) fn fixture(&self) -> bool {
        self.feed.source() == DataState::Fixture
    }

    pub(super) fn follow_source(self) {
        let clock = computed(move || {
            self.daemon_feed.with(|feed| {
                (
                    feed.now,
                    feed.date.clone(),
                    feed.clock.clone(),
                    feed.time_zone.clone(),
                )
            })
        });
        effect(move || {
            if self.data.get() == DataState::Fixture {
                let (now, date, clock, zone) = clock.get();
                if now != 0 {
                    untracked(|| {
                        self.feed.preview.update(|feed| {
                            feed.now = now;
                            feed.date = date;
                            feed.clock = clock;
                            feed.time_zone = zone;
                        })
                    });
                }
            }
        });
    }

    pub(super) fn preview_detail(&self, target: corral_protocol::detail::DetailRef) {
        self.feed.update(|feed| {
            if let Err(message) = preview::detail(feed, &target) {
                feed.details.insert(
                    crate::model::daemon::detail_key(&target),
                    crate::model::daemon::DetailCache {
                        target,
                        pages: Vec::new(),
                        merged: None,
                        loading: false,
                        stale: false,
                        error: Some(crate::model::daemon::ClientError {
                            code: "preview_missing".into(),
                            message,
                        }),
                    },
                );
            }
        });
    }

    /// The sample engine records the outcome under the operation's key, as the client does.
    pub(super) fn simulate(&self, act: &Act) {
        let mut outcome = Ok(());
        self.feed.update(|feed| outcome = preview::act(feed, act));
        if let Err(why) = outcome {
            self.watching.update(|watching| _ = watching.remove(&act.key));
            self.toast(format!("Preview · {} · {why}", act.title));
        }
    }

    pub(super) fn run_preview(&self, effect: Effect) {
        match effect {
            Effect::Act(act) => self.dispatch(act),
            Effect::Detail(get) => {
                self.preview_detail(get.target);
            }
            Effect::Stop(stop) => {
                let key = crate::model::Id::from(crate::client::requests::new_id());
                self.assistant.update(|state| {
                    state
                        .live
                        .stop_ops
                        .insert(key.clone(), (stop.session.clone(), stop.turn));
                });
                let mut outcome = Ok(());
                self.feed.update(|feed| {
                    outcome = preview::operation(feed, &key, &Effect::Stop(stop));
                });
                if let Err(why) = outcome {
                    self.toast(format!("Preview · {why}"));
                }
            }
            other => {
                if let Effect::Session { key, .. } = &other {
                    self.sessions.update(|sessions| {
                        sessions.insert(key.clone(), key.clone());
                    });
                }
                let mut outcome = Ok(());
                self.feed
                    .update(|feed| outcome = preview::effect(feed, &other));
                if let Effect::Device(op) = &other {
                    self.track_preview_device(op);
                }
                if let Err(why) = outcome {
                    self.toast(format!("Preview · {why}"));
                }
            }
        }
    }

    pub(super) fn preview_send(&self, message: corral_protocol::assistant::AssistantSend) -> bool {
        let mut outcome = Ok(());
        self.feed
            .update(|feed| outcome = preview::effect(feed, &Effect::Send(message)));
        if let Err(why) = outcome {
            self.toast(format!("Preview · {why}"));
            false
        } else {
            true
        }
    }
}
