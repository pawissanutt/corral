//! Thread-scoped leases for synchronous commands. Only designated children inherit them:
//! a runner crash must not release a lease while its Git or gate subprocess still writes.
use std::{fs::File, process::Command};

use anyhow::Result;

#[cfg(unix)]
use std::{
    cell::RefCell,
    io,
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::process::CommandExt as _,
    },
    rc::{Rc, Weak},
};

#[cfg(unix)]
thread_local! {
    static LEASES: RefCell<Vec<Weak<File>>> = const { RefCell::new(Vec::new()) };
}

/// Keeps an already-held file lock alive locally and enrolls it for child inheritance.
/// Never unlock this file explicitly: a descendant may still own the open description.
pub struct CommandLease {
    #[cfg(unix)]
    _file: Rc<File>,
    #[cfg(not(unix))]
    _file: File,
}

impl CommandLease {
    pub fn hold(file: File) -> Self {
        #[cfg(unix)]
        let file = {
            let file = Rc::new(file);
            LEASES.with_borrow_mut(|leases| {
                leases.retain(|lease| lease.strong_count() != 0);
                leases.push(Rc::downgrade(&file));
            });
            file
        };
        Self { _file: file }
    }
}

/// Keeps the duplicate CLOEXEC descriptors open in the parent through `spawn`.
pub struct InheritedCommandLeases {
    #[cfg(unix)]
    _files: Vec<OwnedFd>,
}

pub fn inherit_command_leases(command: &mut Command) -> Result<InheritedCommandLeases> {
    #[cfg(unix)]
    {
        use anyhow::Context as _;

        let files = LEASES
            .with_borrow(|leases| {
                leases
                    .iter()
                    .filter_map(Weak::upgrade)
                    .map(|file| rustix::io::fcntl_dupfd_cloexec(&*file, 3))
                    .collect::<rustix::io::Result<Vec<_>>>()
            })
            .context("duplicate command checkout leases")?;
        if !files.is_empty() {
            let fds: Vec<_> = files.iter().map(AsRawFd::as_raw_fd).collect();
            // SAFETY: the caller holds the returned descriptors across spawn. The
            // child hook only calls async-signal-safe fcntl on private descriptors.
            unsafe {
                command.pre_exec(move || {
                    for fd in &fds {
                        if libc::fcntl(*fd, libc::F_SETFD, 0) == -1 {
                            return Err(io::Error::last_os_error());
                        }
                    }
                    Ok(())
                });
            }
        }
        Ok(InheritedCommandLeases { _files: files })
    }
    #[cfg(not(unix))]
    {
        let _ = command;
        Ok(InheritedCommandLeases {})
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs::TryLockError, process::Stdio};

    #[test]
    fn registered_checkout_lease_survives_the_parent_scope_until_child_exit() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        let writer = temp.reopen().unwrap();
        writer.lock_shared().unwrap();
        let lease = CommandLease::hold(writer);
        let mut command = Command::new("sh");
        command
            .args(["-c", "read -r line || true"])
            .stdin(Stdio::piped());
        let child_fd = inherit_command_leases(&mut command).unwrap();
        let mut child = command.spawn().unwrap();
        drop(child_fd);
        drop(lease);

        let probe = temp.reopen().unwrap();
        assert!(matches!(probe.try_lock(), Err(TryLockError::WouldBlock)));
        drop(child.stdin.take());
        assert!(child.wait().unwrap().success());
        probe.lock().unwrap();
    }
}
