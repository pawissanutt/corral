//! A scope owns one bridge cursor; connections own only acknowledged buffer positions.
use std::{
    collections::{BTreeMap, VecDeque},
    io::{self, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, Weak,
        atomic::{AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use corral_engine::runner::pi::Client;
use corral_protocol::{
    self as wire,
    app::APP_CONTROL_MAX,
    frame::Frame,
    subscription::{
        BoardCursor, SessionFleet, SessionLiveEvent, SessionLiveGap, SessionLiveKind,
        SessionLiveToolState, SessionLiveTurnState, SubAck, SubDelta, SubOpen,
    },
};
use corral_types::SessionRef;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

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

const LIVE_IO_TIMEOUT: Duration = Duration::from_millis(250);
const LIVE_HOST_EVENTS: usize = 512;
const LIVE_EVENT_BYTES: usize = 8 * 1024;
const LIVE_PAYLOAD_BYTES: usize = 1024;
const LIVE_ERROR_BYTES: usize = 512;
const LIVE_FLEET_BYTES: usize = 44 * 1024;

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|error| error.into_inner())
}

type Entry = Arc<Mutex<Buffer>>;

#[derive(Default)]
pub(crate) struct SessionLive {
    watches: Arc<Watches>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Default)]
struct Watches {
    entries: Mutex<BTreeMap<Arc<str>, Entry>>,
    revisions: Arc<AtomicU64>,
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
    fleet: Option<(SessionFleet, usize)>,
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
            fleet: None,
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
            self.fleet = None;
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

    fn publish(&mut self, page: LivePage) {
        let changed = self.generation != Some(page.generation);
        if changed {
            self.fleet = None;
        }
        if changed || page.gap.is_some() {
            let gap = page.gap.or_else(|| {
                self.ready.then(|| SessionLiveGap {
                    after: self.next,
                    resumed: page
                        .events
                        .first()
                        .map_or(page.next.saturating_add(1), |(event, _)| event.seq),
                    missing: 0,
                    reconcile: true,
                })
            });
            self.clear(Some(page.generation), gap);
        }
        if let Some(fleet) = page.fleet {
            // A present empty replacement clears the cache; omission leaves it untouched.
            self.fleet = nonempty_fleet(&fleet.0).then_some(fleet);
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
    }
}

/// The socket rings are independent; only the live ring contributes events to app pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Cursors {
    lifecycle: u64,
    live: u64,
}

#[derive(Deserialize)]
struct RingPage {
    last: u64,
    next: u64,
    gap: Option<SessionLiveGap>,
}

impl RingPage {
    fn validate(&self, after: u64) -> Result<()> {
        ensure!(
            (after..=self.last).contains(&self.next),
            "invalid Pi ring cursor"
        );
        ensure!(
            self.gap.as_ref().is_none_or(|gap| {
                gap.reconcile
                    && gap.after == after
                    && gap.resumed > after
                    && gap.resumed <= self.next
                    && gap.missing == gap.resumed - after - 1
            }),
            "invalid Pi ring gap"
        );
        Ok(())
    }
}

#[derive(Deserialize)]
struct HostPage {
    #[serde(flatten)]
    ring: RingPage,
    events: Vec<HostEvent>,
    more: bool,
    fleet: Option<SessionFleet>,
}

#[derive(Deserialize)]
struct HostEvent {
    seq: u64,
    at: String,
    #[serde(flatten)]
    kind: SessionLiveKind,
}

struct LivePage {
    generation: u32,
    events: Vec<(SessionLiveEvent, usize)>,
    next: u64,
    more: bool,
    gap: Option<SessionLiveGap>,
    fleet: Option<(SessionFleet, usize)>,
}

struct LiveReader {
    socket: Option<PathBuf>,
    cursors: Cursors,
    // On loss, keep the observed cursors for restart detection, but ask for
    // retained events and the current fleet when the host comes back.
    replay: bool,
    generation: u32,
}

impl Default for LiveReader {
    fn default() -> Self {
        Self {
            socket: None,
            cursors: Cursors::default(),
            replay: false,
            generation: 1,
        }
    }
}

impl LiveReader {
    fn read(&mut self, shared: &Shared, scope: &str) -> Result<LivePage> {
        let resolved = crate::sessions::resolve(shared, scope)?;
        ensure!(!resolved.ended, "session ended");
        let Some(SessionRef::Pi { socket, .. }) = resolved.session else {
            anyhow::bail!("session has no Pi socket");
        };
        let rebound = self.socket.as_ref().is_some_and(|prev| *prev != socket);
        let requested = if rebound || self.replay {
            Cursors::default()
        } else {
            self.cursors
        };
        let lifecycle = request(&socket, "events_since", requested.lifecycle)?;
        let live = request(&socket, "live_since", requested.live)?;
        let page = self.advance(requested, lifecycle, live, rebound)?;
        self.socket = Some(socket);
        Ok(page)
    }

    fn advance(
        &mut self,
        requested: Cursors,
        lifecycle: RingPage,
        live: HostPage,
        rebound: bool,
    ) -> Result<LivePage> {
        let restarted = rebound
            || lifecycle.last < self.cursors.lifecycle
            || live.ring.last < self.cursors.live;
        let generation = if restarted {
            self.generation
                .checked_add(1)
                .context("live generation exhausted")?
        } else {
            self.generation
        };
        if restarted && requested != Cursors::default() {
            // A reply to an old cursor cannot contain the new ring's beginning.
            // Publish the discontinuity now and replay both rings on the next tick.
            self.cursors = Cursors::default();
            self.generation = generation;
            return Ok(LivePage {
                generation,
                events: Vec::new(),
                next: 0,
                more: live.ring.last != 0,
                gap: None,
                fleet: None,
            });
        }
        lifecycle.validate(requested.lifecycle)?;
        live.ring.validate(requested.live)?;
        ensure!(
            live.events.len() <= LIVE_HOST_EVENTS,
            "too many Pi live events"
        );
        let mut prior = live
            .ring
            .gap
            .as_ref()
            .map_or(requested.live, |gap| gap.resumed - 1);
        let mut events = Vec::with_capacity(live.events.len());
        for event in live.events {
            ensure!(
                prior.checked_add(1) == Some(event.seq),
                "noncontiguous Pi live events"
            );
            prior = event.seq;
            let event = SessionLiveEvent {
                seq: event.seq,
                generation,
                at: event.at,
                kind: event.kind,
            };
            let bytes = event_bytes(&event)?;
            events.push((event, bytes));
        }
        ensure!(
            prior == live.ring.next && live.more == (prior < live.ring.last),
            "invalid Pi live page end"
        );
        let fleet = live.fleet.map(bound_fleet).transpose()?;
        let gap = live.ring.gap.or_else(|| {
            // A lifecycle-ring gap invalidates the overlay too, but its missing
            // count and resume point have no meaning in the live sequence space.
            lifecycle.gap.map(|_| SessionLiveGap {
                after: requested.live,
                resumed: requested.live.saturating_add(1),
                missing: 0,
                reconcile: true,
            })
        });
        self.cursors = Cursors {
            lifecycle: lifecycle.next,
            live: live.ring.next,
        };
        self.replay = false;
        self.generation = generation;
        Ok(LivePage {
            generation,
            events,
            next: live.ring.next,
            more: live.more,
            gap,
            fleet,
        })
    }
}

#[derive(Serialize)]
struct Since {
    seq: u64,
}

fn request<T: DeserializeOwned>(socket: &Path, command: &str, seq: u64) -> Result<T> {
    let deadline = Instant::now() + LIVE_IO_TIMEOUT;
    let stream = connect(socket, deadline)
        .with_context(|| format!("connecting to Pi live socket {}", socket.display()))?;
    Client::request_on(stream, command, Since { seq }, deadline)
}

fn connect(socket: &Path, deadline: Instant) -> Result<UnixStream> {
    use rustix::io::Errno;
    use rustix::net::{self, AddressFamily, SocketAddrUnix, SocketType};

    // A blocking connect can wait indefinitely when the listen queue is full.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let stream = net::socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        net::SocketFlags::CLOEXEC | net::SocketFlags::NONBLOCK,
        None,
    )?;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let stream = {
        let fd = net::socket(AddressFamily::UNIX, SocketType::STREAM, None)?;
        rustix::io::fcntl_setfd(&fd, rustix::io::FdFlags::CLOEXEC)?;
        fd
    };
    let stream = UnixStream::from(stream);
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    stream.set_nonblocking(true)?;
    let address = SocketAddrUnix::new(socket)?;
    match net::connect(&stream, &address) {
        Ok(()) => {}
        Err(Errno::INPROGRESS) => {
            use rustix::event::{PollFd, PollFlags, Timespec, poll};
            let remaining = deadline.saturating_duration_since(Instant::now());
            let mut fds = [PollFd::new(&stream, PollFlags::OUT)];
            let timeout = Timespec {
                tv_sec: 0,
                tv_nsec: remaining.subsec_nanos().into(),
            };
            ensure!(
                poll(&mut fds, Some(&timeout))? != 0,
                "Pi connect deadline elapsed"
            );
            if let Some(error) = stream.take_error()? {
                return Err(error.into());
            }
        }
        Err(error) => return Err(error.into()),
    }
    stream.set_nonblocking(false)?;
    Ok(stream)
}

#[derive(Default)]
struct CountBytes(usize);

impl Write for CountBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn json_bytes(value: &impl Serialize) -> Result<usize> {
    let mut bytes = CountBytes::default();
    serde_json::to_writer(&mut bytes, value).context("sizing live session data")?;
    Ok(bytes.0)
}

fn event_bytes(event: &SessionLiveEvent) -> Result<usize> {
    let payload = match &event.kind {
        SessionLiveKind::LiveText { text, .. } | SessionLiveKind::LiveThinking { text, .. } => {
            Some(text)
        }
        SessionLiveKind::LiveTool {
            state: SessionLiveToolState::Started { args, .. },
            ..
        } => args.as_ref(),
        SessionLiveKind::LiveTool {
            state: SessionLiveToolState::Updated { partial },
            ..
        } => partial.as_ref(),
        SessionLiveKind::LiveTurn {
            state: SessionLiveTurnState::Ended { error, .. },
            ..
        } => {
            ensure!(
                error
                    .as_ref()
                    .is_none_or(|text| text.len() <= LIVE_ERROR_BYTES),
                "live error exceeds byte bound"
            );
            None
        }
        _ => None,
    };
    ensure!(
        payload.is_none_or(|text| text.len() <= LIVE_PAYLOAD_BYTES),
        "live payload exceeds byte bound"
    );
    let bytes = json_bytes(event)?;
    ensure!(bytes <= LIVE_EVENT_BYTES, "live event exceeds byte bound");
    Ok(bytes)
}

fn nonempty_fleet(fleet: &SessionFleet) -> bool {
    !fleet.children.is_empty() || !fleet.messages.is_empty() || fleet.messages_total != 0
}

fn bound_fleet(mut fleet: SessionFleet) -> Result<(SessionFleet, usize)> {
    fleet.messages_total = fleet
        .messages_total
        .max(fleet.messages.last().map_or(0, |message| message.seq));
    fleet
        .messages
        .drain(..fleet.messages.len().saturating_sub(100));
    for message in &mut fleet.messages {
        if let Some((end, _)) = message.text.char_indices().nth(2048) {
            message.text.truncate(end);
        }
    }
    let mut bytes = json_bytes(&fleet)?;
    trim_fleet(&mut fleet.messages, &mut bytes)?;
    trim_fleet(&mut fleet.children, &mut bytes)?;
    Ok((fleet, bytes + 16))
}

fn trim_fleet<T: Serialize>(items: &mut Vec<T>, bytes: &mut usize) -> Result<()> {
    let mut remove = 0;
    for item in items.iter() {
        if *bytes <= LIVE_FLEET_BYTES {
            break;
        }
        // The final item has no preceding comma.
        remove += 1;
        *bytes -= json_bytes(item)? + usize::from(remove != items.len());
    }
    items.drain(..remove);
    Ok(())
}

struct Source {
    entry: Weak<Mutex<Buffer>>,
    reader: LiveReader,
}

#[derive(Default)]
struct Sources {
    readers: BTreeMap<Arc<str>, Source>,
    pending: Vec<(Arc<str>, Entry)>,
}

/// The seam keeps tests at the cursor/buffer layer, without processes or sockets.
fn poll(
    service: &Watches,
    sources: &mut Sources,
    mut read: impl FnMut(&str, &mut LiveReader) -> Result<LivePage>,
    mut closed: impl FnMut() -> bool,
) {
    let registered = lock(&service.entries);
    sources.readers.retain(|scope, source| {
        registered
            .get(scope)
            .is_some_and(|entry| Weak::ptr_eq(&source.entry, &Arc::downgrade(entry)))
    });
    sources.pending.extend(
        registered
            .iter()
            .map(|(scope, entry)| (Arc::clone(scope), Arc::clone(entry))),
    );
    drop(registered);
    for (scope, entry) in sources.pending.drain(..) {
        if closed() {
            break;
        }
        if lock(&entry).users == 0 {
            continue;
        }
        let source = sources
            .readers
            .entry(scope.clone())
            .or_insert_with(|| Source {
                entry: Arc::downgrade(&entry),
                reader: LiveReader::default(),
            });
        let page = read(&scope, &mut source.reader);
        let mut buffer = lock(&entry);
        if buffer.users == 0 {
            continue;
        }
        match page {
            Ok(page) => buffer.publish(page),
            Err(_) => {
                source.reader.replay = true;
                buffer.unavailable();
            }
        }
    }
}

pub(crate) fn start_session_live(shared: &Arc<Shared>) -> Result<()> {
    let mut slot = lock(&shared.session_live.thread);
    ensure!(slot.is_none(), "session live worker already started");
    let shared_worker = Arc::clone(shared);
    let thread = std::thread::Builder::new()
        .name("corrald-session-live".into())
        .spawn(move || {
            let mut sources = Sources::default();
            while !shared_worker.lock().closed {
                poll(
                    &shared_worker.session_live.watches,
                    &mut sources,
                    |scope, reader| reader.read(&shared_worker, scope),
                    || shared_worker.lock().closed,
                );
                // Sleep after the complete pass: wakeups of unrelated topics cannot exceed 10 Hz.
                std::thread::sleep(SESSION_POLL);
            }
        })
        .context("starting session live worker")?;
    *slot = Some(thread);
    Ok(())
}

pub(crate) fn join_session_live(shared: &Shared) {
    let thread = lock(&shared.session_live.thread).take();
    if let Some(thread) = thread
        && thread.join().is_err()
    {
        eprintln!("corrald session live worker failed");
    }
}

struct Watch {
    service: Arc<Watches>,
    scope: Arc<str>,
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
pub(in crate::topics) struct SessionTopics {
    subscribers: BTreeMap<Arc<str>, Subscriber>,
}

impl SessionTopics {
    pub fn open(&mut self, shared: &Shared, open: SubOpen) -> Result<()> {
        let scope: Arc<str> = scope(open.scope)?.into();
        ensure!(
            self.subscribers.contains_key(&scope) || self.subscribers.len() < SESSION_SUBSCRIPTIONS,
            "too many session subscriptions"
        );
        let sub = subscribe(
            Arc::clone(&shared.session_live.watches),
            Arc::clone(&scope),
            shared.epoch(),
            open.since,
        )?;
        self.subscribers.insert(scope, sub);
        Ok(())
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
    ) -> Result<()> {
        for subscriber in self.subscribers.values_mut() {
            if let Some(frame) = subscriber.next(shared.epoch(), fleet)? {
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

    fn next(&mut self, epoch: u64, include_fleet: bool) -> Result<Option<Frame>> {
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
        let initial_fleet = include_fleet && self.version.is_none() && buffer.fleet.is_some();
        if !initial_fleet
            && gap.is_none()
            && buffer
                .events
                .back()
                .is_none_or(|(event, _)| event.seq <= start)
        {
            return Ok(None);
        }
        let fleet = include_fleet.then_some(buffer.fleet.as_ref()).flatten();
        let mut used = fleet.map_or(0, |(_, bytes)| *bytes);
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
                if include_fleet || !matches!(event.kind, SessionLiveKind::LiveFleet {}) {
                    Some(event.clone())
                } else {
                    None
                }
            })
            .collect();
        if seq == start && gap.is_none() && !initial_fleet {
            return Ok(None);
        }
        let version = buffer.version;
        let generation = buffer.generation;
        let next = buffer.next;
        let source_more = buffer.more;
        let fleet = fleet.map(|(fleet, _)| fleet.clone());
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
                fleet,
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
