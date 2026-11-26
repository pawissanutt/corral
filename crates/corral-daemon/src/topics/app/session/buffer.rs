use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use corral_protocol::{
    session_state::SessionState,
    subscription::{SessionFleet, SessionLiveEvent, SessionLiveGap},
};

use super::{
    SESSION_BUFFER_BYTES, SESSION_BUFFER_EVENTS,
    reader::{LivePage, nonempty_fleet},
};
use crate::wake::Waker;

pub(super) type Entry = Arc<Mutex<Buffer>>;

#[derive(Default)]
pub(super) struct Watches {
    pub(super) entries: Mutex<BTreeMap<Arc<str>, Entry>>,
    pub(super) revisions: Arc<AtomicU64>,
}

pub(super) struct Buffer {
    pub(super) transcript: super::transcript_buffer::TranscriptBuffer,
    pub(super) users: usize,
    pub(super) ready: bool,
    pub(super) version: u64,
    revisions: Arc<AtomicU64>,
    pub(super) generation: Option<u32>,
    pub(super) events: VecDeque<(SessionLiveEvent, usize)>,
    pub(super) bytes: usize,
    pub(super) next: u64,
    pub(super) more: bool,
    pub(super) gap: Option<SessionLiveGap>,
    pub(super) fleet: Option<(SessionFleet, usize)>,
    pub(super) state: Option<(SessionState, usize)>,
    pub(super) state_version: u64,
    /// The connections watching this scope: each is woken when there is a page to send,
    /// so none waits out its idle read slice with events buffered.
    pub(super) wakers: Vec<Arc<Waker>>,
}

impl Buffer {
    pub(super) fn new(revisions: Arc<AtomicU64>) -> Self {
        Self {
            transcript: Default::default(),
            users: 0,
            ready: false,
            version: 0,
            revisions,
            generation: None,
            events: VecDeque::new(),
            bytes: 0,
            next: 0,
            more: false,
            gap: None,
            fleet: None,
            state: None,
            state_version: 0,
            wakers: Vec::new(),
        }
    }

    pub(super) fn publish_transcript(
        &mut self,
        ops: Vec<corral_protocol::subscription::TranscriptOp>,
    ) {
        if !ops.is_empty() {
            self.transcript.publish(ops);
            self.wake();
        }
    }

    fn wake(&self) {
        for waker in &self.wakers {
            waker.wake();
        }
    }

    fn clear(&mut self, generation: Option<u32>, gap: Option<SessionLiveGap>) {
        // Tokens cannot repeat when the final watch closes and later recreates a scope.
        self.version = self.revisions.fetch_add(1, Ordering::Relaxed);
        self.generation = generation;
        self.events.clear();
        self.bytes = 0;
        self.next = 0;
        self.more = false;
        self.gap = gap;
        self.ready = true;
    }

    pub(super) fn unavailable(&mut self) {
        self.publish_offline(None);
    }

    fn publish_offline(&mut self, fleet: Option<(SessionFleet, usize)>) {
        if !self.ready || self.generation.is_some() || self.gap.is_none() || self.fleet != fleet {
            self.fleet = fleet;
            self.state = None;
            self.clear(
                None,
                Some(SessionLiveGap {
                    after: self.next,
                    resumed: 0,
                    missing: 0,
                    reconcile: true,
                }),
            );
            self.wake();
        }
    }

    pub(super) fn publish(&mut self, page: LivePage) {
        if page.generation.is_none() && page.gap.is_some() {
            self.publish_offline(page.fleet);
            return;
        }
        let changed = !self.ready
            || self.generation != page.generation
            || (page.generation.is_none() && self.gap.is_some());
        // An idle read publishes nothing a subscriber would send.
        let mut wake = changed || page.gap.is_some() || !page.events.is_empty();
        if changed {
            self.fleet = None;
            self.state = None;
        }
        if changed || page.gap.is_some() {
            let gap = page.gap.or_else(|| {
                (self.ready && page.generation.is_some()).then(|| SessionLiveGap {
                    after: self.next,
                    resumed: page
                        .events
                        .first()
                        .map_or(page.next.saturating_add(1), |(event, _)| event.seq),
                    missing: 0,
                    reconcile: true,
                })
            });
            self.clear(page.generation, gap);
        }
        if let Some(fleet) = page.fleet {
            // A present empty replacement clears the cache; omission leaves it untouched.
            self.fleet = nonempty_fleet(&fleet.0).then_some(fleet);
        }
        if let Some(state) = page.state
            && self.state.as_ref() != Some(&state)
        {
            self.state = Some(state);
            self.state_version = self.state_version.wrapping_add(1);
            wake = true;
        }
        for (event, bytes) in page.events {
            self.bytes += bytes;
            self.events.push_back((event, bytes));
            while self.events.len() > SESSION_BUFFER_EVENTS || self.bytes > SESSION_BUFFER_BYTES {
                if let Some((_, bytes)) = self.events.pop_front() {
                    self.bytes -= bytes;
                }
            }
        }
        self.next = page.next;
        self.more = page.more;
        if wake {
            self.wake();
        }
    }
}
