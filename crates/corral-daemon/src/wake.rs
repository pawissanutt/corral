//! Ends a connection's wait for client input when the daemon has something to push.
use std::{
    io::{self, Read, Write},
    os::{
        fd::{AsFd, BorrowedFd},
        unix::net::UnixStream,
    },
    time::Duration,
};

use rustix::event::{PollFd, PollFlags, Timespec, poll};

/// One connection's wake signal: written by whatever publishes for it, read by its loop.
pub(crate) struct Waker {
    sender: UnixStream,
    receiver: UnixStream,
}

impl Waker {
    pub(crate) fn new() -> io::Result<Self> {
        let (sender, receiver) = UnixStream::pair()?;
        sender.set_nonblocking(true)?;
        receiver.set_nonblocking(true)?;
        Ok(Self { sender, receiver })
    }

    /// A full socket already holds a pending wake, so a refused write loses nothing.
    pub(crate) fn wake(&self) {
        let _ = (&self.sender).write(&[0]);
    }

    /// Whether a wake was pending, consuming it.
    #[cfg(test)]
    pub(crate) fn take(&self) -> bool {
        let pending = readable(self.receiver.as_fd(), None, Duration::ZERO).unwrap_or(false);
        self.drain();
        pending
    }

    fn drain(&self) {
        let mut bytes = [0; 64];
        while matches!((&self.receiver).read(&mut bytes), Ok(read) if read == bytes.len()) {}
    }
}

/// Whether `stream` has input, waiting at most `timeout`. A wake ends the wait early
/// without input; it is consumed here, before the caller reads what was published, so a
/// publication after this returns wakes the next wait.
pub(crate) fn readable(
    stream: BorrowedFd<'_>,
    waker: Option<&Waker>,
    timeout: Duration,
) -> io::Result<bool> {
    let wake = waker.map_or(stream, |waker| waker.receiver.as_fd());
    let mut fds = [
        PollFd::new(&stream, PollFlags::IN),
        PollFd::new(&wake, PollFlags::IN),
    ];
    let watched = if waker.is_some() { 2 } else { 1 };
    let timeout = Timespec {
        tv_sec: timeout.as_secs().try_into().unwrap_or(i64::MAX),
        tv_nsec: timeout.subsec_nanos().into(),
    };
    poll(&mut fds[..watched], Some(&timeout))?;
    if let Some(waker) = waker
        && !fds[1].revents().is_empty()
    {
        waker.drain();
    }
    Ok(!fds[0].revents().is_empty())
}
