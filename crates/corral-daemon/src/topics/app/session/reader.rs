use super::follower::{Follower, HostOpen};
use corral_protocol::subscription::TranscriptOp;
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use corral_engine::runner::pi::Client;
use corral_protocol::{
    session_state::SessionState,
    subscription::{
        LIVE_FLEET_BYTES, LIVE_STATE_BYTES, SessionFleet, SessionLiveEvent, SessionLiveGap,
        SessionLiveKind, SessionLiveToolState, SessionLiveTurnState,
    },
};
use corral_types::SessionRef;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::{
    SESSION_POLL, SESSION_STREAM_BURST, SESSION_STREAM_LINGER, SESSION_STREAM_POLL,
    buffer::{Buffer, Entry, Watches},
    lock,
};
use crate::{
    sessions::{children, forward::connect},
    state::Shared,
    views::json_len as json_bytes,
};

const LIVE_IO_TIMEOUT: Duration = Duration::from_millis(250);
const LIVE_HOST_EVENTS: usize = 512;
pub(super) const LIVE_EVENT_BYTES: usize = 8 * 1024;
const LIVE_PAYLOAD_BYTES: usize = 1024;
const LIVE_ERROR_BYTES: usize = 512;

/// The socket rings are independent; only the live ring contributes events to app pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Cursors {
    pub(super) lifecycle: u64,
    pub(super) live: u64,
}

#[derive(Deserialize)]
pub(super) struct RingPage {
    pub(super) last: u64,
    pub(super) next: u64,
    pub(super) gap: Option<SessionLiveGap>,
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
pub(super) struct HostPage {
    #[serde(flatten)]
    ring: RingPage,
    #[serde(default)]
    open: Option<HostOpen>,
    events: Vec<HostEvent>,
    more: bool,
    fleet: Option<SessionFleet>,
    pub(super) state: Option<SessionState>,
}

#[derive(Deserialize)]
struct HostEvent {
    seq: u64,
    at: String,
    #[serde(flatten)]
    kind: SessionLiveKind,
}

pub(super) struct LivePage {
    pub(super) generation: Option<u32>,
    pub(super) events: Vec<(SessionLiveEvent, usize)>,
    pub(super) next: u64,
    pub(super) host_tail: u64,
    pub(super) more: bool,
    pub(super) gap: Option<SessionLiveGap>,
    pub(super) fleet: Option<(SessionFleet, usize)>,
    pub(super) state: Option<(SessionState, usize)>,
}

pub(super) struct LiveReader {
    socket: Option<PathBuf>,
    journal: Option<PathBuf>,
    resolved_journal: Option<PathBuf>,
    journal_id: String,
    follower: Follower,
    operations: Vec<TranscriptOp>,
    open: Option<HostOpen>,
    /// A journal was followed: its messages are what subscribers were last given.
    shown: bool,
    pub(super) cursors: Cursors,
    // On loss, keep the observed cursors for restart detection, but ask for
    // retained events and the current fleet when the host comes back.
    replay: bool,
    generation: u32,
}

impl Default for LiveReader {
    fn default() -> Self {
        Self {
            socket: None,
            journal: None,
            resolved_journal: None,
            journal_id: String::new(),
            follower: Follower::default(),
            operations: Vec::new(),
            open: None,
            shown: false,
            cursors: Cursors::default(),
            replay: false,
            generation: 1,
        }
    }
}

impl LiveReader {
    /// Keeps what a visit made of the journal until it is published. The follower has read
    /// past it, so a visit whose page is not published must not lose it.
    fn followed(&mut self, ops: Vec<TranscriptOp>) {
        self.operations.extend(ops);
    }

    pub(super) fn read(&mut self, shared: &Shared, scope: &str) -> Result<LivePage> {
        let resolved = match crate::sessions::resolve(shared, scope) {
            Ok(resolved) => resolved,
            Err(error) => {
                if let Some(path) = &self.journal {
                    let ops = self
                        .follower
                        .visit(&self.journal_id, path, None, None, None);
                    self.followed(ops);
                }
                return Err(error);
            }
        };
        let old_socket = self.socket.clone();
        let old_generation = self.generation;
        let replay = self.replay;
        if replay || self.cursors.live == 0 {
            self.follower.reads_from_start();
        }
        let page = self.read_resolved(&resolved);
        if self.socket != old_socket
            || old_generation != self.generation
            || (replay && page.as_ref().is_ok_and(|p| p.generation.is_some()))
            || self.resolved_journal != resolved.transcript
            || (self.journal.is_none() && old_socket.is_none())
            || page
                .as_ref()
                .is_ok_and(|p| p.generation.is_some() && p.gap.is_some())
        {
            self.journal = if page.as_ref().is_ok_and(|page| page.generation.is_some()) {
                resolved.transcript_view().0
            } else {
                resolved.transcript.clone()
            };
        }
        self.resolved_journal.clone_from(&resolved.transcript);
        self.journal_id.clone_from(&resolved.id);
        let path = if page.as_ref().is_ok_and(|page| page.generation.is_some()) {
            self.journal.as_ref().or(resolved.transcript.as_ref())
        } else {
            resolved.transcript.as_ref().or(self.journal.as_ref())
        };
        let path = path.cloned();
        self.follow(
            &resolved.id,
            path.as_deref(),
            resolved.transcript_scope(),
            page.as_ref().ok(),
        );
        page
    }

    /// Follows the journal at `path`. A scope that names none any more (the quick chat it
    /// was an alias of ended) shows no messages: the last transcript followed is reset
    /// away, or a subscriber would be given the ended chat again.
    pub(super) fn follow(
        &mut self,
        id: &str,
        path: Option<&Path>,
        scope: Option<&Path>,
        page: Option<&LivePage>,
    ) {
        if let Some(path) = path {
            let ops = self.follower.visit(id, path, scope, page, self.open.take());
            self.followed(ops);
            self.shown = true;
        } else if std::mem::take(&mut self.shown) {
            self.follower = Follower::default();
            self.followed(vec![TranscriptOp::Reset { older: None }]);
        }
    }

    fn read_resolved(&mut self, resolved: &crate::sessions::Session) -> Result<LivePage> {
        self.open = None;
        if resolved.is_empty_quick() {
            if self.socket.take().is_some() {
                self.generation = self
                    .generation
                    .checked_add(1)
                    .context("live generation exhausted")?;
            }
            self.cursors = Cursors::default();
            self.replay = false;
            return Ok(LivePage {
                generation: None,
                events: Vec::new(),
                next: 0,
                host_tail: 0,
                more: false,
                gap: None,
                fleet: None,
                state: None,
            });
        }
        let live = self.read_host(resolved);
        match live {
            Err(_)
                if !resolved.unreachable
                    && (resolved.ended
                        || !matches!(resolved.session, Some(SessionRef::Pi { .. }))
                        || live.as_ref().is_err_and(crate::builds::disconnected)) =>
            {
                let Some(fleet) = children::fleet(resolved) else {
                    return live;
                };
                self.replay = true;
                Ok(LivePage {
                    generation: None,
                    events: Vec::new(),
                    next: 0,
                    host_tail: 0,
                    more: false,
                    gap: Some(SessionLiveGap {
                        after: self.cursors.live,
                        resumed: 0,
                        missing: 0,
                        reconcile: true,
                    }),
                    fleet: Some(bound_fleet(fleet)?),
                    state: None,
                })
            }
            _ => live,
        }
    }

    fn read_host(&mut self, resolved: &crate::sessions::Session) -> Result<LivePage> {
        ensure!(!resolved.ended, "session ended");
        let Some(SessionRef::Pi { socket, .. }) = &resolved.session else {
            anyhow::bail!("session has no Pi socket");
        };
        let rebound = self.socket.as_ref().is_some_and(|prev| prev != socket);
        let requested = if rebound || self.replay {
            Cursors::default()
        } else {
            self.cursors
        };
        let lifecycle = request(socket, "events_since", requested.lifecycle)?;
        let live = request(socket, "live_since", requested.live)?;
        let page = self.advance(requested, lifecycle, live, rebound, || {
            children::fleet(resolved)
        })?;
        self.socket = Some(socket.clone());
        Ok(page)
    }

    pub(super) fn advance(
        &mut self,
        requested: Cursors,
        lifecycle: RingPage,
        live: HostPage,
        rebound: bool,
        retained_fleet: impl FnOnce() -> Option<SessionFleet>,
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
                generation: Some(generation),
                events: Vec::new(),
                next: 0,
                host_tail: 0,
                more: live.ring.last != 0,
                gap: None,
                fleet: retained_fleet().map(bound_fleet).transpose()?,
                state: None,
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
            let mut kind = event.kind;
            if let SessionLiveKind::LiveTool { call, .. } = &mut kind
                && let std::borrow::Cow::Owned(key) = crate::views::transcript::tool_call_id(call)
            {
                *call = key;
            }
            let event = SessionLiveEvent {
                seq: event.seq,
                generation,
                at: event.at,
                kind,
            };
            let bytes = event_bytes(&event)?;
            events.push((event, bytes));
        }
        ensure!(
            prior == live.ring.next && live.more == (prior < live.ring.last),
            "invalid Pi live page end"
        );
        let state = live.state.map(bound_state).transpose()?;
        // Only load the bounded index on initial/replayed pages and fleet replacements,
        // including an explicit empty replacement. Idle polls leave the buffer alone.
        let retained = if self.socket.is_none() || self.replay || restarted || live.fleet.is_some()
        {
            retained_fleet()
        } else {
            None
        };
        let fleet = merge_fleet(live.fleet, retained)
            .map(bound_fleet)
            .transpose()?;
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
        self.open = live.open;
        self.generation = generation;
        Ok(LivePage {
            generation: Some(generation),
            events,
            next: live.ring.next,
            host_tail: live.ring.last,
            more: live.more,
            gap,
            fleet,
            state,
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

pub(super) fn event_bytes(event: &SessionLiveEvent) -> Result<usize> {
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

fn merge_fleet(live: Option<SessionFleet>, retained: Option<SessionFleet>) -> Option<SessionFleet> {
    let Some(mut live) = live else {
        return retained;
    };
    if let Some(mut retained) = retained {
        let live_ids: HashSet<_> = live
            .children
            .iter()
            .map(|child| child.id.as_str())
            .collect();
        retained
            .children
            .retain(|child| !live_ids.contains(child.id.as_str()));
        // The byte limiter drops older rows from the front. Don't evict a live row
        // to show a retained one; live mail and parallelism remain authoritative.
        retained.children.append(&mut live.children);
        live.children = retained.children;
    }
    Some(live)
}

pub(super) fn nonempty_fleet(fleet: &SessionFleet) -> bool {
    !fleet.children.is_empty() || !fleet.messages.is_empty() || fleet.messages_total != 0
}

pub(super) fn bound_fleet(mut fleet: SessionFleet) -> Result<(SessionFleet, usize)> {
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

pub(super) fn bound_state(mut state: SessionState) -> Result<(SessionState, usize)> {
    let mut bytes = json_bytes(&state)?;
    trim_state_tail(&mut state.notices, "notices", &mut bytes)?;
    trim_state_tail(&mut state.queue, "queue", &mut bytes)?;
    for card in state.cards.iter_mut().rev() {
        if bytes <= LIVE_STATE_BYTES {
            break;
        }
        bytes -= json_bytes(&card.title)? - 2; // Keep the field and its empty string.
        card.title.clear();
    }
    ensure!(bytes <= LIVE_STATE_BYTES, "live state exceeds byte bound");
    Ok((state, bytes + 16))
}

fn trim_state_tail<T: Serialize>(items: &mut Vec<T>, field: &str, bytes: &mut usize) -> Result<()> {
    while *bytes > LIVE_STATE_BYTES {
        let Some(item) = items.pop() else {
            break;
        };
        // An emptied array omits the entire field, including `,"field":[]`.
        let overhead = if items.is_empty() { field.len() + 6 } else { 1 };
        *bytes -= json_bytes(&item)? + overhead;
    }
    Ok(())
}

pub(super) struct Source {
    entry: Weak<Mutex<Buffer>>,
    reader: LiveReader,
    /// When its host last returned events: it is read at the streaming pace for a while after.
    streamed: Option<Instant>,
    /// It is not read again before this.
    due: Option<Instant>,
    /// Full pages followed at once since it last waited its pace.
    burst: u8,
}

#[derive(Default)]
pub(super) struct Sources {
    pub(super) readers: BTreeMap<Arc<str>, Source>,
    pending: Vec<(Arc<str>, Entry)>,
}

/// The seam keeps tests at the cursor/buffer layer, without processes or sockets: one read of
/// every watched scope, whatever its pace.
#[cfg(test)]
pub(super) fn poll(
    service: &Watches,
    sources: &mut Sources,
    read: impl FnMut(&str, &mut LiveReader) -> Result<LivePage>,
    closed: impl FnMut() -> bool,
) {
    pass(service, sources, || None, read, closed);
}

/// Reads every watched scope that is due by `clock` (all of them without one) and returns how
/// long until the next is. A scope whose host returned events within `SESSION_STREAM_LINGER`
/// is due again after `SESSION_STREAM_POLL`, so a reply being written reaches its watchers
/// in steps too short to see; a full page is followed at once, `SESSION_STREAM_BURST` times
/// running at most; a quiet scope waits `SESSION_POLL`.
pub(super) fn pass(
    service: &Watches,
    sources: &mut Sources,
    mut clock: impl FnMut() -> Option<Instant>,
    mut read: impl FnMut(&str, &mut LiveReader) -> Result<LivePage>,
    mut closed: impl FnMut() -> bool,
) -> Duration {
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
    // The earliest moment a visited scope is due again: the wait is counted from the end of
    // the pass, so the time later reads took is not waited a second time.
    let mut next: Option<Instant> = None;
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
                streamed: None,
                due: None,
                burst: 0,
            });
        if let Some(due) = source.due
            && clock().is_some_and(|now| now < due)
        {
            next = Some(next.map_or(due, |next| next.min(due)));
            continue;
        }
        let page = read(&scope, &mut source.reader);
        let (events, more) = page
            .as_ref()
            .map_or((false, false), |page| (!page.events.is_empty(), page.more));
        {
            let mut buffer = lock(&entry);
            if buffer.users == 0 {
                continue;
            }
            let reader = &mut source.reader;
            let ops = std::mem::take(&mut reader.operations);
            buffer.publish_transcript(ops);
            match page {
                Ok(page) => buffer.publish(page),
                Err(_) => {
                    source.reader.replay = true;
                    buffer.unavailable();
                }
            }
        }
        // Paced from the end of the read, so a host that answers slowly is not read back to back.
        let Some(now) = clock() else {
            continue;
        };
        if events {
            source.streamed = Some(now);
        }
        // A host that keeps a full page ready must not hold the worker every other scope
        // shares: after a burst it waits its pace like any streaming scope.
        let follow = events && more && source.burst < SESSION_STREAM_BURST;
        source.burst = if follow { source.burst + 1 } else { 0 };
        let pace = if follow {
            Duration::ZERO
        } else if source
            .streamed
            .is_some_and(|at| now.saturating_duration_since(at) < SESSION_STREAM_LINGER)
        {
            SESSION_STREAM_POLL
        } else {
            SESSION_POLL
        };
        let due = now + pace;
        source.due = Some(due);
        next = Some(next.map_or(due, |next| next.min(due)));
    }
    next.zip(clock()).map_or(SESSION_POLL, |(due, now)| {
        due.saturating_duration_since(now).min(SESSION_POLL)
    })
}
