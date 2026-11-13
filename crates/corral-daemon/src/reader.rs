//! Serialized read computations on an independently opened store, never the loop's connection.
//! Idle time refreshes the classic board frame while someone watches it.
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use corral_core::{
    Store,
    backend::{ChangeSeq, ReadView},
};
use corral_ops::{Ctx, feed_delta::FeedDelta};
use corral_protocol::read::ReadKind;

use crate::classic::ClassicState;
use crate::state::Shared;

const MAX_AGE: Duration = Duration::from_secs(10);
/// Idle wake interval: the classic frame's refresh spacing.
const IDLE_POLL: Duration = Duration::from_millis(500);
const MAX_FRESHNESS_CHANGES: usize = 512;

struct Cached {
    document: Arc<Document>,
    checked_through: u64,
}

struct Reader<C, N> {
    cache: HashMap<ReadKind, Cached>,
    compute: C,
    now: N,
    classic: ClassicState,
}

pub(crate) struct Document {
    pub as_of: String,
    pub seq: u64,
    pub bytes: Arc<Vec<u8>>,
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
    compute: impl FnMut(&Ctx, ReadKind) -> Result<Vec<u8>> + Send + 'static,
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
            let mut failure: Option<String> = None;
            while !shared.lock().closed {
                let job = match pending.take() {
                    Some(job) => Some(job),
                    None => match receiver.recv_timeout(IDLE_POLL) {
                        Ok(job) => Some(job),
                        Err(mpsc::RecvTimeoutError::Timeout) => None,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    },
                };
                if let Some(job) = job {
                    let result = reader.read(&ctx, &shared, job.kind);
                    let _ = job.reply.send(result);
                }
                // Client requests keep priority: only an empty queue refreshes the frame.
                match receiver.try_recv() {
                    Ok(job) => {
                        pending = Some(job);
                        continue;
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                    Err(mpsc::TryRecvError::Disconnected) => break,
                }
                let force = shared.classic_force.swap(false, Ordering::Relaxed);
                match reader.classic.tick(&ctx, &shared, force) {
                    Ok(()) => failure = None,
                    Err(error) => {
                        let text = format!("{error:#}");
                        if failure.as_ref() != Some(&text) {
                            eprintln!("corrald refreshing the classic frame: {text}");
                        }
                        failure = Some(text);
                    }
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
    C: FnMut(&Ctx, ReadKind) -> Result<Vec<u8>>,
    N: Fn() -> Instant,
{
    fn new(compute: C, now: N) -> Self {
        Self {
            cache: HashMap::new(),
            compute,
            now,
            classic: ClassicState::new(),
        }
    }

    fn read(&mut self, ctx: &Ctx, shared: &Shared, kind: ReadKind) -> Result<Served> {
        let label = match kind {
            ReadKind::PmNext => "reader.pm_next",
            ReadKind::PmBrief => "reader.pm_brief",
            ReadKind::AgentPmBrief => "reader.agent_pm_brief",
            ReadKind::AgentPmNext => "reader.agent_pm_next",
            ReadKind::ClassicFrame => return self.classic_frame(ctx, shared),
            ReadKind::Other => bail!("unknown read kind"),
        };
        let now = (self.now)();
        let view = ctx.store.view().context("polling reader store sequence")?;
        if let Some(cached) = self.cache.get_mut(&kind)
            && now.saturating_duration_since(cached.document.computed) < MAX_AGE
            && unchanged(view.as_ref(), cached.checked_through)
        {
            cached.checked_through = view.seq().0;
            return Ok(Served {
                document: Arc::clone(&cached.document),
                cached: true,
            });
        }
        self.refresh(ctx, kind, view, label)
    }

    /// The published frame's bytes are shared, never copied per read.
    fn classic_frame(&mut self, ctx: &Ctx, shared: &Shared) -> Result<Served> {
        let before = shared
            .classic
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let published = self.classic.serve(ctx, shared)?;
        let cached = before.is_some_and(|before| Arc::ptr_eq(&before, &published));
        Ok(Served {
            document: Arc::new(Document {
                as_of: published.as_of.clone(),
                seq: published.seq,
                bytes: Arc::clone(&published.bytes),
                computed: (self.now)(),
            }),
            cached,
        })
    }

    fn refresh(
        &mut self,
        ctx: &Ctx,
        kind: ReadKind,
        view: Box<dyn ReadView + Send>,
        label: &'static str,
    ) -> Result<Served> {
        let _timing = corral_core::timing::span(label);
        let seq = view.seq().0;
        drop(view);
        // Keep the reader handle live so PM projection can prepare derived
        // state before pinning its own view. Lease-only writes do not invalidate it.
        let bytes = (self.compute)(ctx, kind)?;
        let document = Arc::new(Document {
            as_of: jiff::Timestamp::now().to_string(),
            seq,
            bytes: Arc::new(bytes),
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

/// Lease bookkeeping and the command journal cannot change a readout. Gaps, failures,
/// backwards views and scans exceeding the work bound conservatively invalidate it.
pub(crate) fn unchanged(view: &dyn ReadView, after: u64) -> bool {
    !FeedDelta::since(view, ChangeSeq(after), MAX_FRESHNESS_CHANGES).any()
}

fn compute(ctx: &Ctx, kind: ReadKind) -> Result<Vec<u8>> {
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
        // The classic frame is served from `ClassicState`, never computed here.
        ReadKind::ClassicFrame | ReadKind::Other => bail!("unknown read kind"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::Fixture;
    use corral_core::Status;
    use std::cell::Cell;

    #[test]
    fn pm_read_is_reused_until_the_store_changes_or_it_ages() {
        let fixture = Fixture::new();
        let shared = Shared::new();
        let base = Instant::now();
        let clock = Cell::new(base);
        let calls = Cell::new(0);
        let mut reader = Reader::new(
            |ctx: &Ctx, kind: ReadKind| {
                assert!(!ctx.store.is_snapshot());
                assert_eq!(kind, ReadKind::PmNext);
                calls.set(calls.get() + 1);
                Ok(b"{}".to_vec())
            },
            || clock.get(),
        );
        let first = reader
            .read(&fixture.ctx, &shared, ReadKind::PmNext)
            .unwrap();
        assert!(!first.cached);
        clock.set(base + Duration::from_secs(1));
        let second = reader
            .read(&fixture.ctx, &shared, ReadKind::PmNext)
            .unwrap();
        assert!(second.cached);
        assert_eq!(calls.get(), 1);
        fixture
            .ctx
            .store
            .create("Changed", None, Status::Ready, "")
            .unwrap();
        let third = reader
            .read(&fixture.ctx, &shared, ReadKind::PmNext)
            .unwrap();
        assert!(!third.cached);
        assert!(third.document.seq > first.document.seq);
        assert_eq!(calls.get(), 2);

        // Age alone recomputes a read even without a store write.
        clock.set(base + Duration::from_secs(12));
        let fourth = reader
            .read(&fixture.ctx, &shared, ReadKind::PmNext)
            .unwrap();
        assert!(!fourth.cached);
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn classic_warming_stops_when_no_request_arrived_for_120_seconds() {
        let base = Instant::now();
        let warm = crate::classic::warm;
        assert!(!warm(0, None, base));
        assert!(warm(1, None, base));
        assert!(warm(0, Some(base), base + Duration::from_secs(119)));
        assert!(!warm(0, Some(base), base + Duration::from_secs(120)));
        assert!(!warm(0, Some(base), base + Duration::from_secs(300)));
    }

    #[test]
    fn idle_reader_never_recomputes_pm_reads() {
        let fixture = Fixture::new();
        let shared = Shared::new();
        let calls = Cell::new(0);
        let mut reader = Reader::new(
            |_: &Ctx, _: ReadKind| {
                calls.set(calls.get() + 1);
                Ok(b"{}".to_vec())
            },
            Instant::now,
        );
        reader
            .read(&fixture.ctx, &shared, ReadKind::PmNext)
            .unwrap();
        fixture
            .ctx
            .store
            .create("Changed", None, Status::Ready, "")
            .unwrap();
        // Cold: no subscriber and no frame read, so idle time computes nothing and an
        // owner-requested recompute stays pending.
        reader.classic.tick(&fixture.ctx, &shared, true).unwrap();
        assert_eq!(calls.get(), 1);
        assert!(
            shared
                .classic
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_none()
        );
        assert!(shared.classic_force.load(Ordering::Relaxed));
    }
}
