//! Bounded local I/O and kernel/pane ownership evidence. Only the captured Linux
//! process-token format is supported; other platforms fail closed.
use std::io::Read;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use serde::de::DeserializeOwned;

use super::{Session, rejected};
use crate::delivery::SendOutcome;
use crate::watchdog::{ProcessControl, SystemProcesses};

pub(super) fn read(path: &Path, cap: u64) -> Result<Vec<u8>> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
    }
    let file = options
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    ensure!(
        file.metadata().context("inspecting Claude file")?.is_file(),
        "Claude input must be a regular file"
    );
    let mut bytes = Vec::new();
    file.take(cap + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {}", path.display()))?;
    ensure!(
        bytes.len() as u64 <= cap,
        "Claude input exceeds {cap} bytes"
    );
    Ok(bytes)
}

pub(super) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_slice(&read(path, 64 * 1024)?)
        .with_context(|| format!("parsing {}", path.display()))
}

pub(super) fn in_pane(
    session: &Session,
    foreground: &[corral_herdr::ForegroundProcess],
) -> Result<bool> {
    let Some(process) = SystemProcesses.inspect(session.pid)? else {
        return Ok(false);
    };
    if process.started != session.proc_start {
        return Ok(false);
    }
    let mut pid = session.pid;
    // Bound ancestry walk; no process inventory or history scan.
    for _ in 0..64 {
        if foreground.iter().any(|p| p.pid == pid) {
            return Ok(true);
        }
        let Some(parent) = SystemProcesses.inspect(pid)? else {
            return Ok(false);
        };
        if parent.parent_pid <= 1 || parent.parent_pid == pid {
            return Ok(false);
        }
        pid = parent.parent_pid;
    }
    Ok(false)
}

#[cfg(unix)]
pub(super) fn safe_socket(path: &Path) -> Result<()> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    ensure!(path.is_absolute(), "Claude socket must be absolute");
    let socket = std::fs::symlink_metadata(path).context("inspecting Claude socket")?;
    let parent = std::fs::symlink_metadata(path.parent().context("Claude socket has no parent")?)
        .context("inspecting Claude socket directory")?;
    let uid = rustix::process::getuid().as_raw();
    ensure!(
        socket.file_type().is_socket() && socket.uid() == uid,
        "Claude socket is not owned by this user"
    );
    ensure!(
        parent.is_dir() && parent.uid() == uid && parent.permissions().mode() & 0o777 == 0o700,
        "Claude socket directory must be owned by this user with mode 0700"
    );
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn safe_socket(_path: &Path) -> Result<()> {
    anyhow::bail!("Claude socket protocol is unverified on this platform")
}

#[cfg(target_os = "linux")]
pub(super) fn write_frame(session: &Session, bytes: &[u8], timeout: Duration) -> SendOutcome {
    use rustix::net::{
        AddressFamily, SocketAddrUnix, SocketFlags, SocketType, connect, socket_with,
    };
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    use std::time::Instant;

    let deadline = Instant::now() + timeout;
    let open = (|| {
        safe_socket(&session.messaging_socket_path)?;
        let fd = socket_with(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
            None,
        )
        .context("opening Claude socket")?;
        connect(
            &fd,
            &SocketAddrUnix::new(&session.messaging_socket_path)
                .context("encoding Claude socket")?,
        )
        .context("connecting Claude inbox")?;
        let stream = UnixStream::from(fd);
        let peer = rustix::net::sockopt::socket_peercred(&stream)
            .context("checking Claude peer credentials")?;
        ensure!(
            peer.uid == rustix::process::getuid() && peer.pid.as_raw_pid() as u32 == session.pid,
            "Claude socket process changed"
        );
        let process = SystemProcesses
            .inspect(session.pid)?
            .context("Claude process exited")?;
        ensure!(
            process.started == session.proc_start,
            "Claude process generation changed"
        );
        stream
            .set_nonblocking(false)
            .context("configuring Claude socket")?;
        Ok::<_, anyhow::Error>(stream)
    })();
    let mut stream = match open {
        Ok(stream) => stream,
        Err(error) => return rejected(format!("Claude connect: {error:#}")),
    };
    let mut written = 0;
    while written < bytes.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let result = if remaining.is_zero() {
            Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Claude send deadline",
            ))
        } else {
            stream
                .set_write_timeout(Some(remaining))
                .and_then(|()| stream.write(&bytes[written..]))
        };
        match result {
            Ok(0) => return write_error(written, "Claude socket closed during write".into()),
            Ok(count) => written += count,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return write_error(written, format!("Claude write: {e}")),
        }
    }
    // The protocol has no on-connection guard acknowledgement: a completed write is
    // only Accepted. In particular it is not a delivered receipt or permission approval.
    SendOutcome::Accepted
}

#[cfg(not(target_os = "linux"))]
pub(super) fn write_frame(_session: &Session, _bytes: &[u8], _timeout: Duration) -> SendOutcome {
    rejected("Claude socket protocol is unverified on this platform".into())
}

#[cfg(target_os = "linux")]
fn write_error(written: usize, reason: String) -> SendOutcome {
    if written == 0 {
        rejected(reason)
    } else {
        SendOutcome::Uncertain { reason }
    }
}
