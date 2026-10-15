use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use corral_core::Store;

use super::{DiskSpace, ResourceProbe, ResourceSnapshot, existing_parent};

#[derive(Debug, Clone, Copy)]
pub struct SystemResourceProbe;

impl ResourceProbe for SystemResourceProbe {
    fn sample(&self, store: &Store) -> Result<ResourceSnapshot> {
        let cores = u32::try_from(
            std::thread::available_parallelism()
                .context("reading CPU count")?
                .get(),
        )
        .context("CPU count exceeds u32")?;
        let load_one =
            parse_load(&fs::read_to_string("/proc/loadavg").context("reading /proc/loadavg")?)?;
        let mem_available =
            parse_mem(&fs::read_to_string("/proc/meminfo").context("reading /proc/meminfo")?)?;
        let mut paths = BTreeSet::from([store.dir().to_path_buf()]);
        for name in store.config.projects.keys() {
            let project = store.project_path(name)?;
            paths.insert(project.join(".worktrees"));
            paths.insert(project.join("target"));
        }
        for task in store.list()? {
            for run in task.runs {
                if run.worktree.exists() {
                    paths.insert(run.worktree.join("target"));
                    paths.insert(run.worktree);
                }
            }
        }
        if let Some(target) = std::env::var_os("CARGO_TARGET_DIR").filter(|v| !v.is_empty()) {
            let target = PathBuf::from(target);
            paths.insert(if target.is_absolute() {
                target
            } else {
                std::env::current_dir()
                    .context("resolving CARGO_TARGET_DIR")?
                    .join(target)
            });
        }
        let disks = paths
            .into_iter()
            .map(|path| {
                let existing = existing_parent(&path)?;
                let stat = rustix::fs::statvfs(&existing)
                    .with_context(|| format!("reading free space for {}", path.display()))?;
                let free_bytes = stat
                    .f_bavail
                    .checked_mul(stat.f_frsize)
                    .context("disk free space overflow")?;
                Ok(DiskSpace { path, free_bytes })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(ResourceSnapshot {
            load_per_core: load_one / f64::from(cores),
            load_one,
            cores,
            mem_available,
            cpu_psi: optional_psi("/proc/pressure/cpu")?,
            mem_psi: optional_psi("/proc/pressure/memory")?,
            disks,
        })
    }
}

pub(super) fn parse_load(text: &str) -> Result<f64> {
    let value: f64 = text
        .split_whitespace()
        .next()
        .context("missing one-minute load")?
        .parse()
        .context("invalid one-minute load")?;
    ensure!(value.is_finite() && value >= 0.0, "invalid one-minute load");
    Ok(value)
}

pub(super) fn parse_mem(text: &str) -> Result<u64> {
    let line = text
        .lines()
        .find_map(|l| l.strip_prefix("MemAvailable:"))
        .context("missing MemAvailable")?;
    let mut words = line.split_whitespace();
    let value: u64 = words
        .next()
        .context("missing MemAvailable value")?
        .parse()
        .context("invalid MemAvailable")?;
    ensure!(words.next() == Some("kB"), "invalid MemAvailable unit");
    value.checked_mul(1024).context("MemAvailable overflow")
}

pub(super) fn parse_psi(text: &str) -> Result<f64> {
    let line = text
        .lines()
        .find(|l| l.starts_with("some "))
        .context("missing PSI some line")?;
    let value: f64 = line
        .split_whitespace()
        .find_map(|s| s.strip_prefix("avg10="))
        .context("missing PSI avg10")?
        .parse()
        .context("invalid PSI avg10")?;
    ensure!(
        value.is_finite() && (0.0..=100.0).contains(&value),
        "invalid PSI avg10"
    );
    Ok(value)
}

fn optional_psi(path: &str) -> Result<Option<f64>> {
    match fs::read_to_string(path) {
        Ok(text) => parse_psi(&text).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("reading {path}")),
    }
}
