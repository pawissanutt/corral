use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, ensure};
use corral_protocol::{
    self as wire,
    app::APP_CONTROL_MAX,
    frame::Frame,
    subscription::{BoardCursor, SessionLiveGap, SessionLiveKind, SubAck, SubDelta, SubOpen},
};

use super::{
    SESSION_PAGE_BYTES, SESSION_PAGE_EVENTS, SESSION_SCOPE_BYTES, SESSION_SCOPES,
    SESSION_SUBSCRIPTIONS,
    buffer::{Buffer, Entry, Watches},
    lock,
};
use crate::{state::Shared, topics::notification, wake::Waker};

pub(super) struct Watch {
    service: Arc<Watches>,
    scope: Arc<str>,
    pub(super) entry: Entry,
    waker: Option<Arc<Waker>>,
}

impl Watch {
    /// Publications to this scope wake `waker`'s connection for as long as it watches.
    pub(super) fn wake(&mut self, waker: &Arc<Waker>) {
        lock(&self.entry).wakers.push(Arc::clone(waker));
        self.waker = Some(Arc::clone(waker));
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let mut entries = lock(&self.service.entries);
        let mut buffer = lock(&self.entry);
        if let Some(waker) = &self.waker
            && let Some(at) = buffer
                .wakers
                .iter()
                .position(|held| Arc::ptr_eq(held, waker))
        {
            buffer.wakers.swap_remove(at);
        }
        buffer.users -= 1;
        if buffer.users == 0 {
            entries.remove(&self.scope);
        }
    }
}

#[derive(Default)]
pub(in crate::topics) struct SessionTopics {
    subscribers: BTreeMap<Arc<str>, Subscriber>,
    /// Made with the first subscription: a connection that watches no session waits as before.
    waker: Option<Arc<Waker>>,
}

impl SessionTopics {
    pub fn open(&mut self, shared: &Shared, open: SubOpen) -> Result<()> {
        let scope: Arc<str> = scope(open.scope)?.into();
        ensure!(
            crate::sessions::child_scope(&scope)?.is_none(),
            "child transcripts have no live topic"
        );
        ensure!(
            self.subscribers.contains_key(&scope) || self.subscribers.len() < SESSION_SUBSCRIPTIONS,
            "too many session subscriptions"
        );
        let waker = match &self.waker {
            Some(waker) => Arc::clone(waker),
            None => {
                let waker = Arc::new(Waker::new().context("opening the session wake signal")?);
                self.waker = Some(Arc::clone(&waker));
                waker
            }
        };
        let mut sub = subscribe(
            Arc::clone(&shared.session_live.watches),
            Arc::clone(&scope),
            shared.epoch(),
            open.since,
        )?;
        sub.watch.wake(&waker);
        self.subscribers.insert(scope, sub);
        Ok(())
    }

    pub fn waker(&self) -> Option<Arc<Waker>> {
        self.waker.clone()
    }

    pub fn close(&mut self, scope: Option<String>) {
        if let Some(scope) = scope {
            self.subscribers.remove(scope.as_str());
        }
    }

    pub fn ack(&mut self, ack: SubAck) -> Result<()> {
        let scope = scope(ack.scope)?;
        self.subscribers
            .get_mut(scope.as_str())
            .context("session topic is not open")?
            .ack(ack.cursor)
    }

    pub fn flush(
        &mut self,
        shared: &Shared,
        conn: &mut dyn crate::conn::Conn,
        fleet: bool,
        state: bool,
    ) -> Result<()> {
        for subscriber in self.subscribers.values_mut() {
            if let Some(frame) = subscriber.next(shared.epoch(), fleet, state)? {
                conn.send_frame(&frame)
                    .context("pushing session live page")?;
            }
        }
        Ok(())
    }
}

fn scope(scope: Option<String>) -> Result<String> {
    let scope = scope.context("session scope is required")?;
    ensure!(
        !scope.is_empty()
            && scope.len() <= SESSION_SCOPE_BYTES
            && !scope.chars().any(char::is_control),
        "invalid session scope"
    );
    Ok(scope)
}

pub(super) fn subscribe(
    service: Arc<Watches>,
    scope: Arc<str>,
    epoch: u64,
    since: Option<wire::subscription::SubSince>,
) -> Result<Subscriber> {
    let entry = {
        let mut entries = lock(&service.entries);
        ensure!(
            entries.contains_key(&scope) || entries.len() < SESSION_SCOPES,
            "too many watched sessions"
        );
        let entry = entries
            .entry(Arc::clone(&scope))
            .or_insert_with(|| Arc::new(Mutex::new(Buffer::new(Arc::clone(&service.revisions)))));
        lock(entry).users += 1;
        Arc::clone(entry)
    };
    let version = since
        .as_ref()
        .and_then(|since| since.cursor.after.as_deref())
        .and_then(|after| after.strip_prefix("session:"))
        .and_then(|after| after.split(':').next())
        .and_then(|version| version.parse().ok());
    let force_gap = since
        .as_ref()
        .is_some_and(|since| since.epoch != epoch || version.is_none());
    Ok(Subscriber {
        watch: Watch {
            service,
            scope,
            entry,
            waker: None,
        },
        epoch,
        version,
        seq: since.map_or(0, |since| since.cursor.rev),
        force_gap,
        serial: 0,
        in_flight: None,
        state_version: None,
    })
}

pub(super) struct Subscriber {
    pub(super) watch: Watch,
    epoch: u64,
    version: Option<u64>,
    seq: u64,
    force_gap: bool,
    serial: u64,
    in_flight: Option<BoardCursor>,
    state_version: Option<u64>,
}

impl Subscriber {
    pub(super) fn ack(&mut self, cursor: BoardCursor) -> Result<()> {
        ensure!(
            self.in_flight.as_ref() == Some(&cursor),
            "ack does not match the in-flight cursor"
        );
        self.in_flight = None;
        Ok(())
    }

    pub(super) fn next(
        &mut self,
        epoch: u64,
        include_fleet: bool,
        include_state: bool,
    ) -> Result<Option<Frame>> {
        if !include_state {
            self.state_version = None;
        }
        if self.in_flight.is_some() {
            return Ok(None);
        }
        let buffer = lock(&self.watch.entry);
        if !buffer.ready {
            return Ok(None);
        }
        let changed = self.force_gap
            || self.epoch != epoch
            || self
                .version
                .is_some_and(|version| version != buffer.version);
        let empty_quick = buffer.generation.is_none() && buffer.gap.is_none();
        let send_empty_quick = empty_quick && (changed || self.version.is_none());
        let first = buffer
            .events
            .front()
            .map_or(buffer.next.saturating_add(1), |(event, _)| event.seq);
        let mut start = self.seq;
        let mut gap = None;
        if empty_quick {
            start = 0;
        } else if changed || self.version.is_none() && buffer.gap.is_some() {
            start = first.saturating_sub(1);
            gap = Some(SessionLiveGap {
                after: self.seq,
                resumed: first,
                missing: buffer.gap.as_ref().map_or(0, |gap| gap.missing),
                reconcile: true,
            });
        } else if self.seq < first.saturating_sub(1) || self.seq > buffer.next {
            // A slow consumer resumes from the present, never accumulates a catch-up queue.
            start = buffer.next;
            gap = Some(SessionLiveGap {
                after: self.seq,
                resumed: start.saturating_add(1),
                missing: start.saturating_sub(self.seq),
                reconcile: true,
            });
        }
        let initial_fleet = include_fleet && self.version.is_none() && buffer.fleet.is_some();
        let state = if include_state
            && (gap.is_some() || self.state_version != Some(buffer.state_version))
        {
            buffer.state.as_ref()
        } else {
            None
        };
        if !initial_fleet
            && !send_empty_quick
            && state.is_none()
            && gap.is_none()
            && buffer
                .events
                .back()
                .is_none_or(|(event, _)| event.seq <= start)
        {
            return Ok(None);
        }
        let fleet = include_fleet.then_some(buffer.fleet.as_ref()).flatten();
        let mut used = fleet.map_or(0, |(_, bytes)| *bytes) + state.map_or(0, |(_, bytes)| *bytes);
        let mut seq = start;
        let events: Vec<_> = buffer
            .events
            .iter()
            .filter(|(event, _)| event.seq > start)
            .take(SESSION_PAGE_EVENTS)
            .take_while(|(_, bytes)| {
                used += *bytes + 1;
                used <= SESSION_PAGE_BYTES
            })
            .filter_map(|(event, _)| {
                seq = event.seq;
                let visible = match event.kind {
                    SessionLiveKind::LiveFleet {} => include_fleet,
                    SessionLiveKind::LiveState {}
                    | SessionLiveKind::LivePart { .. }
                    | SessionLiveKind::LiveInput { .. }
                    | SessionLiveKind::LiveEntry {} => include_state,
                    _ => true,
                };
                visible.then(|| event.clone())
            })
            .collect();
        if seq == start && !send_empty_quick && gap.is_none() && !initial_fleet && state.is_none() {
            return Ok(None);
        }
        let version = buffer.version;
        let generation = buffer.generation;
        let next = buffer.next;
        let source_more = buffer.more;
        let fleet = fleet.map(|(fleet, _)| fleet.clone());
        let state_version = buffer
            .state
            .as_ref()
            .filter(|_| include_state)
            .map(|_| buffer.state_version);
        let state = state.map(|(state, _)| Box::new(state.clone()));
        drop(buffer);
        self.serial = self
            .serial
            .checked_add(1)
            .context("session page serial exhausted")?;
        let cursor = BoardCursor {
            rev: seq,
            after: Some(format!("session:{version}:{}", self.serial)),
        };
        let frame = notification(
            wire::SUB_DELTA,
            SubDelta::Session {
                scope: self.watch.scope.to_string(),
                epoch,
                generation,
                events,
                cursor: cursor.clone(),
                more: seq < next || source_more,
                gap,
                transcript: Vec::new(),
                fleet,
                state,
            },
        )?;
        ensure!(
            frame.payload.len() <= APP_CONTROL_MAX,
            "session envelope exceeds CONTROL budget"
        );
        self.epoch = epoch;
        self.version = Some(version);
        self.seq = seq;
        self.force_gap = false;
        self.state_version = state_version;
        self.in_flight = Some(cursor);
        Ok(Some(frame))
    }
}
