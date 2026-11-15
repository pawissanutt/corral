//! A scope owns one bridge cursor; connections own only acknowledged buffer positions.
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use corral_core::Store;
use corral_ops::{Ctx, agent::bridge::LivePage, session_live::LiveReader};
use corral_protocol::{
    self as wire,
    app::APP_CONTROL_MAX,
    board::BoardCursor,
    frame::Frame,
    subscription::{SessionLiveEvent, SessionLiveGap, SubAck, SubDelta, SubOpen},
};

use super::super::notification;
use crate::state::Shared;

pub(crate) const SESSION_POLL: Duration = Duration::from_millis(100);
pub(crate) const SESSION_SCOPES: usize = 128;
pub(crate) const SESSION_SUBSCRIPTIONS: usize = 32;
pub(crate) const SESSION_SCOPE_BYTES: usize = 512;
pub(crate) const SESSION_BUFFER_EVENTS: usize = 512;
pub(crate) const SESSION_BUFFER_BYTES: usize = 512 * 1024;
pub(crate) const SESSION_PAGE_EVENTS: usize = 32;
// A quoted 512-byte scope needs at most 1,024 bytes; this also reserves the RPC
// envelope, scalar fields, cursor and optional gap without serializing twice.
pub(crate) const SESSION_ENVELOPE_BYTES: usize = 4 * 1024;
pub(crate) const SESSION_PAGE_BYTES: usize = APP_CONTROL_MAX - SESSION_ENVELOPE_BYTES;

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|error| error.into_inner())
}

type Entry = Arc<Mutex<Buffer>>;

pub(crate) struct Service {
    entries: Mutex<BTreeMap<String, Entry>>,
    revisions: Arc<AtomicU64>,
    thread: Mutex<Option<JoinHandle<()>>>,
    pub(super) terminal: Arc<super::terminal::Service>,
}

struct Buffer {
    users: usize,
    ready: bool,
    version: u64,
    revisions: Arc<AtomicU64>,
    generation: Option<u32>,
    events: VecDeque<(SessionLiveEvent, usize)>,
    bytes: usize,
    next: u64,
    more: bool,
    gap: Option<SessionLiveGap>,
}

impl Buffer {
    fn new(revisions: Arc<AtomicU64>) -> Self {
        Self {
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

    fn unavailable(&mut self) {
        if !self.ready || self.generation.is_some() {
            self.clear(
                None,
                Some(SessionLiveGap {
                    after: self.next,
                    resumed: 0,
                    missing: 0,
                    reconcile: true,
                }),
            );
        }
    }

    fn publish(&mut self, page: LivePage) -> Result<()> {
        let changed = self.generation != Some(page.generation);
        if changed || page.gap.is_some() {
            let gap = page.gap.clone().or_else(|| {
                self.ready.then(|| SessionLiveGap {
                    after: self.next,
                    resumed: page
                        .events
                        .first()
                        .map_or(page.next.saturating_add(1), |e| e.seq),
                    missing: 0,
                    reconcile: true,
                })
            });
            self.clear(Some(page.generation), gap);
        }
        for event in page.events {
            let bytes = serde_json::to_vec(&event)
                .context("sizing session buffer event")?
                .len();
            ensure!(
                bytes <= corral_ops::session_live::LIVE_EVENT_BYTES,
                "live event too large"
            );
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
        Ok(())
    }
}

struct Source {
    entry: Weak<Mutex<Buffer>>,
    reader: LiveReader,
}

/// The seam keeps tests at the cursor/buffer layer, without processes or sockets.
fn poll(
    service: &Service,
    sources: &mut BTreeMap<String, Source>,
    mut read: impl FnMut(&str, &mut LiveReader) -> Result<LivePage>,
    mut closed: impl FnMut() -> bool,
) {
    let registered = lock(&service.entries);
    sources.retain(|scope, source| {
        registered
            .get(scope)
            .is_some_and(|entry| Weak::ptr_eq(&source.entry, &Arc::downgrade(entry)))
    });
    let entries: Vec<_> = registered
        .iter()
        .map(|(scope, entry)| (scope.clone(), Arc::clone(entry)))
        .collect();
    drop(registered);
    for (scope, entry) in entries {
        if closed() {
            break;
        }
        if lock(&entry).users == 0 {
            continue;
        }
        if !sources.contains_key(&scope) {
            sources.insert(
                scope.clone(),
                Source {
                    entry: Arc::downgrade(&entry),
                    reader: LiveReader::default(),
                },
            );
        }
        let Some(source) = sources.get_mut(&scope) else {
            continue;
        };
        let page = read(&scope, &mut source.reader);
        let mut buffer = lock(&entry);
        if buffer.users == 0 {
            continue;
        }
        if page.and_then(|page| buffer.publish(page)).is_err() {
            buffer.unavailable();
        }
    }
}

pub(crate) fn start(ctx: &Ctx, shared: &Arc<Shared>) -> Result<()> {
    let ctx = Ctx {
        store: Store::open_unobserved(ctx.store.root()).context("opening session live store")?,
        herdr: ctx.herdr.clone(),
        resource_probe: Arc::clone(&ctx.resource_probe),
    };
    let service = Arc::new(Service {
        entries: Mutex::default(),
        revisions: Arc::default(),
        thread: Mutex::default(),
        terminal: Arc::default(),
    });
    service.terminal.start(&ctx, shared)?;
    let worker = Arc::clone(&service);
    let shared_worker = Arc::clone(shared);
    let thread = std::thread::Builder::new()
        .name("corrald-session-live".into())
        .spawn(move || {
            let mut sources = BTreeMap::new();
            while !shared_worker.lock().closed {
                poll(
                    &worker,
                    &mut sources,
                    |scope, reader| reader.read(&ctx, scope),
                    || shared_worker.lock().closed,
                );
                // Sleep after the complete pass: wakeups of unrelated topics cannot exceed 10 Hz.
                std::thread::sleep(SESSION_POLL);
            }
        })
        .context("starting session live worker")?;
    *lock(&service.thread) = Some(thread);
    *lock(&shared.session_live) = Some(service);
    Ok(())
}

pub(crate) fn join(shared: &Shared) {
    let service = lock(&shared.session_live).clone();
    if let Some(service) = service {
        if let Some(thread) = lock(&service.thread).take()
            && thread.join().is_err()
        {
            eprintln!("corrald session live worker failed");
        }
        service.terminal.join();
    }
}

struct Watch {
    service: Arc<Service>,
    scope: String,
    entry: Entry,
}

impl Drop for Watch {
    fn drop(&mut self) {
        let mut entries = lock(&self.service.entries);
        let mut buffer = lock(&self.entry);
        buffer.users -= 1;
        if buffer.users == 0 {
            entries.remove(&self.scope);
        }
    }
}

#[derive(Default)]
pub(super) struct SessionTopics {
    subscribers: BTreeMap<String, Subscriber>,
}

impl SessionTopics {
    pub fn len(&self) -> usize {
        self.subscribers.len()
    }
    pub fn contains(&self, scope: Option<&str>) -> bool {
        scope.is_some_and(|scope| self.subscribers.contains_key(scope))
    }

    pub fn open(&mut self, shared: &Shared, open: SubOpen) -> Result<()> {
        let scope = scope(open.scope)?;
        let service = lock(&shared.session_live)
            .clone()
            .context("session live service unavailable")?;
        let sub = subscribe(service, scope.clone(), shared.epoch(), open.since)?;
        self.subscribers.insert(scope, sub);
        Ok(())
    }

    pub fn close(&mut self, scope: Option<String>) {
        if let Some(scope) = scope {
            self.subscribers.remove(&scope);
        }
    }

    pub fn ack(&mut self, ack: SubAck) -> Result<()> {
        let scope = scope(ack.scope)?;
        self.subscribers
            .get_mut(&scope)
            .context("session topic is not open")?
            .ack(ack.cursor)
    }

    pub fn flush(&mut self, shared: &Shared, conn: &mut dyn crate::conn::Conn) -> Result<()> {
        for subscriber in self.subscribers.values_mut() {
            if let Some(frame) = subscriber.next(shared.epoch())? {
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

fn subscribe(
    service: Arc<Service>,
    scope: String,
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
            .entry(scope.clone())
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
        },
        epoch,
        version,
        seq: since.map_or(0, |since| since.cursor.rev),
        force_gap,
        serial: 0,
        in_flight: None,
    })
}

struct Subscriber {
    watch: Watch,
    epoch: u64,
    version: Option<u64>,
    seq: u64,
    force_gap: bool,
    serial: u64,
    in_flight: Option<BoardCursor>,
}

impl Subscriber {
    fn ack(&mut self, cursor: BoardCursor) -> Result<()> {
        ensure!(
            self.in_flight.as_ref() == Some(&cursor),
            "ack does not match the in-flight cursor"
        );
        self.in_flight = None;
        Ok(())
    }

    fn next(&mut self, epoch: u64) -> Result<Option<Frame>> {
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
        let first = buffer
            .events
            .front()
            .map_or(buffer.next.saturating_add(1), |(event, _)| event.seq);
        let mut start = self.seq;
        let mut gap = None;
        if changed || self.version.is_none() && buffer.gap.is_some() {
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
        let mut used = 0;
        let events: Vec<_> = buffer
            .events
            .iter()
            .filter(|(event, _)| event.seq > start)
            .take(SESSION_PAGE_EVENTS)
            .take_while(|(_, bytes)| {
                used += *bytes + 1;
                used <= SESSION_PAGE_BYTES
            })
            .map(|(event, _)| event.clone())
            .collect();
        if events.is_empty() && gap.is_none() {
            return Ok(None);
        }
        let version = buffer.version;
        let generation = buffer.generation;
        let next = buffer.next;
        let source_more = buffer.more;
        drop(buffer);
        self.serial = self
            .serial
            .checked_add(1)
            .context("session page serial exhausted")?;
        let seq = events.last().map_or(start, |event| event.seq);
        let cursor = BoardCursor {
            rev: seq,
            after: Some(format!("session:{version}:{}", self.serial)),
        };
        let frame = notification(
            wire::SUB_DELTA,
            SubDelta::Session {
                scope: self.watch.scope.clone(),
                epoch,
                generation,
                events,
                cursor: cursor.clone(),
                more: seq < next || source_more,
                gap,
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
        self.in_flight = Some(cursor);
        Ok(Some(frame))
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
