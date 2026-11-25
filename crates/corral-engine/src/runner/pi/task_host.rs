//! When a task's Pi socket disappears, its placement may be an idle shell or
//! herdr may itself be offline. Neither alone is evidence that the host died.
use serde::Deserialize;
use std::{
    fs::OpenOptions,
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
}

pub(super) fn confirmed_absent(dir: &Path, socket: &Path) -> bool {
    read_identity(dir, socket).is_some_and(|pid| {
        // Signal zero probes existence without signalling. PID reuse or a
        // permission failure is uncertain, so defer to normal reconciliation.
        unsafe {
            libc::kill(pid, 0) == -1
                && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        }
    })
}

fn read_identity(dir: &Path, socket: &Path) -> Option<i32> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(dir.join("session.json"))
        .ok()?;
    let meta = file.metadata().ok()?;
    const MAX: u64 = 1 << 20;
    if !meta.is_file() || meta.len() > MAX {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX {
        return None;
    }
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
    .then_some(record.pid)
}
