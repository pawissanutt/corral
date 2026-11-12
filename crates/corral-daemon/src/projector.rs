//! The projector has its own store handle and never runs on the pass thread.
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use corral_ops::{
    Ctx,
    boardview::{Projection, cache_path},
};

use crate::state::Shared;

const SAVE_EVERY: Duration = Duration::from_secs(10);

pub(crate) fn start(ctx: &Ctx, shared: &Arc<Shared>) -> Result<JoinHandle<()>> {
    let ctx = ctx.clone();
    let path = cache_path(&ctx).context("locating daemon projection cache")?;
    let projection = match Projection::load(&path) {
        Some(mut projection) => match projection.fold(&ctx) {
            Ok(_) => projection,
            Err(error) => {
                eprintln!("corrald loading board cache: {error:#}; rebuilding");
                Projection::rebuild(&ctx).context("rebuilding daemon board projection")?
            }
        },
        None => Projection::rebuild(&ctx).context("building daemon board projection")?,
    };
    *shared.projection() = Some(projection);
    let shared = Arc::clone(shared);
    std::thread::Builder::new()
        .name("corrald-projector".into())
        .spawn(move || {
            let mut dirty = true;
            let mut saved = Instant::now();
            loop {
                // Observe the generation before folding: a nudge/pass racing the fold
                // must cause another fold rather than be lost in the following wait.
                let wake = shared.lock().projection_wake;
                if let Err(error) = fold(&ctx, &shared, &mut dirty) {
                    eprintln!("corrald projecting board: {error:#}");
                }
                let stopping = shared.lock().closed;
                if stopping && let Err(error) = fold(&ctx, &shared, &mut dirty) {
                    eprintln!("corrald final board fold: {error:#}");
                }
                if dirty && (stopping || saved.elapsed() >= SAVE_EVERY) {
                    saved = Instant::now();
                    match save(&shared, &path) {
                        Ok(()) => {
                            dirty = false;
                        }
                        Err(error) => eprintln!("corrald saving board cache: {error:#}"),
                    }
                }
                if stopping {
                    break;
                }
                let delay = if shared.board_subscribers.load(Ordering::Relaxed) > 0 {
                    Duration::from_millis(500)
                } else {
                    Duration::from_secs(2)
                };
                drop(
                    shared
                        .changed
                        .wait_timeout_while(shared.lock(), delay, |state| {
                            !state.closed && state.projection_wake == wake
                        })
                        .unwrap_or_else(|error| error.into_inner()),
                );
            }
        })
        .context("starting daemon projector")
}

fn fold(ctx: &Ctx, shared: &Shared, dirty: &mut bool) -> Result<()> {
    let seq = ctx
        .store
        .view()
        .context("polling board store sequence")?
        .seq();
    let mut projection = {
        let guard = shared.projection();
        let projection = guard.as_ref().context("daemon projection is unavailable")?;
        if seq == projection.folded_through {
            return Ok(());
        }
        projection.clone()
    };
    let previous = projection.folded_through;
    let outcome = projection
        .fold(ctx)
        .context("folding daemon board changes")?;
    *dirty |= outcome.reset || previous != outcome.folded_through;
    *shared.projection() = Some(projection);
    Ok(())
}

fn save(shared: &Shared, path: &Path) -> Result<()> {
    shared
        .projection()
        .as_ref()
        .context("daemon projection is unavailable")?
        .save(path)
        .context("saving daemon board projection")
}
