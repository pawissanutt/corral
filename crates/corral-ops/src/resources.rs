//! Machine observations and persisted admission hysteresis; no escalation side effects.
use std::fmt::Debug;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use corral_core::Store;
pub use corral_core::{DiskSpace, ResourceSnapshot, ResourceState};
use jiff::Timestamp;

use crate::Ctx;

mod system;
pub use system::SystemResourceProbe;

pub trait ResourceProbe: Send + Sync + Debug {
    fn sample(&self, store: &Store) -> Result<ResourceSnapshot>;
}

/// An injectable, host-independent observation for callers and tests.
#[derive(Debug, Clone)]
pub struct FixedResourceProbe(pub ResourceSnapshot);

impl FixedResourceProbe {
    pub fn ctx(store: Store) -> Ctx {
        Ctx {
            resource_probe: Self::arc(),
            ..Ctx::from_store(store)
        }
    }

    pub fn healthy() -> Self {
        Self(ResourceSnapshot {
            load_per_core: 0.1,
            load_one: 0.1,
            cores: 1,
            mem_available: u64::MAX,
            cpu_psi: None,
            mem_psi: None,
            disks: vec![DiskSpace {
                path: PathBuf::from("/"),
                free_bytes: u64::MAX,
            }],
        })
    }

    pub fn arc() -> Arc<dyn ResourceProbe> {
        Arc::new(Self::healthy())
    }
}

impl ResourceProbe for FixedResourceProbe {
    fn sample(&self, _store: &Store) -> Result<ResourceSnapshot> {
        Ok(self.0.clone())
    }
}

/// Sample and save a coherent observation while holding the cross-process resource lock.
pub fn observe(ctx: &Ctx, now: Timestamp) -> Result<ResourceState> {
    let lock_path = ctx.store.dir().join("lock/resources.lock");
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("opening {}", lock_path.display()))?;
    lock.lock().context("locking resource observations")?;
    let state = evaluate(ctx, now)?;
    let path = ctx.store.state_dir().join("resources.json");
    fs::create_dir_all(ctx.store.state_dir()).context("creating resource state directory")?;
    let mut tmp = tempfile::NamedTempFile::new_in(ctx.store.state_dir())
        .context("creating temporary resource state")?;
    serde_json::to_writer(&mut tmp, &state).context("serializing resource state")?;
    tmp.flush().context("flushing resource state")?;
    tmp.persist(&path).context("persisting resource state")?;
    // Closing this non-inherited descriptor releases its advisory lock.
    drop(lock);
    Ok(state)
}

fn evaluate(ctx: &Ctx, now: Timestamp) -> Result<ResourceState> {
    let path = ctx.store.state_dir().join("resources.json");
    let previous: Option<ResourceState> = match fs::read(&path) {
        Ok(bytes) => Some(serde_json::from_slice(&bytes).context("parsing resource state")?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("reading resource state"),
    };
    let snapshot = ctx.resource_probe.sample(&ctx.store)?;
    let cfg = &ctx.store.config.resources;
    let min_mem = cfg.min_mem_bytes()?;
    let min_disk = cfg.min_disk_bytes()?;
    let disk_low = snapshot.disks.iter().any(|d| d.free_bytes < min_disk);
    let severe_now = snapshot.mem_available < min_mem || disk_low;
    // Integer arithmetic preserves exact byte boundaries, including large thresholds.
    let recovered_bytes = |value: u64, min: u64| u128::from(value) * 10 >= u128::from(min) * 11;
    let severe_clear = recovered_bytes(snapshot.mem_available, min_mem)
        && snapshot
            .disks
            .iter()
            .all(|d| recovered_bytes(d.free_bytes, min_disk));
    let severe = severe_now || (previous.as_ref().is_some_and(|p| p.severe) && !severe_clear);
    let recovered = snapshot.load_per_core <= cfg.max_load_per_core * 0.9 && severe_clear;
    let held = snapshot.load_per_core > cfg.max_load_per_core
        || severe_now
        || (previous.as_ref().is_some_and(|p| p.held) && !recovered);
    let mut reasons = Vec::new();
    if held {
        let load_bound = if previous.as_ref().is_some_and(|p| p.held) {
            cfg.max_load_per_core * 0.9
        } else {
            cfg.max_load_per_core
        };
        if snapshot.load_per_core > load_bound {
            reasons.push(format!(
                "load/core {:.2}, max {:.2}, load1 {:.2}, cores {}",
                snapshot.load_per_core, cfg.max_load_per_core, snapshot.load_one, snapshot.cores
            ));
        }
        if !recovered_bytes(snapshot.mem_available, min_mem) {
            reasons.push(format!(
                "memory available {} bytes, min {} bytes",
                snapshot.mem_available, min_mem
            ));
        }
        for disk in &snapshot.disks {
            if !recovered_bytes(disk.free_bytes, min_disk) {
                reasons.push(format!(
                    "disk {} free {} bytes, min {} bytes",
                    disk.path.display(),
                    disk.free_bytes,
                    min_disk
                ));
            }
        }
    }
    Ok(ResourceState {
        snapshot,
        reason: held.then(|| reasons.join("; ")),
        severe,
        held,
        observed_at: now,
    })
}

pub fn admission(ctx: &Ctx) -> Result<Option<String>> {
    Ok(observe(ctx, Timestamp::now())?
        .reason
        .map(|reason| format!("waiting: machine busy ({reason})")))
}

pub fn status(ctx: &Ctx) -> Result<ResourceState> {
    read_status(ctx)
}

/// Sample against the last persisted latch without mutating the store or creating locks.
pub fn read_status(ctx: &Ctx) -> Result<ResourceState> {
    evaluate(ctx, Timestamp::now())
}

pub fn read_admission(ctx: &Ctx) -> Result<Option<String>> {
    Ok(read_status(ctx)?
        .reason
        .map(|reason| format!("waiting: machine busy ({reason})")))
}

fn existing_parent(path: &Path) -> Result<PathBuf> {
    path.ancestors()
        .find(|p| p.exists())
        .map(Path::to_path_buf)
        .with_context(|| format!("no existing ancestor for {}", path.display()))
}

#[cfg(test)]
#[path = "resources/tests.rs"]
mod tests;
