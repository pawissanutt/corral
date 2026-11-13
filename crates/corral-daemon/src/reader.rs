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
// Core writes this scope in sqlite/tx_storage.rs and backend/fake/storage.rs.
// It does not currently export a lease-area constant.
const LEASE_AREA: &str = "leases";
const MAX_FRESHNESS_CHANGES: usize = 512;

struct Cached {
    document: Arc<Document>,
    checked_through: u64,
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
    mut compute: impl FnMut(&Ctx, ReadKind, Option<AttentionBoard>) -> Result<Vec<u8>> + Send + 'static,
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
            let mut cache = HashMap::<ReadKind, Cached>::new();
            while !shared.lock().closed {
                let job = match receiver.recv_timeout(Duration::from_millis(500)) {
                    Ok(job) => job,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                let result = read(&ctx, &shared, job.kind, &mut cache, &mut compute);
                let _ = job.reply.send(result);
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

fn read(
    ctx: &Ctx,
    shared: &Shared,
    kind: ReadKind,
    cache: &mut HashMap<ReadKind, Cached>,
    compute: &mut impl FnMut(&Ctx, ReadKind, Option<AttentionBoard>) -> Result<Vec<u8>>,
) -> Result<Served> {
    let view = ctx.store.view().context("polling reader store sequence")?;
    let seq = view.seq().0;
    if let Some(cached) = cache.get_mut(&kind)
        && cached.document.computed.elapsed() < MAX_AGE
        && unchanged(view.as_ref(), cached.checked_through)
    {
        cached.checked_through = seq;
        return Ok(Served {
            document: Arc::clone(&cached.document),
            cached: true,
        });
    }
    let label = match kind {
        ReadKind::PmNext => "reader.pm_next",
        ReadKind::PmBrief => "reader.pm_brief",
        ReadKind::BoardReadback => "reader.board",
        ReadKind::Other => bail!("unknown read kind"),
    };
    let _timing = corral_core::timing::span(label);
    let attention = if kind == ReadKind::BoardReadback {
        attention(shared, view.as_ref())
    } else {
        None
    };
    drop(view);
    // PM projection prepares derived state before pinning its own view. Keep
    // this handle live, and cache under the pre-computation sequence so writes
    // during preparation or evaluation invalidate reuse unless they are leases.
    let bytes = compute(ctx, kind, attention)?;
    let document = Arc::new(Document {
        as_of: jiff::Timestamp::now().to_string(),
        seq,
        bytes,
        computed: Instant::now(),
    });
    cache.insert(
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
        ReadKind::BoardReadback => {
            serde_json::to_vec(&corral_ops::pm_read::board_readback(ctx, attention)?)
                .context("encoding board read-back")
        }
        ReadKind::Other => bail!("unknown read kind"),
    }
}
