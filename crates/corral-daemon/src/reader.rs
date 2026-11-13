//! Serialized read computations on an independently opened store, never the loop's connection.
use std::collections::HashMap;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use corral_core::{
    Store,
    backend::{ChangeScope, ChangeSeq, ReadView},
};
use corral_ops::{Ctx, attention::AttentionBoard};
use corral_protocol::read::ReadKind;

use crate::state::Shared;

const MAX_AGE: Duration = Duration::from_secs(10);
const BOARD_MAX_AGE: Duration = Duration::from_secs(45);
const WARM_REFRESH_AGE: Duration = Duration::from_secs(30);
const WARM_FOR: Duration = Duration::from_secs(120);
const WARM_SPACING: Duration = Duration::from_secs(10);
const WARM_POLL: Duration = Duration::from_secs(1);
// Core writes this scope in sqlite/tx_storage.rs and backend/fake/storage.rs.
// It does not currently export a lease-area constant.
const LEASE_AREA: &str = "leases";
const MAX_FRESHNESS_CHANGES: usize = 512;

struct Cached {
    document: Arc<Document>,
    checked_through: u64,
}

struct Reader<C, N> {
    cache: HashMap<ReadKind, Cached>,
    compute: C,
    now: N,
    board_requested: Option<Instant>,
    warm_started: Option<Instant>,
    last_poll: Option<Instant>,
}

fn warm_due(
    kind: ReadKind,
    requested: Option<Instant>,
    started: Option<Instant>,
    computed: Option<Instant>,
    changed: bool,
    now: Instant,
) -> bool {
    kind == ReadKind::BoardReadback
        && requested.is_some_and(|at| now.saturating_duration_since(at) < WARM_FOR)
        && started.is_none_or(|at| now.saturating_duration_since(at) >= WARM_SPACING)
        && (changed
            || computed.is_none_or(|at| now.saturating_duration_since(at) > WARM_REFRESH_AGE))
}

pub(crate) struct Document {
    pub as_of: String,
    pub seq: u64,
    pub bytes: Vec<u8>,
    computed: Instant,
}

pub(crate) struct Served {
    pub document: Arc<Document>,
    pub cached: bool,
}

pub(crate) struct Job {
    pub kind: ReadKind,
    pub reply: mpsc::Sender<Result<Served>>,
}

pub(crate) fn start(ctx: &Ctx, shared: &Arc<Shared>) -> Result<()> {
    start_with_compute(ctx, shared, compute)
}

pub(crate) fn start_with_compute(
    ctx: &Ctx,
    shared: &Arc<Shared>,
    compute: impl FnMut(&Ctx, ReadKind, Option<AttentionBoard>) -> Result<Vec<u8>> + Send + 'static,
) -> Result<()> {
    let ctx = Ctx {
        store: Store::open_unobserved(ctx.store.root()).context("opening daemon reader store")?,
        herdr: ctx.herdr.clone(),
        resource_probe: Arc::clone(&ctx.resource_probe),
    };
    let (sender, receiver) = mpsc::channel::<Job>();
    let worker_shared = Arc::clone(shared);
    std::thread::Builder::new()
        .name("corrald-reader".into())
        .spawn(move || {
            let shared = worker_shared;
            let mut reader = Reader::new(compute, Instant::now);
            let mut pending = None;
            while !shared.lock().closed {
                let job = match pending
                    .take()
                    .map_or_else(|| receiver.recv_timeout(Duration::from_millis(500)), Ok)
                {
                    Ok(job) => Some(job),
                    Err(mpsc::RecvTimeoutError::Timeout) => match receiver.try_recv() {
                        Ok(job) => Some(job),
                        Err(mpsc::TryRecvError::Empty) => None,
                        Err(mpsc::TryRecvError::Disconnected) => break,
                    },
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                if let Some(job) = job {
                    let result = reader.read(&ctx, &shared, job.kind);
                    let _ = job.reply.send(result);
                } else if let Err(error) =
                    reader.warm(&ctx, &shared, || match receiver.try_recv() {
                        Ok(job) => {
                            pending = Some(job);
                            false
                        }
                        Err(mpsc::TryRecvError::Empty) => true,
                        Err(mpsc::TryRecvError::Disconnected) => false,
                    })
                {
                    eprintln!("corrald warming board read-back: {error:#}");
                }
            }
        })
        .context("starting daemon reader")?;
    *shared
        .reader
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(sender);
    Ok(())
}

pub(crate) fn request(shared: &Shared, kind: ReadKind) -> Option<mpsc::Receiver<Result<Served>>> {
    let (reply, receiver) = mpsc::channel();
    shared
        .reader
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()?
        .send(Job { kind, reply })
        .ok()?;
    Some(receiver)
}

impl<C, N> Reader<C, N>
where
    C: FnMut(&Ctx, ReadKind, Option<AttentionBoard>) -> Result<Vec<u8>>,
    N: Fn() -> Instant,
{
    fn new(compute: C, now: N) -> Self {
        Self {
            cache: HashMap::new(),
            compute,
            now,
            board_requested: None,
            warm_started: None,
            last_poll: None,
        }
    }

    fn read(&mut self, ctx: &Ctx, shared: &Shared, kind: ReadKind) -> Result<Served> {
        let now = (self.now)();
        if kind == ReadKind::BoardReadback {
            self.board_requested = Some(now);
        }
        self.last_poll = Some(now);
        let view = ctx.store.view().context("polling reader store sequence")?;
        let max_age = if kind == ReadKind::BoardReadback {
            BOARD_MAX_AGE
        } else {
            MAX_AGE
        };
        if let Some(cached) = self.cache.get_mut(&kind)
            && now.saturating_duration_since(cached.document.computed) < max_age
            && unchanged(view.as_ref(), cached.checked_through)
        {
            cached.checked_through = view.seq().0;
            return Ok(Served {
                document: Arc::clone(&cached.document),
                cached: true,
            });
        }
        let label = match kind {
            ReadKind::PmNext => "reader.pm_next",
            ReadKind::PmBrief => "reader.pm_brief",
            ReadKind::AgentPmBrief => "reader.agent_pm_brief",
            ReadKind::AgentPmNext => "reader.agent_pm_next",
            ReadKind::BoardReadback => "reader.board",
            ReadKind::Other => bail!("unknown read kind"),
        };
        self.refresh(ctx, shared, kind, view, label)
    }

    fn warm(&mut self, ctx: &Ctx, shared: &Shared, mut idle: impl FnMut() -> bool) -> Result<()> {
        let now = (self.now)();
        // Do not even open a store view while cold or rate-limited. Client
        // requests take priority in the receive loop; only idle time reaches here.
        if !warm_due(
            ReadKind::BoardReadback,
            self.board_requested,
            self.warm_started,
            None,
            true,
            now,
        ) || self
            .last_poll
            .is_some_and(|at| now.saturating_duration_since(at) < WARM_POLL)
        {
            return Ok(());
        }
        self.last_poll = Some(now);
        let view = ctx.store.view().context("polling warm board sequence")?;
        let cached = self.cache.get_mut(&ReadKind::BoardReadback);
        let (computed, changed) = match cached {
            Some(cached) => {
                let changed = !unchanged(view.as_ref(), cached.checked_through);
                if !changed {
                    cached.checked_through = view.seq().0;
                }
                (Some(cached.document.computed), changed)
            }
            None => (None, true),
        };
        if warm_due(
            ReadKind::BoardReadback,
            self.board_requested,
            self.warm_started,
            computed,
            changed,
            now,
        ) && idle()
        {
            self.warm_started = Some((self.now)());
            self.refresh(
                ctx,
                shared,
                ReadKind::BoardReadback,
                view,
                "reader.board.warm",
            )?;
        }
        Ok(())
    }

    fn refresh(
        &mut self,
        ctx: &Ctx,
        shared: &Shared,
        kind: ReadKind,
        view: Box<dyn ReadView + Send>,
        label: &'static str,
    ) -> Result<Served> {
        let _timing = corral_core::timing::span(label);
        let seq = view.seq().0;
        let attention = if kind == ReadKind::BoardReadback {
            attention(shared, view.as_ref())
        } else {
            None
        };
        drop(view);
        // Keep the reader handle live so PM projection can prepare derived
        // state before pinning its own view. Lease-only writes do not invalidate it.
        let bytes = (self.compute)(ctx, kind, attention)?;
        let document = Arc::new(Document {
            as_of: jiff::Timestamp::now().to_string(),
            seq,
            bytes,
            computed: (self.now)(),
        });
        self.cache.insert(
            kind,
            Cached {
                document: Arc::clone(&document),
                checked_through: seq,
            },
        );
        Ok(Served {
            document,
            cached: false,
        })
    }
}

/// Lease bookkeeping cannot change a readout. Gaps, failures, backwards views,
/// unknown scopes and scans exceeding the work bound conservatively invalidate it.
pub(crate) fn unchanged(view: &dyn ReadView, after: u64) -> bool {
    let current = view.seq();
    let mut cursor = ChangeSeq(after);
    if cursor >= current {
        return cursor == current;
    }
    let mut scanned = 0;
    while scanned < MAX_FRESHNESS_CHANGES {
        let limit = (MAX_FRESHNESS_CHANGES - scanned).min(128) as u32;
        let Ok(page) = view.changes(cursor, limit) else {
            return false;
        };
        if page.entries.is_empty() || page.entries.len() > limit as usize {
            return false;
        }
        scanned += page.entries.len();
        for entry in page.entries {
            if entry.seq <= cursor
                || entry.seq > current
                || entry.scopes.is_empty()
                || entry.scopes.iter().any(
                    |scope| !matches!(scope, ChangeScope::Record { area, .. } if area == LEASE_AREA),
                )
            {
                return false;
            }
            cursor = entry.seq;
        }
        if !page.more {
            return cursor == current;
        }
    }
    false
}

fn attention(shared: &Shared, view: &dyn ReadView) -> Option<AttentionBoard> {
    let seq = shared
        .attention
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
        .map(|(seq, _)| *seq)?;
    // Scan without holding Shared's mutex; the pass can publish a newer board.
    if !unchanged(view, seq) {
        return None;
    }
    let mut attention = shared
        .attention
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let (attention_seq, board) = attention.as_mut()?;
    if *attention_seq != seq {
        return None;
    }
    *attention_seq = view.seq().0;
    Some(board.clone())
}

fn compute(ctx: &Ctx, kind: ReadKind, attention: Option<AttentionBoard>) -> Result<Vec<u8>> {
    match kind {
        ReadKind::PmNext => {
            serde_json::to_vec(&corral_ops::pm_read::next(ctx)?).context("encoding PM next readout")
        }
        ReadKind::PmBrief => serde_json::to_vec(&corral_ops::pm_read::brief(ctx)?)
            .context("encoding PM brief readout"),
        ReadKind::AgentPmBrief => serde_json::to_vec(&corral_ops::pm_read::agent_brief(ctx)?)
            .context("encoding agent PM brief readout"),
        ReadKind::AgentPmNext => serde_json::to_vec(&corral_ops::pm_read::agent_next(ctx)?)
            .context("encoding agent PM next readout"),
        ReadKind::BoardReadback => {
            serde_json::to_vec(&corral_ops::pm_read::board_readback(ctx, attention)?)
                .context("encoding board read-back")
        }
        ReadKind::Other => bail!("unknown read kind"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::Fixture;
    use corral_core::Status;
    use std::cell::Cell;

    #[test]
    fn changed_warm_board_is_recomputed_before_the_next_request() {
        let fixture = Fixture::new();
        let shared = Shared::new();
        let base = Instant::now();
        let clock = Cell::new(base);
        let calls = Cell::new(0);
        let mut reader = Reader::new(
            |ctx: &Ctx, kind, _: Option<AttentionBoard>| {
                assert!(!ctx.store.is_snapshot());
                assert_eq!(kind, ReadKind::BoardReadback);
                calls.set(calls.get() + 1);
                Ok(b"{}".to_vec())
            },
            || clock.get(),
        );
        let first = reader
            .read(&fixture.ctx, &shared, ReadKind::BoardReadback)
            .unwrap();
        assert!(!first.cached);
        fixture
            .ctx
            .store
            .create("Changed", None, Status::Ready, "")
            .unwrap();
        clock.set(base + Duration::from_secs(1));
        reader.warm(&fixture.ctx, &shared, || false).unwrap();
        assert_eq!(calls.get(), 1, "queued requests must win over warming");
        clock.set(base + Duration::from_secs(2));
        reader.warm(&fixture.ctx, &shared, || true).unwrap();
        assert_eq!(calls.get(), 2);
        clock.set(base + Duration::from_secs(20));
        let next = reader
            .read(&fixture.ctx, &shared, ReadKind::BoardReadback)
            .unwrap();
        assert!(next.cached);
        assert!(next.document.seq > first.document.seq);
        assert_eq!(calls.get(), 2);

        // Age alone refreshes a warm result even without a store write.
        clock.set(base + Duration::from_secs(33));
        reader.warm(&fixture.ctx, &shared, || true).unwrap();
        assert_eq!(calls.get(), 3);
        fixture
            .ctx
            .store
            .create("Burst", None, Status::Ready, "")
            .unwrap();
        clock.set(base + Duration::from_secs(34));
        reader.warm(&fixture.ctx, &shared, || true).unwrap();
        assert_eq!(calls.get(), 3, "warm computations must be spaced");
        clock.set(base + Duration::from_secs(44));
        reader.warm(&fixture.ctx, &shared, || true).unwrap();
        assert_eq!(calls.get(), 4);
        let last_poll = reader.last_poll;
        clock.set(base + Duration::from_secs(140));
        reader.warm(&fixture.ctx, &shared, || true).unwrap();
        assert_eq!(calls.get(), 4);
        assert_eq!(reader.last_poll, last_poll, "cold readers must not poll");
    }

    #[test]
    fn board_warming_stops_when_no_request_arrived_for_120_seconds() {
        let base = Instant::now();
        let due = |requested, now| {
            warm_due(
                ReadKind::BoardReadback,
                requested,
                None,
                Some(base),
                true,
                now,
            )
        };
        assert!(!due(None, base));
        assert!(due(Some(base), base + Duration::from_secs(119)));
        assert!(!due(Some(base), base + Duration::from_secs(120)));
        assert!(!due(Some(base), base + Duration::from_secs(300)));
    }

    #[test]
    fn pm_read_kinds_are_never_recomputed_unasked() {
        let base = Instant::now();
        for kind in [
            ReadKind::PmNext,
            ReadKind::PmBrief,
            ReadKind::AgentPmBrief,
            ReadKind::AgentPmNext,
            ReadKind::Other,
        ] {
            for age in [1, 31, 121] {
                assert!(!warm_due(
                    kind,
                    Some(base),
                    None,
                    Some(base),
                    true,
                    base + Duration::from_secs(age),
                ));
            }
        }
    }
}
