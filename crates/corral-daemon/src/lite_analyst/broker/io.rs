//! Nonblocking Unix transport for the closed broker adapter and shim. All waits
//! run the supervisor pump and check idle channels for EOF or unsolicited traffic.
use anyhow::{Context, Result, ensure};
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    fs,
    io::{Errno, FdFlags, fcntl_getfd},
    net::{
        self, AddressFamily, RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, ReturnFlags,
        SendAncillaryBuffer, SendAncillaryMessage, SendFlags, SocketFlags, SocketType, recvmsg,
        send, sendmsg, sockopt,
    },
};
use serde_json::Value;
use std::{
    io::{IoSlice, IoSliceMut},
    mem::MaybeUninit,
    os::fd::{BorrowedFd, OwnedFd},
    time::{Duration, Instant},
};

use super::super::wire::{MAX_FRAME, decode_signed};

const TICK: Duration = Duration::from_millis(20);
const IDLE: PollFlags = PollFlags::IN.union(PollFlags::RDHUP);
const MARKER: &[u8; 1] = b"B";
const PRIVATE_MAX: usize = 4096;

pub(super) fn socketpair() -> Result<(OwnedFd, OwnedFd)> {
    net::socketpair(
        AddressFamily::UNIX,
        SocketType::SEQPACKET,
        SocketFlags::CLOEXEC,
        None,
    )
    .context("create fixture broker socketpair")
}

pub(super) fn socket_identity(fd: BorrowedFd<'_>, kind: SocketType) -> Result<(u64, u64)> {
    ensure!(
        sockopt::socket_domain(fd)? == AddressFamily::UNIX && sockopt::socket_type(fd)? == kind,
        "wrong fixture broker socket type"
    );
    let local = net::getsockname(fd)?;
    let peer = net::getpeername(fd)?.context("fixture broker socket has no peer")?;
    for addr in [local, peer] {
        ensure!(
            addr.address_family() == AddressFamily::UNIX
                && addr.addr_len() as usize == std::mem::offset_of!(libc::sockaddr_un, sun_path),
            "fixture broker socket must be unnamed"
        );
    }
    let stat = fs::fstat(fd)?;
    Ok((stat.st_dev, stat.st_ino))
}

pub(super) fn quiet(fds: &[BorrowedFd<'_>]) -> Result<()> {
    let mut p: Vec<_> = fds
        .iter()
        .map(|&fd| PollFd::from_borrowed_fd(fd, IDLE))
        .collect();
    loop {
        match poll(&mut p, Some(&Timespec::default())) {
            Err(Errno::INTR) => continue,
            result => result.context("check fixture broker channels")?,
        };
        ensure!(
            p.iter().all(|fd| fd.revents().is_empty()),
            "fixture broker channel closed or unsolicited data"
        );
        return Ok(());
    }
}

pub(super) struct Scope<'a> {
    pub(super) deadline: Instant,
    pub(super) pump: &'a mut dyn FnMut() -> Result<()>,
}

impl Scope<'_> {
    fn step(&mut self) -> Result<()> {
        self.remaining()?;
        (self.pump)()?;
        self.remaining()?;
        Ok(())
    }

    fn remaining(&self) -> Result<Duration> {
        let t = self.deadline.saturating_duration_since(Instant::now());
        ensure!(!t.is_zero(), "fixture broker deadline expired");
        Ok(t)
    }

    fn wait(
        &mut self,
        fd: BorrowedFd<'_>,
        interest: PollFlags,
        idle: &[BorrowedFd<'_>],
    ) -> Result<()> {
        let mut p = Vec::with_capacity(idle.len() + 1);
        p.push(PollFd::from_borrowed_fd(fd, interest | IDLE));
        p.extend(idle.iter().map(|&fd| PollFd::from_borrowed_fd(fd, IDLE)));
        loop {
            self.step()?;
            match poll(&mut p, Some(&timespec(self.remaining()?.min(TICK)))) {
                Ok(0) | Err(Errno::INTR) => continue,
                result => result.context("wait for fixture broker channel")?,
            };
            ensure!(
                p[1..].iter().all(|fd| fd.revents().is_empty()),
                "fixture broker idle channel closed or unsolicited data"
            );
            let ready = p[0].revents();
            // When waiting to write, input is unsolicited. Never use a queued reply
            // to authorize a write or accept it as satisfying an unsent request.
            ensure!(
                (ready - interest).is_empty(),
                "fixture broker channel closed or unsolicited data"
            );
            if ready.contains(interest) {
                self.step()?;
                quiet(idle)?;
                if interest == PollFlags::OUT {
                    quiet(&[fd])?;
                }
                return Ok(());
            }
        }
    }

    pub(super) fn pause_until(&mut self, until: Instant, idle: &[BorrowedFd<'_>]) -> Result<()> {
        let mut p: Vec<_> = idle
            .iter()
            .map(|&fd| PollFd::from_borrowed_fd(fd, IDLE))
            .collect();
        loop {
            self.step()?;
            let remaining = until.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return quiet(idle);
            }
            match poll(
                &mut p,
                Some(&timespec(self.remaining()?.min(remaining).min(TICK))),
            ) {
                Ok(0) | Err(Errno::INTR) => continue,
                result => result.context("watch fixture broker while polling")?,
            };
            ensure!(
                p.iter().all(|fd| fd.revents().is_empty()),
                "fixture broker closed or unsolicited data between polls"
            );
        }
    }

    pub(super) fn send_packet(
        &mut self,
        fd: BorrowedFd<'_>,
        value: &Value,
        rights: &[BorrowedFd<'_>],
        idle: &[BorrowedFd<'_>],
    ) -> Result<()> {
        let bytes = encode(value, MAX_FRAME)?;
        self.send_record(fd, &bytes, rights, idle)
    }

    pub(super) fn send_private(
        &mut self,
        fd: BorrowedFd<'_>,
        value: &Value,
        idle: &[BorrowedFd<'_>],
    ) -> Result<()> {
        let bytes = encode(value, PRIVATE_MAX)?;
        self.send_record(fd, &bytes, &[], idle)
    }

    pub(super) fn send_handoff(
        &mut self,
        fd: BorrowedFd<'_>,
        rights: &[BorrowedFd<'_>; 2],
        idle: &[BorrowedFd<'_>],
    ) -> Result<()> {
        self.send_record(fd, MARKER, rights, idle)
    }

    fn send_record(
        &mut self,
        fd: BorrowedFd<'_>,
        bytes: &[u8],
        rights: &[BorrowedFd<'_>],
        idle: &[BorrowedFd<'_>],
    ) -> Result<()> {
        loop {
            self.wait(fd, PollFlags::OUT, idle)?;
            let mut area = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(2))];
            let mut control = SendAncillaryBuffer::new(&mut area);
            ensure!(
                rights.is_empty() || control.push(SendAncillaryMessage::ScmRights(rights)),
                "fixture broker descriptor send overflow"
            );
            match sendmsg(
                fd,
                &[IoSlice::new(bytes)],
                &mut control,
                SendFlags::NOSIGNAL | SendFlags::DONTWAIT,
            ) {
                Ok(n) => {
                    ensure!(n == bytes.len(), "short fixture broker record write");
                    return Ok(());
                }
                Err(Errno::INTR | Errno::AGAIN) => continue,
                Err(e) => return Err(e).context("write fixture broker record"),
            }
        }
    }

    pub(super) fn recv_handoff(&mut self, fd: BorrowedFd<'_>) -> Result<[OwnedFd; 2]> {
        loop {
            self.wait(fd, PollFlags::IN, &[])?;
            let mut data = [0; 2];
            let mut area = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(2))];
            let mut control = RecvAncillaryBuffer::new(&mut area);
            let msg = match recvmsg(
                fd,
                &mut [IoSliceMut::new(&mut data)],
                &mut control,
                RecvFlags::DONTWAIT | RecvFlags::CMSG_CLOEXEC,
            ) {
                Ok(msg) => msg,
                Err(Errno::INTR | Errno::AGAIN) => continue,
                Err(e) => return Err(e).context("receive fixture shim descriptor handoff"),
            };
            // The buffer and its drain own unconsumed rights, including on error.
            ensure!(
                msg.bytes == MARKER.len()
                    && data[..msg.bytes] == *MARKER
                    && !msg
                        .flags
                        .intersects(ReturnFlags::TRUNC | ReturnFlags::CTRUNC),
                "invalid fixture shim handoff marker"
            );
            let mut ancillary = control.drain();
            let Some(RecvAncillaryMessage::ScmRights(rights)) = ancillary.next() else {
                anyhow::bail!("missing fixture shim descriptors");
            };
            let rights: Vec<_> = rights.collect();
            ensure!(
                ancillary.next().is_none(),
                "extra fixture shim ancillary message"
            );
            let rights: [OwnedFd; 2] = rights
                .try_into()
                .map_err(|_| anyhow::anyhow!("wrong fixture shim descriptor count"))?;
            for right in &rights {
                ensure!(
                    fcntl_getfd(right)?.contains(FdFlags::CLOEXEC),
                    "inheritable fixture shim descriptor"
                );
            }
            return Ok(rights);
        }
    }

    pub(super) fn recv_packet(
        &mut self,
        fd: BorrowedFd<'_>,
        idle: &[BorrowedFd<'_>],
    ) -> Result<Value> {
        let mut bytes = vec![0; MAX_FRAME];
        let n = self.recv(fd, &mut bytes, idle)?;
        decode_signed(&bytes[..n])
    }

    pub(super) fn recv_private(
        &mut self,
        fd: BorrowedFd<'_>,
        idle: &[BorrowedFd<'_>],
    ) -> Result<Value> {
        let mut bytes = [0; PRIVATE_MAX];
        let n = self.recv(fd, &mut bytes, idle)?;
        decode_signed(&bytes[..n])
    }

    fn recv(
        &mut self,
        fd: BorrowedFd<'_>,
        bytes: &mut [u8],
        idle: &[BorrowedFd<'_>],
    ) -> Result<usize> {
        loop {
            self.wait(fd, PollFlags::IN, idle)?;
            // Unclaimed SCM_RIGHTS are closed by Linux. CTRUNC also rejects the data.
            match recvmsg(
                fd,
                &mut [IoSliceMut::new(bytes)],
                &mut RecvAncillaryBuffer::default(),
                RecvFlags::DONTWAIT | RecvFlags::CMSG_CLOEXEC,
            ) {
                Ok(msg) => {
                    ensure!(
                        msg.bytes != 0
                            && !msg
                                .flags
                                .intersects(ReturnFlags::TRUNC | ReturnFlags::CTRUNC),
                        "fixture broker EOF, truncated record or ancillary data"
                    );
                    return Ok(msg.bytes);
                }
                Err(Errno::INTR | Errno::AGAIN) => continue,
                Err(e) => return Err(e).context("read fixture broker record"),
            }
        }
    }

    pub(super) fn send_line(
        &mut self,
        fd: BorrowedFd<'_>,
        value: &Value,
        idle: &[BorrowedFd<'_>],
    ) -> Result<()> {
        let mut bytes = encode(value, MAX_FRAME)?;
        bytes.push(b'\n');
        let mut rest = &bytes[..];
        while !rest.is_empty() {
            self.wait(fd, PollFlags::OUT, idle)?;
            match send(fd, rest, SendFlags::NOSIGNAL | SendFlags::DONTWAIT) {
                Ok(n) => {
                    ensure!(n != 0, "fixture shim stream write ended");
                    rest = &rest[n..];
                }
                Err(Errno::INTR | Errno::AGAIN) => continue,
                Err(e) => return Err(e).context("write fixture shim stream"),
            }
        }
        Ok(())
    }
}

// Only the startup notification and its following list request may be legal
// consecutive lines without an intervening reply. Callers check `unbuffered` at
// all other boundaries, so a second coalesced reply/call isn't silently dropped.
#[derive(Default)]
pub(super) struct Lines(Vec<u8>);

impl Lines {
    pub(super) fn read(
        &mut self,
        scope: &mut Scope<'_>,
        fd: BorrowedFd<'_>,
        idle: &[BorrowedFd<'_>],
    ) -> Result<Value> {
        let mut bytes = std::mem::take(&mut self.0);
        let mut searched = 0;
        loop {
            scope.step()?;
            quiet(idle)?;
            if let Some(end) = bytes[searched..].iter().position(|&b| b == b'\n') {
                let end = searched + end;
                ensure!(
                    (1..=MAX_FRAME).contains(&end) && !bytes[..end].contains(&b'\r'),
                    "invalid fixture shim LF record"
                );
                self.0 = bytes.split_off(end + 1);
                return decode_signed(&bytes[..end]);
            }
            ensure!(bytes.len() <= MAX_FRAME, "fixture shim line too large");
            searched = bytes.len();
            let mut chunk = [0; 4096];
            let n = scope.recv(fd, &mut chunk, idle)?;
            bytes.extend_from_slice(&chunk[..n]);
        }
    }

    pub(super) fn unbuffered(&self) -> Result<()> {
        ensure!(self.0.is_empty(), "unsolicited fixture shim line");
        Ok(())
    }
}

fn encode(value: &Value, limit: usize) -> Result<Vec<u8>> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(
        (1..=limit).contains(&bytes.len()),
        "fixture broker record too large"
    );
    Ok(bytes)
}

fn timespec(duration: Duration) -> Timespec {
    Timespec {
        tv_sec: duration.as_secs() as _,
        tv_nsec: duration.subsec_nanos().into(),
    }
}
