//! A borrowed view of the selected driver. Switching sources never copies retained transcripts
//! or detail caches, and writes such as retiring send acceptances reach their owning driver.

use nsh_core::prelude::*;

use super::AppState;
use crate::model::{DataState, daemon::Feed};

#[derive(Clone, Copy)]
pub struct FeedSource {
    pub(super) data: Signal<DataState>,
    pub(super) daemon: Signal<Feed>,
    pub(super) preview: Signal<Feed>,
    pinned: Option<DataState>,
}

impl FeedSource {
    pub(super) fn new(
        data: Signal<DataState>,
        daemon: Signal<Feed>,
        preview: Signal<Feed>,
    ) -> Self {
        Self {
            data,
            daemon,
            preview,
            pinned: None,
        }
    }

    pub(super) fn source(&self) -> DataState {
        self.pinned.unwrap_or_else(|| self.data.get_untracked())
    }

    fn selected(&self, source: DataState) -> Signal<Feed> {
        if source == DataState::Fixture {
            self.preview
        } else {
            self.daemon
        }
    }

    pub fn with<R>(&self, read: impl FnOnce(&Feed) -> R) -> R {
        let source = self.pinned.unwrap_or_else(|| self.data.get());
        self.selected(source).with(read)
    }

    pub fn with_untracked<R>(&self, read: impl FnOnce(&Feed) -> R) -> R {
        self.selected(self.source()).with_untracked(read)
    }

    pub fn update(&self, update: impl FnOnce(&mut Feed)) {
        self.selected(self.source()).update(update);
    }
}

/// Interaction state belongs to its source: a preview action form cannot become a live intent.
pub struct StateSource<S: Clone + 'static> {
    data: Signal<DataState>,
    daemon: Signal<S>,
    preview: Signal<S>,
    pinned: Option<DataState>,
}

impl<S: Clone + 'static> Copy for StateSource<S> {}
impl<S: Clone + 'static> Clone for StateSource<S> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<S: Clone + 'static> StateSource<S> {
    pub(super) fn new(data: Signal<DataState>, daemon: S, preview: S) -> Self
    where
        S: PartialEq,
    {
        Self {
            data,
            daemon: signal(daemon),
            preview: signal(preview),
            pinned: None,
        }
    }

    fn selected(&self, source: DataState) -> Signal<S> {
        if source == DataState::Fixture {
            self.preview
        } else {
            self.daemon
        }
    }

    pub fn with<R>(&self, read: impl FnOnce(&S) -> R) -> R {
        self.selected(self.pinned.unwrap_or_else(|| self.data.get()))
            .with(read)
    }

    pub fn with_untracked<R>(&self, read: impl FnOnce(&S) -> R) -> R {
        self.selected(self.pinned.unwrap_or_else(|| self.data.get_untracked()))
            .with_untracked(read)
    }

    pub fn get(&self) -> S {
        self.with(Clone::clone)
    }

    pub fn update(&self, update: impl FnOnce(&mut S)) {
        self.selected(self.pinned.unwrap_or_else(|| self.data.get_untracked()))
            .update(update);
    }
}

/// Reducers accept ordinary signals and source-owned domain state identically.
pub enum StateHandle<S: Clone + 'static> {
    Signal(Signal<S>),
    Source(StateSource<S>),
}

impl<S: Clone + 'static> From<Signal<S>> for StateHandle<S> {
    fn from(signal: Signal<S>) -> Self {
        Self::Signal(signal)
    }
}
impl<S: Clone + 'static> From<StateSource<S>> for StateHandle<S> {
    fn from(source: StateSource<S>) -> Self {
        Self::Source(source)
    }
}
impl<S: Clone + 'static> StateHandle<S> {
    pub(super) fn update(&self, update: impl FnOnce(&mut S)) {
        match self {
            Self::Signal(signal) => signal.update(update),
            Self::Source(source) => source.update(update),
        }
    }
}

impl AppState {
    /// Captures the owning driver for a mounted subscription and its later cleanup. Rendering
    /// continues to read the unpinned app; only that subscription's acquire/release uses this.
    pub fn scope(mut self) -> Self {
        self.feed.pinned = Some(self.feed.source());
        self.assistant.pinned = Some(self.feed.source());
        self.work.pinned = Some(self.feed.source());
        self.plans.pinned = Some(self.feed.source());
        self.agents.pinned = Some(self.feed.source());
        self.session.pinned = Some(self.feed.source());
        self.review.pinned = Some(self.feed.source());
        self.settings.pinned = Some(self.feed.source());
        self
    }
}
