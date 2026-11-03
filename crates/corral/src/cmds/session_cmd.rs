use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use corral_ops::Ctx;

use super::start_dir;

pub(super) fn hook(root: &Option<PathBuf>, entry: &crate::entry::Entry) {
    let result = start_dir(root)
        .and_then(|start| Ctx::open_for_sync(&start))
        .and_then(|ctx| {
            entry.apply(&ctx.store)?;
            coalesced_sync(&ctx)
        });
    if let Err(e) = result {
        eprintln!("corral hook: {e:#}");
    }
}

/// herdr starts one hook per pane event, and each ran its own full sync pass, so bursts piled
/// passes on top of each other. One hook syncs at a time: a hook that finds one running marks
/// the store dirty and leaves, and the running hook makes one more pass for those marks.
/// Marking then retrying the lock once, and re-checking the mark after unlocking, leaves no
/// window in which a recorded event waits for a later event. The trailing pass is capped so a
/// continuous burst cannot keep one hook process alive; a mark left after it is taken by the
/// next hook or the board's periodic sync.
fn coalesced_sync(ctx: &Ctx) -> Result<()> {
    let dir = ctx.store.cache_root()?;
    std::fs::create_dir_all(&dir).context("creating hook sync state directory")?;
    let dirty = dir.join("hook-sync.dirty");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("hook-sync.lock"))
        .context("opening hook sync lock")?;
    for _ in 0..2 {
        if !try_lock(&lock)? {
            std::fs::write(&dirty, b"").context("marking hook sync dirty")?;
            if !try_lock(&lock)? {
                return Ok(());
            }
        }
        match std::fs::remove_file(&dirty) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).context("clearing hook sync mark"),
        }
        corral_ops::sync_hook(ctx)?;
        lock.unlock().context("unlocking hook sync")?;
        if !dirty.exists() {
            return Ok(());
        }
    }
    Ok(())
}

fn try_lock(lock: &std::fs::File) -> Result<bool> {
    match lock.try_lock() {
        Ok(()) => Ok(true),
        Err(std::fs::TryLockError::WouldBlock) => Ok(false),
        Err(std::fs::TryLockError::Error(e)) => Err(e).context("locking hook sync"),
    }
}

pub(super) fn bind(
    root: &Option<PathBuf>,
    socket: &Path,
    launch_record: &Path,
    entry: &crate::entry::Entry,
) -> Result<()> {
    let ctx = Ctx::open_for_sync(&start_dir(root)?)?;
    entry.apply(&ctx.store)?;
    let evidence = std::fs::read_to_string(launch_record)
        .with_context(|| format!("reading {}", launch_record.display()))?;
    ctx.store.bind_herdr_session(socket, &evidence)
}
