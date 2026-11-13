//! Serialized read computations on an independently opened store, never the loop's connection.
use std::collections::HashMap;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use corral_core::Store;
use corral_ops::{Ctx, attention::AttentionBoard};
use corral_protocol::read::ReadKind;

use crate::state::Shared;

const MAX_AGE: Duration = Duration::from_secs(10);

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
            let mut cache = HashMap::<ReadKind, Arc<Document>>::new();
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
    cache: &mut HashMap<ReadKind, Arc<Document>>,
    compute: &mut impl FnMut(&Ctx, ReadKind, Option<AttentionBoard>) -> Result<Vec<u8>>,
) -> Result<Served> {
    let seq = ctx
        .store
        .view()
        .context("polling reader store sequence")?
        .seq()
        .0;
    if let Some(document) = cache.get(&kind)
        && document.seq == seq
        && document.computed.elapsed() < MAX_AGE
    {
        return Ok(Served {
            document: Arc::clone(document),
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
        shared
            .attention
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .as_ref()
            .filter(|(attention_seq, _)| *attention_seq == seq)
            .map(|(_, board)| board.clone())
    } else {
        None
    };
    // PM projection prepares derived state before pinning its own view. Keep
    // this handle live, and cache under the pre-computation sequence so writes
    // during preparation or evaluation invalidate reuse conservatively.
    let bytes = compute(ctx, kind, attention)?;
    let document = Arc::new(Document {
        as_of: jiff::Timestamp::now().to_string(),
        seq,
        bytes,
        computed: Instant::now(),
    });
    cache.insert(kind, Arc::clone(&document));
    Ok(Served {
        document,
        cached: false,
    })
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
