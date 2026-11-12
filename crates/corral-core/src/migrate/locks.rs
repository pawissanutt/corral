//! The locks maintenance takes (plan section 12): `lock/migrate.lock` serializes maintenance
//! commands, `lock/dispatch.lock` keeps 0.3.x task writers out, and the store gate `lock/store`
//! taken exclusively proves no v2 connection exists.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::StoreLock;

const RETRY: Duration = Duration::from_millis(25);

fn lock_file(corral: &Path, name: &str) -> Result<fs::File> {
    let dir = corral.join("lock");
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(name);
    fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))
}

fn exclusive(corral: &Path, name: &str, wait: Duration, what: &str) -> Result<StoreLock> {
    let file = lock_file(corral, name)?;
    let deadline = Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(StoreLock::new(file)),
            Err(fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(RETRY);
            }
            Err(fs::TryLockError::WouldBlock) => {
                bail!("{what} is held by another process; nothing changed")
            }
            Err(fs::TryLockError::Error(error)) => {
                return Err(error).with_context(|| format!("taking {what}"));
            }
        }
    }
}

/// `lock/migrate.lock`: one maintenance command at a time.
pub(crate) fn migrate_lock(corral: &Path, wait: Duration) -> Result<StoreLock> {
    exclusive(
        corral,
        "migrate.lock",
        wait,
        "another corral migrate (lock/migrate.lock)",
    )
}

/// `lock/dispatch.lock`, the lock every 0.3.x task writer takes.
pub(crate) fn dispatch_lock(corral: &Path, wait: Duration) -> Result<StoreLock> {
    exclusive(
        corral,
        "dispatch.lock",
        wait,
        "the board dispatch lock (lock/dispatch.lock)",
    )
}

/// The store gate exclusively, retried for `wait`; `on_busy` runs once when another process
/// holds it. On timeout the refusal lists the registered holders (diagnostics only) and nothing
/// changes.
pub(crate) fn store_gate(
    corral: &Path,
    wait: Duration,
    on_busy: impl FnOnce() -> Result<()>,
) -> Result<StoreLock> {
    let deadline = Instant::now() + wait;
    let mut on_busy = Some(on_busy);
    loop {
        if let Some(gate) = crate::store::try_exclusive_store_gate(corral)? {
            return Ok(gate);
        }
        if let Some(busy) = on_busy.take() {
            busy()?;
        }
        if Instant::now() >= deadline {
            let holders = crate::store::store_holders(corral)?;
            let pids: Vec<String> = holders
                .iter()
                .map(|holder| {
                    let pid = holder.split('-').next().unwrap_or(holder);
                    format!("pid {pid} ({holder})")
                })
                .collect();
            bail!(
                "the store is open in other corral processes after waiting {}s: {}; stop the board, \
                 corrald and the herdr plugin hook, then retry; nothing changed",
                wait.as_secs(),
                if pids.is_empty() {
                    "no registered holder".to_owned()
                } else {
                    pids.join(", ")
                }
            );
        }
        std::thread::sleep(RETRY);
    }
}

/// The store gate shared, for a read-only look at a published database; `None` while
/// maintenance holds it exclusively.
pub(crate) fn store_gate_shared(corral: &Path) -> Result<Option<StoreLock>> {
    let file = lock_file(corral, crate::store::db::GATE)?;
    match file.try_lock_shared() {
        Ok(()) => Ok(Some(StoreLock::new(file))),
        Err(fs::TryLockError::WouldBlock) => Ok(None),
        Err(fs::TryLockError::Error(error)) => Err(error).context("taking the store gate"),
    }
}
