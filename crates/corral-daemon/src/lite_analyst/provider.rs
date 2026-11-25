//! One authenticated, nonblocking exchange with the fixed fixture provider.
mod http;
mod landlock;
mod sse;

use anyhow::{Context, Result, ensure};
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    io::Errno,
    net::{
        self, AddressFamily, RecvFlags, SendAncillaryBuffer, SendFlags, SocketFlags, SocketType,
        ipproto, sockopt,
    },
};
use serde_json::Value;
use std::{
    io::IoSlice,
    net::{Ipv4Addr, SocketAddrV4},
    os::fd::OwnedFd,
    time::{Duration, Instant},
};

use super::policy::MAX_BODY;
pub(super) use landlock::restrict_egress;

const PORT: u16 = 43173;
const PREFIX: &[u8] =
    b"POST /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1:43173\r\nAuthorization: Bearer ";
const HEADERS: &[u8] = b"\r\nContent-Type: application/json\r\nAccept: text/event-stream\r\nConnection: close\r\nContent-Length: ";
const END_HEADERS: &[u8] = b"\r\n\r\n";
const PUMP_INTERVAL: Duration = Duration::from_millis(10);

#[cfg(test)]
mod tests;

pub(super) struct Provider {
    socket: OwnedFd,
    // Taken exactly once, even on a failing first send syscall. Never returned or logged.
    credential: Option<[u8; 64]>,
    request_sent: bool,
    remainder: Vec<u8>,
}

impl Provider {
    /// Set up a single connected socket. No request data is sent or prepared here.
    pub(super) fn connect(
        credential: &[u8],
        deadline: Instant,
        pump: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Self> {
        ensure!(
            credential.len() == 64
                && credential
                    .iter()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b)),
            "invalid fixture provider credential"
        );
        let mut bearer = [0; 64];
        bearer.copy_from_slice(credential);
        check(deadline, pump)?;
        let socket = net::socket_with(
            AddressFamily::INET,
            SocketType::STREAM,
            SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
            Some(ipproto::TCP),
        )
        .context("open fixture provider socket")?;
        match net::connect(&socket, &SocketAddrV4::new(Ipv4Addr::LOCALHOST, PORT)) {
            Ok(()) => (),
            Err(Errno::INPROGRESS) => {
                wait(&socket, PollFlags::OUT, deadline, pump)?;
                sockopt::socket_error(&socket)
                    .context("check fixture provider connection")?
                    .context("connect fixture provider socket")?;
            }
            Err(error) => return Err(error).context("connect fixture provider socket"),
        }
        check(deadline, pump)?;
        Ok(Self {
            socket,
            credential: Some(bearer),
            request_sent: false,
            remainder: Vec::new(),
        })
    }

    /// Invoke immediately inside the C2 admission gate, AFTER durable consumption. `body`
    /// is the exact previously finalized and reserved bytes. No callback, allocation, or
    /// readiness wait precedes the one sendmsg attempt. Positive progress is handoff;
    /// even a failed first attempt leaves the already-consumed permit indeterminate.
    pub(super) fn send(&mut self, body: &[u8]) -> Result<()> {
        let bearer = self
            .credential
            .take()
            .context("fixture request already attempted")?;
        ensure!(
            !body.is_empty() && body.len() <= MAX_BODY,
            "invalid fixture provider body length"
        );
        // Content-Length decimal digits in place, with capacity for MAX_BODY (262144).
        let mut digits = [0; 6];
        let mut size = body.len();
        let mut start = digits.len();
        while size != 0 {
            start -= 1;
            digits[start] = b'0' + (size % 10) as u8;
            size /= 10;
        }
        let chunks = [
            PREFIX,
            &bearer,
            HEADERS,
            &digits[start..],
            END_HEADERS,
            body,
        ];
        let sent = net::sendmsg(
            &self.socket,
            &chunks.map(IoSlice::new),
            &mut SendAncillaryBuffer::default(),
            SendFlags::NOSIGNAL,
        )
        .context("fixture provider initial write")?;
        ensure!(sent > 0, "fixture provider initial write made no progress");
        self.request_sent = true;
        // Copy only the unwritten suffix, and only after entering the kernel once.
        let mut skip = sent;
        for chunk in chunks {
            if skip < chunk.len() {
                self.remainder.extend_from_slice(&chunk[skip..]);
                skip = 0;
            } else {
                skip -= chunk.len();
            }
        }
        Ok(())
    }

    /// Finish the same request on the same TCP stream, then validate the complete response.
    /// A callback error, timeout, EOF before valid completion, or bad framing is never a retry.
    pub(super) fn receive(
        self,
        deadline: Instant,
        pump: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Value> {
        ensure!(
            self.request_sent,
            "fixture provider has no handed-off request"
        );
        let mut rest = self.remainder.as_slice();
        while !rest.is_empty() {
            check(deadline, pump)?;
            match net::send(&self.socket, rest, SendFlags::NOSIGNAL) {
                Ok(n) if n > 0 => rest = &rest[n..],
                Ok(_) => anyhow::bail!("fixture provider write made no progress"),
                Err(Errno::INTR) => continue,
                Err(Errno::AGAIN) => wait(&self.socket, PollFlags::OUT, deadline, pump)?,
                Err(error) => return Err(error).context("finish fixture provider write"),
            }
        }

        let mut raw = Vec::new();
        let mut buf = [0; 8192];
        loop {
            check(deadline, pump)?;
            match net::recv(&self.socket, &mut buf[..], RecvFlags::empty()) {
                Ok((_, 0)) => break,
                Ok((_, n)) => {
                    ensure!(
                        raw.len() + n <= http::MAX_WIRE,
                        "fixture HTTP reply too large"
                    );
                    raw.extend_from_slice(&buf[..n]);
                }
                Err(Errno::INTR) => continue,
                Err(Errno::AGAIN) => wait(&self.socket, PollFlags::IN, deadline, pump)?,
                Err(error) => return Err(error).context("read fixture provider response"),
            }
        }
        let body = http::body(&raw)?;
        let response = sse::assistant(&body)?;
        check(deadline, pump)?;
        Ok(response)
    }
}

fn check(deadline: Instant, pump: &mut dyn FnMut() -> Result<()>) -> Result<()> {
    ensure!(
        Instant::now() < deadline,
        "fixture provider deadline expired"
    );
    pump()?;
    ensure!(
        Instant::now() < deadline,
        "fixture provider deadline expired"
    );
    Ok(())
}

fn wait(
    socket: &OwnedFd,
    events: PollFlags,
    deadline: Instant,
    pump: &mut dyn FnMut() -> Result<()>,
) -> Result<()> {
    let mut fds = [PollFd::new(socket, events)];
    loop {
        check(deadline, pump)?;
        let limit = deadline
            .saturating_duration_since(Instant::now())
            .min(PUMP_INTERVAL);
        let timeout = Timespec {
            tv_sec: 0,
            tv_nsec: limit.subsec_nanos().into(),
        };
        match poll(&mut fds, Some(&timeout)) {
            Ok(0) | Err(Errno::INTR) => continue,
            Ok(_) => {
                ensure!(
                    !fds[0].revents().contains(PollFlags::NVAL),
                    "invalid fixture provider socket"
                );
                // Let connect's SO_ERROR, or the next send/recv, interpret HUP or ERR.
                return Ok(());
            }
            Err(error) => return Err(error).context("poll fixture provider socket"),
        }
    }
}
