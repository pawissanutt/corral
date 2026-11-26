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

    /// Takes the pending wake. The connection's loop does this just before it reads what was
    /// published, so a publication after it wakes the next wait and none is lost. No wait
    /// takes it: a wait that also found input would otherwise hide the wake from the loop.
    pub(crate) fn clear(&self) {
        let mut bytes = [0; 64];
        while matches!((&self.receiver).read(&mut bytes), Ok(read) if read == bytes.len()) {}
    }

    /// Whether a wake was pending, taking it.
    #[cfg(test)]
    pub(crate) fn take(&self) -> bool {
        let woken =
            wait(self.receiver.as_fd(), Some(self), Duration::ZERO).is_ok_and(|ready| ready.woken);
        self.clear();
        woken
    }
}

/// What ended a wait.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Ready {
    /// The stream has input, or closed.
    pub(crate) input: bool,
    /// A wake is pending: it stays so until the loop clears it.
    pub(crate) woken: bool,
}

impl Ready {
    /// Whether to read the input now. A pending wake goes first: the input may be only part
    /// of a frame, and the page the loop was woken for would wait for the rest of it.
    pub(crate) fn read(self) -> bool {
        self.input && !self.woken
    }
}

/// Waits at most `timeout` for input on `stream` or a wake of `waker`.
pub(crate) fn wait(
    stream: BorrowedFd<'_>,
    waker: Option<&Waker>,
    timeout: Duration,
) -> io::Result<Ready> {
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
    Ok(Ready {
        input: !fds[0].revents().is_empty(),
        woken: waker.is_some() && !fds[1].revents().is_empty(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Input that arrived with a wake was read first. When it was the start of a frame, the
    /// page the connection was woken for waited for the rest of it.
    #[test]
    fn a_wake_goes_before_input_that_is_ready_with_it() {
        let waker = Waker::new().unwrap();
        let (stream, mut peer) = UnixStream::pair().unwrap();
        peer.write_all(b"start of a frame").unwrap();
        waker.wake();
        let ready = || wait(stream.as_fd(), Some(&waker), Duration::ZERO).unwrap();
        assert!(!ready().read());
        waker.clear();
        assert!(ready().read(), "the input went with the wake");
    }
}
