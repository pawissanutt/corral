use rustix::event::{PollFd, PollFlags, Timespec};
use std::io;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Instant;

pub(super) fn connect(socket: &Path, deadline: Instant) -> Result<UnixStream, String> {
    use rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType};
    let connect = || -> io::Result<UnixStream> {
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let flags = SocketFlags::NONBLOCK | SocketFlags::CLOEXEC;
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        let flags = SocketFlags::empty();
        let fd = rustix::net::socket_with(AddressFamily::UNIX, SocketType::STREAM, flags, None)?;
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        {
            let flags = rustix::io::fcntl_getfd(&fd)?;
            rustix::io::fcntl_setfd(&fd, flags | rustix::io::FdFlags::CLOEXEC)?;
        }
        let stream = UnixStream::from(fd);
        #[cfg(not(any(target_os = "linux", target_os = "android")))]
        stream.set_nonblocking(true)?;
        let address = SocketAddrUnix::new(socket)?;
        match rustix::net::connect(&stream, &address) {
            Ok(()) => {}
            Err(rustix::io::Errno::INPROGRESS) => {
                if !wait(&stream, PollFlags::OUT, deadline)? {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "daemon connect deadline expired",
                    ));
                }
                rustix::net::sockopt::socket_error(&stream)??;
            }
            Err(error) => return Err(error.into()),
        }
        stream.set_nonblocking(false)?;
        Ok(stream)
    };
    let stream =
        connect().map_err(|error| format!("connecting to {}: {error}", socket.display()))?;
    check_peer(&stream)?;
    Ok(stream)
}

/// Poll before framing so an idle timeout never consumes part of a frame.
pub(super) fn readable(stream: &UnixStream, deadline: Instant) -> Result<bool, String> {
    wait(stream, PollFlags::IN, deadline).map_err(|error| format!("waiting for daemon: {error}"))
}

fn wait(stream: &UnixStream, flags: PollFlags, deadline: Instant) -> io::Result<bool> {
    loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Ok(false);
        };
        let timeout = Timespec::try_from(remaining)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        match rustix::event::poll(&mut [PollFd::new(stream, flags)], Some(&timeout)) {
            Ok(count) => return Ok(count != 0),
            Err(rustix::io::Errno::INTR) => continue,
            Err(error) => return Err(error.into()),
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn check_peer(stream: &UnixStream) -> Result<(), String> {
    let peer = rustix::net::sockopt::socket_peercred(stream)
        .map_err(|error| format!("reading daemon peer credentials: {error}"))?;
    if peer.uid != rustix::process::getuid() {
        return Err("daemon peer uid does not match this user".into());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn check_peer(stream: &UnixStream) -> Result<(), String> {
    use std::os::fd::AsRawFd;
    // Rustix exposes SO_PEERCRED on Linux but has no getpeereid wrapper.
    unsafe extern "C" {
        fn getpeereid(socket: std::ffi::c_int, uid: *mut u32, gid: *mut u32) -> std::ffi::c_int;
    }
    let mut uid = 0;
    let mut gid = 0;
    // SAFETY: the descriptor is live and both outputs have macOS's uid_t/gid_t ABI.
    if unsafe { getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
        return Err(format!(
            "reading daemon peer credentials: {}",
            io::Error::last_os_error()
        ));
    }
    if uid != rustix::process::getuid().as_raw() {
        return Err("daemon peer uid does not match this user".into());
    }
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
fn check_peer(_stream: &UnixStream) -> Result<(), String> {
    Err("daemon peer credentials are unavailable on this platform".into())
}
