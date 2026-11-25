//! Apply a TCP-only Landlock layer; the launcher still owns the network namespace
//! and the supervisor syscall policy must deny other socket families/protocols.
use anyhow::{Context, Result, ensure};
use std::{
    io,
    mem::size_of,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
};

use super::PORT;

// Linux uapi <linux/landlock.h>, ABI 4 prefix. Newer structs are backwards compatible.
const VERSION: libc::c_uint = 1;
const NET_PORT: libc::c_int = 2;
const BIND_TCP: u64 = 1;
const CONNECT_TCP: u64 = 2;

#[repr(C)]
struct RulesetAttr {
    handled_access_fs: u64,
    handled_access_net: u64,
}

#[repr(C)]
struct NetPortAttr {
    allowed_access: u64,
    port: u64,
}

fn version() -> Result<libc::c_long> {
    // SAFETY: version query takes a null attribute pointer and zero size.
    let abi = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<RulesetAttr>(),
            0usize,
            VERSION,
        )
    };
    if abi == -1 {
        return Err(io::Error::last_os_error()).context("query fixture Landlock ABI");
    }
    Ok(abi)
}

/// Must run on the sending supervisor, while single-threaded, before any provider socket
/// is opened. Restrictions are inherited by children; they do not revoke existing sockets.
pub(in crate::lite_analyst) fn restrict_egress() -> Result<()> {
    ensure!(
        version()? >= 4,
        "fixture requires Landlock TCP ABI 4 or newer"
    );
    ensure!(
        rustix::thread::no_new_privs().context("query fixture supervisor no_new_privs")?,
        "fixture supervisor requires no_new_privs"
    );
    let attr = RulesetAttr {
        handled_access_fs: 0,
        handled_access_net: BIND_TCP | CONNECT_TCP,
    };
    // SAFETY: pass the valid ABI 4 prefix and its exact size, kernel copies synchronously.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            &raw const attr,
            size_of::<RulesetAttr>(),
            0u32,
        )
    };
    if fd == -1 {
        return Err(io::Error::last_os_error()).context("create fixture Landlock ruleset");
    }
    // SAFETY: successful create_ruleset returned a new, owned descriptor.
    let ruleset = unsafe { OwnedFd::from_raw_fd(fd as _) };
    let port = NetPortAttr {
        allowed_access: CONNECT_TCP,
        port: PORT.into(),
    };
    // SAFETY: pass an open ruleset FD and a live valid NET_PORT attribute pointer.
    let added = unsafe {
        libc::syscall(
            libc::SYS_landlock_add_rule,
            ruleset.as_raw_fd(),
            NET_PORT,
            &raw const port,
            0u32,
        )
    };
    if added != 0 {
        return Err(io::Error::last_os_error())
            .context("add pinned TCP port to fixture Landlock ruleset");
    }
    // SAFETY: restrict this thread with the live ruleset; no flags are needed at ABI 4.
    let applied =
        unsafe { libc::syscall(libc::SYS_landlock_restrict_self, ruleset.as_raw_fd(), 0u32) };
    if applied != 0 {
        return Err(io::Error::last_os_error()).context("apply fixture Landlock ruleset");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustix::{
        io::Errno,
        net::{self, AddressFamily, SocketFlags, SocketType, ipproto},
    };
    use std::net::{Ipv4Addr, SocketAddrV4};

    #[test]
    fn tcp_landlock_fails_closed_or_enforces_denials() {
        // Landlock and no_new_privs are per-thread; do not change other tests or
        // make any authorized connection in the ambient namespace.
        std::thread::spawn(|| {
            if !matches!(version(), Ok(4..)) {
                assert!(restrict_egress().is_err());
                return;
            }
            rustix::thread::set_no_new_privs(true).unwrap();
            restrict_egress().unwrap();
            let fd = net::socket_with(
                AddressFamily::INET,
                SocketType::STREAM,
                SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
                Some(ipproto::TCP),
            )
            .unwrap();
            let unapproved = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0);
            assert_eq!(net::bind(&fd, &unapproved), Err(Errno::ACCESS));
            assert_eq!(net::connect(&fd, &unapproved), Err(Errno::ACCESS));
        })
        .join()
        .unwrap();
    }
}
