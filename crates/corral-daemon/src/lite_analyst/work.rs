//! Bounded, full-duplex work channel on the exclusively inherited Unix capability.
use super::wire::{self, MAX_FRAME, Message};
use anyhow::{Context, ensure};
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    io::Errno,
    net::{RecvAncillaryBuffer, RecvFlags, ReturnFlags, SendFlags, recvmsg, send},
};
use serde::Serialize;
use std::{
    io::IoSliceMut,
    os::{fd::OwnedFd, unix::net::UnixStream},
    time::{Duration, Instant},
};

pub(super) const FRAME_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub(super) struct LifetimeEnded;
impl std::fmt::Display for LifetimeEnded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("fixture lifetime ended")
    }
}
impl std::error::Error for LifetimeEnded {}

pub(super) struct Work {
    socket: UnixStream,
    lifetime: OwnedFd,
    broker: Option<OwnedFd>,
}

impl Work {
    pub(super) fn new(socket: UnixStream, lifetime: OwnedFd) -> anyhow::Result<Self> {
        socket
            .set_nonblocking(true)
            .context("configure fixture work socket")?;
        Ok(Self {
            socket,
            lifetime,
            broker: None,
        })
    }

    pub(super) fn with_broker(mut self, broker: OwnedFd) -> Self {
        self.broker = Some(broker);
        self
    }

    pub(super) fn check_live(&self) -> anyhow::Result<()> {
        self.poll(PollFlags::RDHUP, Instant::now()).map(|_| ())
    }

    fn poll(&self, interest: PollFlags, deadline: Instant) -> anyhow::Result<PollFlags> {
        loop {
            let mut fds = [
                PollFd::new(&self.lifetime, PollFlags::IN),
                PollFd::new(&self.socket, interest),
                PollFd::new(
                    self.broker.as_ref().unwrap_or(&self.lifetime),
                    PollFlags::IN | PollFlags::RDHUP,
                ),
            ];
            let len = if self.broker.is_some() { 3 } else { 2 };
            let remaining = deadline.saturating_duration_since(Instant::now());
            let timeout = Timespec {
                tv_sec: remaining.as_secs() as _,
                tv_nsec: remaining.subsec_nanos().into(),
            };
            match poll(&mut fds[..len], Some(&timeout)) {
                Ok(_) => {
                    if fds[0].revents() == PollFlags::HUP {
                        return Err(LifetimeEnded.into());
                    }
                    ensure!(
                        fds[0].revents().is_empty(),
                        "invalid fixture lifetime event"
                    );
                    ensure!(
                        fds[2].revents().is_empty(),
                        "fixture broker closed or unexpected data"
                    );
                    let flags = fds[1].revents();
                    ensure!(
                        !flags.intersects(
                            PollFlags::ERR | PollFlags::NVAL | PollFlags::HUP | PollFlags::RDHUP
                        ),
                        "fixture work disconnected"
                    );
                    return Ok(flags);
                }
                Err(Errno::INTR) if Instant::now() < deadline => continue,
                Err(e) => return Err(e).context("wait for fixture work"),
            }
        }
    }

    pub(super) fn readable(&self, deadline: Instant) -> anyhow::Result<bool> {
        Ok(self
            .poll(PollFlags::IN | PollFlags::RDHUP, deadline)?
            .contains(PollFlags::IN))
    }

    pub(super) fn receive(&self, deadline: Instant) -> anyhow::Result<Vec<u8>> {
        let mut header = [0; 4];
        self.read_exact(&mut header, deadline)?;
        let n = u32::from_be_bytes(header) as usize;
        ensure!((1..=MAX_FRAME).contains(&n), "invalid fixture frame length");
        let mut out = vec![0; n];
        self.read_exact(&mut out, deadline)?;
        Ok(out)
    }

    fn read_exact(&self, mut bytes: &mut [u8], deadline: Instant) -> anyhow::Result<()> {
        while !bytes.is_empty() {
            ensure!(
                Instant::now() < deadline && self.readable(deadline)? && Instant::now() < deadline,
                "fixture read timeout"
            );
            // A zero-capacity ancillary buffer makes Linux close unclaimed rights; CTRUNC
            // also rejects the associated frame, even when rights arrive on its first byte.
            match recvmsg(
                &self.socket,
                &mut [IoSliceMut::new(bytes)],
                &mut RecvAncillaryBuffer::default(),
                RecvFlags::CMSG_CLOEXEC,
            ) {
                Ok(msg) => {
                    ensure!(
                        msg.bytes > 0
                            && !msg
                                .flags
                                .intersects(ReturnFlags::CTRUNC | ReturnFlags::TRUNC),
                        "fixture work EOF or ancillary data"
                    );
                    bytes = &mut bytes[msg.bytes..];
                }
                Err(Errno::INTR | Errno::AGAIN) => continue,
                Err(e) => return Err(e).context("read fixture work"),
            }
        }
        Ok(())
    }

    pub(super) fn send(&self, value: &impl Serialize, deadline: Instant) -> anyhow::Result<()> {
        let payload = serde_json::to_vec(value).context("encode fixture frame")?;
        ensure!(
            (1..=MAX_FRAME).contains(&payload.len()),
            "invalid outgoing fixture frame length"
        );
        self.write_all(&(payload.len() as u32).to_be_bytes(), deadline)?;
        self.write_all(&payload, deadline)
    }

    fn write_all(&self, mut bytes: &[u8], deadline: Instant) -> anyhow::Result<()> {
        while !bytes.is_empty() {
            ensure!(
                Instant::now() < deadline
                    && self
                        .poll(PollFlags::OUT | PollFlags::RDHUP, deadline)?
                        .contains(PollFlags::OUT)
                    && Instant::now() < deadline,
                "fixture write timeout"
            );
            match send(&self.socket, bytes, SendFlags::NOSIGNAL) {
                Ok(n) => {
                    ensure!(n > 0, "fixture write ended");
                    bytes = &bytes[n..];
                }
                Err(Errno::INTR | Errno::AGAIN) => continue,
                Err(e) => return Err(e).context("write fixture work"),
            }
        }
        Ok(())
    }

    pub(super) fn receive_message(&self, deadline: Instant) -> anyhow::Result<Message> {
        Message::decode(&self.receive(deadline)?)
    }

    pub(super) fn respond(
        &self,
        id: String,
        result: wire::Result<serde_json::Value>,
        deadline: Instant,
    ) -> anyhow::Result<()> {
        let msg = match result {
            Ok(result) => Message::Success { v: 1, id, result },
            Err(error) => Message::Failure { v: 1, id, error },
        };
        self.send(&msg, deadline)
    }
}
