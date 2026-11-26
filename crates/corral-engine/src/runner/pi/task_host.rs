//! When a task's Pi socket disappears, its placement may be an idle shell or
//! herdr may itself be offline. Neither alone is evidence that the host died.
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
struct HostRecord {
    id: String,
    role: String,
    socket: PathBuf,
    pid: i32,
    #[serde(rename = "sessionFile")]
    session_file: Option<PathBuf>,
}

#[derive(Deserialize)]
struct CapabilityProcess {
    pid: i32,
}

#[cfg(target_os = "linux")]
pub(super) fn confirmed_absent(dir: &Path, socket: &Path) -> bool {
    read_identity(dir, socket).is_some_and(|record| pid_absent(record.pid))
}

/// Reconciliation only needs host liveness. Takeover must also wait for known
/// managed subprocesses, whose detached groups can outlive a failed host.
pub(super) fn takeover_absent(dir: &Path, socket: &Path) -> Result<bool> {
    let Some(record) = read_identity(dir, socket) else {
        return Ok(false);
    };
    if !pid_absent(record.pid) {
        return Ok(false);
    }
    let sessions = dir.join("sessions");
    let Some(session_file) = record.session_file else {
        return Ok(false);
    };
    ensure!(
        session_file.parent() == Some(sessions.as_path())
            && sessions
                .canonicalize()
                .context("resolve Pi session records")?
                == sessions,
        "Pi host records refer outside the canonical session directory"
    );
    // Reviewers can restart in the same task directory. Include registries
    // from previous hosts, not just the PID in the latest host record.
    for entry in fs::read_dir(&sessions).context("list recorded Pi capability processes")? {
        let entry = entry.context("inspect Pi session directory entry")?;
        let name = entry.file_name();
        let Some((_, host_pid)) = name
            .to_str()
            .and_then(|name| name.strip_suffix(".json"))
            .and_then(|name| name.rsplit_once(".capabilities-"))
        else {
            continue;
        };
        ensure!(
            host_pid.parse::<i32>().is_ok_and(|pid| pid > 1),
            "invalid Pi capability registry identity"
        );
        let bytes = read_bounded(&entry.path()).context("read Pi capability process registry")?;
        let processes: Vec<CapabilityProcess> =
            serde_json::from_slice(&bytes).context("parse Pi capability process registry")?;
        for process in processes {
            ensure!(process.pid > 1, "invalid recorded Pi capability PID");
            // Check both the directly tracked process and any surviving members
            // of its group. PID reuse conservatively fails closed.
            if !pid_absent(process.pid) || !pid_absent(-process.pid) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn pid_absent(pid: i32) -> bool {
    // SAFETY: signal zero only inspects existence; a negative PID probes its group.
    unsafe {
        libc::kill(pid, 0) == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
}

fn read_bounded(path: &Path) -> Option<Vec<u8>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let meta = file.metadata().ok()?;
    const MAX: u64 = 1 << 20;
    if !meta.is_file() || meta.len() > MAX {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= MAX).then_some(bytes)
}

fn read_identity(dir: &Path, socket: &Path) -> Option<HostRecord> {
    let bytes = read_bounded(&dir.join("session.json"))?;
    let record: HostRecord = serde_json::from_slice(&bytes).ok()?;
    let task = dir.parent()?.file_name()?.to_str()?;
    let run = dir.file_name()?.to_str()?;
    let numeric = |id: &str| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit());
    (record.pid > 0
        && record.socket == socket
        && task.strip_prefix("T-").is_some_and(numeric)
        && record.id == format!("{task}/{run}")
        && match record.role.as_str() {
            "worker" => run.strip_prefix('R').is_some_and(numeric),
            "reviewer" => run == "reviewer",
            _ => false,
        })
    .then_some(record)
}
