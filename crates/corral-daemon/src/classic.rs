//! The daemon-computed classic board frame (contract B.6.2): refreshed on the reader thread
//! while someone watches, encoded once per published revision and served as the same bytes
//! to every board.
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use corral_core::backend::ReadView;
use corral_ops::{
    Ctx,
    attention::AttentionBoard,
    classic::{ClassicCache, ClassicFrame, ClassicInputs},
};

use crate::state::Shared;

/// A `ClassicFrame` read keeps the frame refreshed this long without a subscriber.
const WARM_FOR: Duration = Duration::from_secs(120);
/// Refreshes are coalesced to at most one per interval.
const SPACING: Duration = Duration::from_millis(500);

/// One encoded frame revision; every reader and notice of that revision shares it.
pub(crate) struct Published {
    pub epoch: u64,
    pub rev: u64,
    pub seq: u64,
    pub as_of: String,
    pub bytes: Arc<Vec<u8>>,
}

pub(crate) struct ClassicState {
    cache: ClassicCache,
    epoch: u64,
    requested: Option<Instant>,
    last_tick: Option<Instant>,
    seen_pass: u64,
}

/// Whether the frame is worth refreshing without a read waiting for it.
pub(crate) fn warm(subscribers: usize, requested: Option<Instant>, now: Instant) -> bool {
    subscribers > 0 || requested.is_some_and(|at| now.saturating_duration_since(at) < WARM_FOR)
}

impl ClassicState {
    pub fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos() as u64);
        Self {
            cache: ClassicCache::new(),
            epoch: nanos ^ u64::from(std::process::id()),
            requested: None,
            last_tick: None,
            seen_pass: 0,
        }
    }

    /// Warm when `shared.classic_subscribers > 0` or a ClassicFrame read came within 120 s.
    /// Runs at most every 500 ms (the cache itself honours its `next_due`); publishes on change.
    /// A `force` that cannot run now stays pending in `shared.classic_force`.
    pub fn tick(&mut self, ctx: &Ctx, shared: &Shared, force: bool) -> Result<()> {
        let now = Instant::now();
        if !warm(
            shared.classic_subscribers.load(Ordering::Relaxed),
            self.requested,
            now,
        ) || !self.spaced(now)
        {
            if force {
                shared.classic_force.store(true, Ordering::Relaxed);
            }
            return Ok(());
        }
        self.refresh(ctx, shared, force, now)
    }

    /// For a read: tick first if the last tick is older than 500 ms, then serve `Published`.
    pub fn serve(&mut self, ctx: &Ctx, shared: &Shared) -> Result<Arc<Published>> {
        let now = Instant::now();
        self.requested = Some(now);
        if self.spaced(now) {
            let force = shared.classic_force.swap(false, Ordering::Relaxed);
            let refreshed = self.refresh(ctx, shared, force, now);
            if refreshed.is_err() && force {
                shared.classic_force.store(true, Ordering::Relaxed);
            }
            refreshed?;
        }
        shared
            .classic
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
            .context("classic frame is not computed yet")
    }

    fn spaced(&self, now: Instant) -> bool {
        self.last_tick
            .is_none_or(|at| now.saturating_duration_since(at) >= SPACING)
    }

    fn refresh(&mut self, ctx: &Ctx, shared: &Shared, force: bool, now: Instant) -> Result<()> {
        let _timing = corral_core::timing::span("classic.tick");
        self.last_tick = Some(now);
        let completed = shared.lock().completed_seq;
        // A completed daemon pass supplies its attention board when nothing but lease
        // bookkeeping was written since; otherwise the cache derives attention itself.
        let attention = if completed == self.seen_pass {
            None
        } else {
            self.seen_pass = completed;
            let view = ctx.store.view().context("reading classic frame sequence")?;
            pass_attention(shared, view.as_ref())
        };
        let refreshed = self.cache.refresh(
            ctx,
            ClassicInputs {
                now: jiff::Timestamp::now(),
                attention: attention.map(Ok),
                force,
            },
        )?;
        // An earlier encode failure leaves the cache's frame unpublished.
        let Some(frame) = refreshed.or_else(|| self.cache.current()) else {
            return Ok(());
        };
        let current = shared
            .classic
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .is_some_and(|published| published.epoch == self.epoch && published.rev == frame.rev);
        if !current {
            self.publish(shared, &frame)?;
        }
        Ok(())
    }

    fn publish(&self, shared: &Shared, frame: &ClassicFrame) -> Result<()> {
        let bytes = {
            let _timing = corral_core::timing::span("classic.encode");
            serde_json::to_vec(frame).context("encoding classic frame")?
        };
        let published = Arc::new(Published {
            epoch: self.epoch,
            rev: frame.rev,
            seq: frame.seq,
            as_of: frame.as_of.to_string(),
            bytes: Arc::new(bytes),
        });
        *shared
            .classic
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(published);
        Ok(())
    }
}

/// The last pass's attention board, if the store is unchanged since it was computed.
fn pass_attention(shared: &Shared, view: &dyn ReadView) -> Option<AttentionBoard> {
    let seq = shared
        .attention
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
        .map(|(seq, _)| *seq)?;
    // Scan without holding Shared's mutex; the pass can publish a newer board.
    if !crate::reader::unchanged(view, seq) {
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
