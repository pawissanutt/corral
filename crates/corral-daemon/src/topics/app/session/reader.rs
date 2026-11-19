use std::{
    collections::BTreeMap,
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
    buffer::{Buffer, Entry, Watches},
    lock,
};
use crate::{sessions::forward::connect, state::Shared, views::json_len as json_bytes};

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
    pub(super) more: bool,
    pub(super) gap: Option<SessionLiveGap>,
    pub(super) fleet: Option<(SessionFleet, usize)>,
    pub(super) state: Option<(SessionState, usize)>,
}

pub(super) struct LiveReader {
    socket: Option<PathBuf>,
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
            cursors: Cursors::default(),
            replay: false,
            generation: 1,
        }
    }
}

impl LiveReader {
    pub(super) fn read(&mut self, shared: &Shared, scope: &str) -> Result<LivePage> {
        let resolved = crate::sessions::resolve(shared, scope)?;
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
                more: false,
                gap: None,
                fleet: None,
                state: None,
            });
        }
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

    pub(super) fn advance(
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
                generation: Some(generation),
                events: Vec::new(),
                next: 0,
                more: live.ring.last != 0,
                gap: None,
                fleet: None,
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
        let state = live.state.map(bound_state).transpose()?;
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
            generation: Some(generation),
            events,
            next: live.ring.next,
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
}

#[derive(Default)]
pub(super) struct Sources {
    pub(super) readers: BTreeMap<Arc<str>, Source>,
    pending: Vec<(Arc<str>, Entry)>,
}

/// The seam keeps tests at the cursor/buffer layer, without processes or sockets.
pub(super) fn poll(
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
