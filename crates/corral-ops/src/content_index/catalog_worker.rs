//! One disposable background worker per live store in this process; cross-process batches
//! use a separate nonblocking builder lease. It never occupies the daemon's sync thread.
use crate::Ctx;
use anyhow::Result;

#[cfg(not(test))]
use std::collections::BTreeSet;
#[cfg(not(test))]
use std::path::PathBuf;
#[cfg(not(test))]
use std::sync::Mutex;

#[cfg(not(test))]
static RUNNING: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

#[cfg(test)]
pub(crate) fn schedule(_ctx: &Ctx, _force: bool) -> Result<()> {
    Ok(())
}

#[cfg(not(test))]
pub(crate) fn schedule(ctx: &Ctx, force: bool) -> Result<()> {
    use anyhow::Context;
    if !super::super::enabled(ctx) {
        return Ok(());
    }
    let root = super::root(ctx)?;
    if !force
        && !root.join("restart").is_file()
        && !root.join("progress.json").is_file()
        && let Some(manifest) = super::read_manifest(&root)?
        && !super::due(ctx, &manifest)?
    {
        return Ok(());
    }
    let mut ctx = ctx.clone();
    // A snapshot supplied by a read-only caller must never keep its view in a worker.
    if ctx.store.is_snapshot() {
        ctx.store = corral_core::Store::open_unobserved(ctx.store.root())?;
    }
    let mut running = RUNNING
        .lock()
        .map_err(|error| anyhow::anyhow!("locking catalog worker registry: {error}"))?;
    if !running.insert(root.clone()) {
        return Ok(());
    }
    let worker_root = root.clone();
    let spawn = std::thread::Builder::new()
        .name("corral-search-index".into())
        .spawn(move || {
            let result = (|| -> Result<()> {
                // Priority reduction is a condition of this optional worker: defer rather
                // than compete with foreground queries if the host refuses it.
                #[cfg(target_os = "linux")]
                rustix::process::nice(10).context("lowering catalog worker priority")?;
                ctx.reload_config()
                    .context("reloading catalog worker config")?;
                if !super::super::enabled(&ctx) {
                    return Ok(());
                }
                let cache = super::load_prefix_cache(&ctx)?;
                loop {
                    ctx.reload_config()
                        .context("reloading catalog worker config")?;
                    if !super::super::enabled(&ctx) {
                        break;
                    }
                    match super::advance_with_cache(&ctx, Some(&cache))? {
                        super::Step::More => {}
                        super::Step::Busy => break,
                        super::Step::Ready => {
                            if let Err(error) = super::control::clear_error(&ctx) {
                                eprintln!("search catalog error status unavailable: {error:#}");
                            }
                            break;
                        }
                    }
                }
                Ok(())
            })();
            if let Err(error) = result {
                super::super::deferred(&ctx, &error);
            }
            if let Ok(mut running) = RUNNING.lock() {
                running.remove(&worker_root);
            }
        });
    if let Err(error) = spawn {
        running.remove(&root);
        return Err(error).context("starting catalog background worker");
    }
    Ok(())
}
