//! The synthetic feed is a local driver, never a transport fallback. The real connection keeps
//! its own signal, so replies arriving after a source change cannot overwrite sample records.
//! Each tick of the real clock also moves the sample's running pi turns on by one beat.

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
                            preview::tick(feed);
                        })
                    });
                }
            }
        });
    }

    /// Serves `target` from the sample, a walked list up to the page `page` asks for.
    pub(super) fn preview_detail(
        &self,
        target: corral_protocol::detail::DetailRef,
        page: Option<&str>,
    ) {
        self.feed.update(|feed| {
            if let Err(message) = preview::detail(feed, &target, page) {
                feed.details.insert(
                    crate::model::daemon::detail_key(&target),
                    crate::model::daemon::DetailCache {
                        transcript_seams: Default::default(),
                        target,
                        pages: Vec::new(),
                        merged: None,
                        loading: false,
                        stale: false,
                        error: Some(preview::failure(message)),
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
            self.watching
                .update(|watching| _ = watching.remove(&act.key));
            self.toast(format!("Preview · {} · {why}", act.title));
        }
    }

    pub(super) fn run_preview(&self, effect: Effect) {
        match effect {
            Effect::Act(act) => self.dispatch(act),
            Effect::Detail(get) => {
                self.preview_detail(get.target, get.page.as_deref());
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
}
