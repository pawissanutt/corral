//! Native inventory plus kernel-bound identities, never a bare PID kill.
use std::path::Path;

use anyhow::{Context, Result, bail};
use corral_core::StuckProcess;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::ProcessSnapshot;

#[cfg(target_os = "macos")]
#[path = "process_macos.rs"]
pub(crate) mod platform;
#[cfg(windows)]
#[path = "process_windows.rs"]
pub(crate) mod platform;

pub(crate) use platform::terminate;

fn refresh(system: &mut System, pids: &[Pid]) {
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(pids),
        true,
        ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_cwd(UpdateKind::Always)
            .with_environ(UpdateKind::Always),
    );
}

pub(crate) fn inspect(pid: u32) -> Result<Option<ProcessSnapshot>> {
    let Some(started) = platform::identity(pid)? else {
        return Ok(None);
    };
    let mut system = System::new();
    refresh(&mut system, &[Pid::from_u32(pid)]);
    let Some(process) = system.process(Pid::from_u32(pid)) else {
        return Ok(None);
    };
    if matches!(
        process.status(),
        sysinfo::ProcessStatus::Zombie | sysinfo::ProcessStatus::Dead
    ) {
        return Ok(None);
    }
    if platform::identity(pid)?.as_ref() != Some(&started) {
        return Ok(None);
    }
    Ok(Some(ProcessSnapshot {
        parent_pid: process.parent().map_or(0, Pid::as_u32),
        elapsed_secs: process.run_time(),
        argv: process
            .cmd()
            .iter()
            .map(|s| s.to_string_lossy().into())
            .collect(),
        started,
    }))
}

fn inventory() -> Result<System> {
    let mut system = System::new();
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing());
    if system.process(Pid::from_u32(std::process::id())).is_none() {
        bail!("native process inventory is unavailable");
    }
    Ok(system)
}

pub(crate) fn residents(path: &Path) -> Result<Vec<StuckProcess>> {
    let mut system = inventory()?;
    let pids: Vec<_> = system.processes().keys().copied().collect();
    let mut result = Vec::new();
    for pid in pids {
        // Identity surrounds the cwd/argv read; a recycled PID cannot inherit membership.
        let before = platform::identity(pid.as_u32());
        refresh(&mut system, &[pid]);
        if let Some(process) = system.process(pid)
            && process
                .cwd()
                .and_then(|cwd| cwd.canonicalize().ok())
                .is_some_and(|cwd| cwd.starts_with(path))
        {
            let started = before?.context("resident process disappeared during inspection")?;
            if platform::identity(pid.as_u32())?.as_ref() != Some(&started) {
                bail!("resident process identity changed during cleanup; retry");
            }
            result.push(StuckProcess {
                pid: pid.as_u32(),
                started,
                argv: process
                    .cmd()
                    .iter()
                    .map(|s| s.to_string_lossy().into())
                    .collect(),
            });
        }
    }
    Ok(result)
}

/// Read a marker only while the resident retains its recorded creation identity.
pub(crate) fn ownership_marker(process: &StuckProcess) -> Result<Option<String>> {
    if platform::identity(process.pid)?.as_ref() != Some(&process.started) {
        return Ok(None);
    }
    let pid = Pid::from_u32(process.pid);
    let mut system = System::new();
    refresh(&mut system, &[pid]);
    let marker = system.process(pid).and_then(|p| {
        p.environ().iter().find_map(|value| {
            value
                .to_str()?
                .strip_prefix("CORRAL_PROCESS_OWNER=")
                .map(str::to_owned)
        })
    });
    if platform::identity(process.pid)?.as_ref() != Some(&process.started) {
        return Ok(None);
    }
    Ok(marker)
}

pub(crate) fn has_token(token: &str) -> Result<bool> {
    let mut system = inventory()?;
    let pids: Vec<_> = system.processes().keys().copied().collect();
    let needle = format!("CORRAL_HOOK_TOKEN={token}");
    for pid in pids {
        refresh(&mut system, &[pid]);
        if system
            .process(pid)
            .is_some_and(|p| p.environ().iter().any(|value| value == needle.as_str()))
        {
            return Ok(true);
        }
    }
    Ok(false)
}
