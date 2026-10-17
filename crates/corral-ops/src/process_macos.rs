//! Darwin's PID version is checked by the kernel when delivering each signal.
//! A start-time check followed by kill(2) would still have a PID-reuse race.
use std::time::Duration;

use anyhow::{Context, Result, bail};
use corral_core::StuckProcess;
use kqueue::{EventFilter, FilterFlag, Watcher};

#[repr(C)]
#[derive(Default)]
struct UniqueInfo {
    uuid: [u8; 16],
    unique_id: u64,
    parent_unique_id: u64,
    pid_version: i32,
    reserved: u32,
    reserved_more: [u64; 2],
}

// SAFETY: signatures and the 8-word audit token ABI are from Darwin libproc.h.
#[link(name = "proc")]
unsafe extern "C" {
    fn proc_signal_with_audittoken(token: *const [u32; 8], signal: i32) -> i32;
}

fn unique(pid: u32) -> Result<Option<UniqueInfo>> {
    let mut info = UniqueInfo::default();
    let len = i32::try_from(std::mem::size_of::<UniqueInfo>())?;
    // SAFETY: FFI boundary. repr(C) matches proc_uniqidentifierinfo; all fields
    // are initialized integers and the writable buffer is exactly len bytes.
    let read = unsafe {
        libc::proc_pidinfo(
            i32::try_from(pid)?,
            17, // PROC_PIDUNIQIDENTIFIERINFO
            0,
            std::ptr::from_mut(&mut info).cast(),
            len,
        )
    };
    if read == 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(None);
        }
        return Err(error).context("reading Darwin process identity");
    }
    if read != len {
        bail!("Darwin returned an incomplete process identity");
    }
    Ok(Some(info))
}

fn token(pid: u32, version: i32) -> [u32; 8] {
    // psignal_by_audit_token uses only the PID and PID version for lookup;
    // permission checks still use the calling process's credentials.
    let mut token = [0; 8];
    token[5] = pid;
    token[7] = u32::from_ne_bytes(version.to_ne_bytes());
    token
}

pub(super) fn identity(pid: u32) -> Result<Option<String>> {
    Ok(unique(pid)?.map(|info| {
        format!(
            "{}:{}:{}",
            sysinfo::System::boot_time(),
            info.unique_id,
            info.pid_version
        )
    }))
}

pub(crate) fn exit_watch(pid: u32) -> Result<Watcher> {
    let mut watcher = Watcher::new().context("creating process exit kqueue")?;
    watcher
        .add_pid(
            i32::try_from(pid)?,
            EventFilter::EVFILT_PROC,
            FilterFlag::NOTE_EXIT,
        )
        .context("registering process exit event")?;
    watcher
        .watch()
        .context("starting process exit observation")?;
    Ok(watcher)
}

pub(crate) fn wait_exit(watcher: &Watcher, timeout: Duration) -> Result<bool> {
    match watcher.poll(Some(timeout)) {
        Some(event) if event.is_err() => bail!("process exit kqueue returned {event:?}"),
        Some(_) => Ok(true),
        None => Ok(false),
    }
}

pub(crate) fn terminate(process: &StuckProcess) -> Result<()> {
    if process.pid <= 1 {
        bail!("refusing to signal a system PID");
    }
    let watcher = exit_watch(process.pid)?;
    let info = unique(process.pid)?.context("stuck process exited")?;
    let current = super::inspect(process.pid)?.context("stuck process exited")?;
    if current.started != process.started
        || current.argv != process.argv
        || process.started
            != format!(
                "{}:{}:{}",
                sysinfo::System::boot_time(),
                info.unique_id,
                info.pid_version
            )
    {
        bail!("stuck process identity changed; sync and confirm again");
    }
    let audit = token(process.pid, info.pid_version);
    for signal in [libc::SIGTERM, libc::SIGKILL] {
        // SAFETY: FFI boundary. audit is a live, aligned 8-word audit_token_t;
        // the kernel checks its PID version atomically before sending the signal.
        let result = unsafe { proc_signal_with_audittoken(&audit, signal) };
        if result != 0 {
            let error = std::io::Error::from_raw_os_error(result);
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(());
            }
            return Err(error).context("signalling retained Darwin process identity");
        }
        if wait_exit(&watcher, Duration::from_secs(3))? {
            return Ok(());
        }
    }
    bail!("stuck process has not exited after SIGKILL")
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_audit_token_preserves_pid_version_bits() {
        // Given a PID version whose high bit is set.
        let version = i32::MIN + 7;
        // When constructing the kernel identity for a signal.
        let token = super::token(123, version);
        // Then both lookup fields preserve their exact bit representation.
        assert_eq!(token[5], 123);
        assert_eq!(token[7], 0x8000_0007);
        assert_eq!(std::mem::size_of::<super::UniqueInfo>(), 56);
    }
}
