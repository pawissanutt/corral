//! Trusted, session-scoped adapter for the two Lite broker capabilities.
//! The shim is a separate payload; only it retains the evidence endpoint.
#[path = "broker/io.rs"]
mod io;
#[path = "broker/protocol.rs"]
mod protocol;
#[path = "broker/shim.rs"]
mod shim;

use anyhow::{Context, Result, ensure};
use rustix::net::SocketType;
use serde_json::{Value, json};
use std::{
    os::{
        fd::{AsFd, BorrowedFd, OwnedFd},
        unix::net::UnixStream,
    },
    time::{Duration, Instant},
};

use super::{bootstrap::Enrollment, policy, wire};
use io::{Lines, Scope};
use protocol::{Arm, BindingKind, BindingRecord, Call, Probe};

const MAX_CALLS: u64 = 8;
const MAX_REPLY: u64 = 8192;
const MAX_TOTAL_REPLY: u64 = 32768;
const MAX_POLLS: usize = 2;
const MIN_POLL_INTERVAL: Duration = Duration::from_millis(1000);
// Strictly below the historical 15s client timeout; a failed call is never retried.
const CALL_TIMEOUT: Duration = Duration::from_secs(9);
const PROBE_TIMEOUT: Duration = Duration::from_millis(200);

pub(super) fn shim_main() -> Result<()> {
    shim::run_inherited()
}

pub(super) struct Broker {
    binding: BindingRecord,
    pipes: Option<Pipes>,
    next_id: u64,
    budget: Option<Budget>,
}

struct Pipes {
    stdio: UnixStream,
    control: OwnedFd,
    host_poll: OwnedFd,
    lines: Lines,
}

impl Pipes {
    fn idle(&self) -> [BorrowedFd<'_>; 3] {
        [
            self.stdio.as_fd(),
            self.control.as_fd(),
            self.host_poll.as_fd(),
        ]
    }
}

struct Budget {
    calls_left: u64,
    per_reply: u64,
    total_left: u64,
}

impl Budget {
    fn admit_call(&mut self) -> Result<()> {
        self.calls_left = self
            .calls_left
            .checked_sub(1)
            .context("fixture broker call limit exhausted")?;
        Ok(())
    }

    fn record_reply(&mut self, payload: &Value) -> Result<()> {
        let bytes = wire::canonical_signed(payload)?.len() as u64;
        ensure!(
            bytes <= self.per_reply && bytes <= self.total_left,
            "fixture broker reply exceeds byte limit"
        );
        self.total_left -= bytes;
        Ok(())
    }
}

impl Broker {
    pub(super) fn attach(
        bootstrap: &OwnedFd,
        enrollment: &Enrollment,
        binding: &wire::Binding,
        stdio: UnixStream,
        deadline: Instant,
        pump: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Self> {
        let mut scope = Scope { deadline, pump };
        let binding = BindingRecord::from_bound(enrollment, binding)?;
        io::socket_identity(bootstrap.as_fd(), SocketType::SEQPACKET)?;
        io::socket_identity(stdio.as_fd(), SocketType::STREAM)?;
        let (evidence, lite_evidence) = io::socketpair()?;
        let (host_poll, lite_poll) = io::socketpair()?;
        let (control, shim_control) = io::socketpair()?;

        scope.send_packet(
            bootstrap.as_fd(),
            &binding.record(BindingKind::BrokerAttach)?,
            &[lite_evidence.as_fd(), lite_poll.as_fd()],
            &[stdio.as_fd(), evidence.as_fd(), host_poll.as_fd()],
        )?;
        drop((lite_evidence, lite_poll));
        let ack = scope.recv_packet(
            bootstrap.as_fd(),
            &[stdio.as_fd(), evidence.as_fd(), host_poll.as_fd()],
        )?;
        ensure!(
            ack == binding.record(BindingKind::BrokerAttached)?,
            "fixture broker attach acknowledgement mismatch"
        );

        // Neither route can dispatch before both descriptor lists have been pinned.
        negotiate_packet(
            &mut scope,
            evidence.as_fd(),
            false,
            &[bootstrap.as_fd(), stdio.as_fd(), host_poll.as_fd()],
        )?;
        negotiate_packet(
            &mut scope,
            host_poll.as_fd(),
            true,
            &[bootstrap.as_fd(), stdio.as_fd(), evidence.as_fd()],
        )?;
        scope.send_handoff(
            stdio.as_fd(),
            &[evidence.as_fd(), shim_control.as_fd()],
            &[bootstrap.as_fd(), host_poll.as_fd(), evidence.as_fd()],
        )?;
        drop((evidence, shim_control));
        let idle = [bootstrap.as_fd(), stdio.as_fd(), host_poll.as_fd()];
        scope.send_private(
            control.as_fd(),
            &binding.record(BindingKind::ShimBind)?,
            &idle,
        )?;
        let ack = scope.recv_private(control.as_fd(), &idle)?;
        ensure!(
            ack == binding.record(BindingKind::ShimBound)?,
            "fixture shim binding acknowledgement mismatch"
        );

        let mut lines = Lines::default();
        negotiate_shim(
            &mut scope,
            stdio.as_fd(),
            &mut lines,
            &[bootstrap.as_fd(), host_poll.as_fd(), control.as_fd()],
        )?;
        let pipes = Pipes {
            stdio,
            control,
            host_poll,
            lines,
        };
        io::quiet(&pipes.idle())?;
        Ok(Self {
            binding,
            pipes: Some(pipes),
            next_id: 3,
            budget: None,
        })
    }

    pub(super) fn set_limits(&mut self, limits: &wire::Limits) -> Result<()> {
        ensure!(
            self.budget.is_none()
                && self.pipes.is_some()
                && (1..=MAX_CALLS).contains(&limits.calls)
                && (1..=MAX_REPLY).contains(&limits.response_bytes)
                && (1..=MAX_TOTAL_REPLY).contains(&limits.total_response_bytes),
            "invalid, unavailable or already installed fixture broker limits"
        );
        self.budget = Some(Budget {
            calls_left: limits.calls,
            per_reply: limits.response_bytes,
            total_left: limits.total_response_bytes,
        });
        Ok(())
    }

    /// Nonblocking observation for an idle broker. The shim continuously
    /// watches evidence and closes its retained channels when it detects loss.
    pub(super) fn check_live(&self) -> Result<()> {
        let pipes = self.pipes.as_ref().context("fixture broker is closed")?;
        pipes.lines.unbuffered()?;
        io::quiet(&pipes.idle())
    }

    /// Bounded round trip for a synchronous observation of shim-owned evidence.
    /// It is not a grant/permit check or a claim about later revocation.
    pub(super) fn prove_live(
        &self,
        deadline: Instant,
        pump: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        self.check_live()?;
        let pipes = self.pipes.as_ref().context("fixture broker is closed")?;
        let probe = Probe::new()?;
        let idle = [pipes.stdio.as_fd(), pipes.host_poll.as_fd()];
        let mut scope = Scope {
            deadline: deadline.min(Instant::now() + PROBE_TIMEOUT),
            pump,
        };
        scope.send_private(pipes.control.as_fd(), &serde_json::to_value(&probe)?, &idle)?;
        ensure!(
            scope.recv_private(pipes.control.as_fd(), &idle)? == probe.ack()?,
            "fixture shim evidence observation mismatch"
        );
        io::quiet(&pipes.idle())
    }

    pub(super) fn invoke(
        &mut self,
        tool: &str,
        args: &Value,
        deadline: Instant,
        pump: &mut dyn FnMut() -> Result<()>,
    ) -> Result<Value> {
        // Errors (including timeout, failed pump, or protocol mismatch) close all
        // capabilities. An uncertain send is not permission to try the call again.
        let mut pipes = self.pipes.take().context("fixture broker is closed")?;
        let mut scope = Scope {
            deadline: deadline.min(Instant::now() + CALL_TIMEOUT),
            pump,
        };
        let reply = self.invoke_inner(&mut pipes, tool, args, &mut scope)?;
        pipes.lines.unbuffered()?;
        io::quiet(&pipes.idle())?;
        self.pipes = Some(pipes);
        Ok(reply)
    }

    fn invoke_inner(
        &mut self,
        pipes: &mut Pipes,
        tool: &str,
        args: &Value,
        scope: &mut Scope<'_>,
    ) -> Result<Value> {
        io::quiet(&pipes.idle())?;
        let call = Call::new(self.allocate_id(), tool, args, false)?;
        let arm = Arm::new(&call)?;
        let budget = self
            .budget
            .as_mut()
            .context("fixture broker limits not installed")?;
        budget.admit_call()?;
        let idle = [pipes.stdio.as_fd(), pipes.host_poll.as_fd()];
        scope.send_private(pipes.control.as_fd(), &serde_json::to_value(&arm)?, &idle)?;
        let ack = scope.recv_private(pipes.control.as_fd(), &idle)?;
        ensure!(
            ack == arm.ack()?,
            "fixture shim arm acknowledgement mismatch"
        );

        let idle = [pipes.control.as_fd(), pipes.host_poll.as_fd()];
        scope.send_line(pipes.stdio.as_fd(), &serde_json::to_value(&call)?, &idle)?;
        let reply = pipes.lines.read(scope, pipes.stdio.as_fd(), &idle)?;
        pipes.lines.unbuffered()?;
        let payload = protocol::structured_reply(&call, reply)?;
        budget.record_reply(&payload)?;
        if tool != "calculate_metric" || payload["status"] != "pending" {
            return Ok(payload);
        }

        // Only a validated pending reply to this immutable, outstanding call
        // creates a polling opportunity. Keep that handle local and never create
        // a provider-visible poll call or return a nonterminal reply as completed.
        let handle = payload["result"]
            .as_str()
            .context("missing fixture pending handle")?;
        let args = json!({"result":handle});
        for _ in 0..MAX_POLLS {
            let id = self.allocate_id();
            let budget = self
                .budget
                .as_mut()
                .context("fixture broker limits not installed")?;
            budget.admit_call()?;
            scope.pause_until(Instant::now() + MIN_POLL_INTERVAL, &pipes.idle())?;
            let call = Call::new(id, "read_result", &args, true)?;
            let poll = Arm::new(&call)?;
            let idle = [pipes.stdio.as_fd(), pipes.control.as_fd()];
            scope.send_packet(
                pipes.host_poll.as_fd(),
                &poll.request(&self.binding, &call)?,
                &[],
                &idle,
            )?;
            let reply = scope.recv_packet(pipes.host_poll.as_fd(), &idle)?;
            let (_, payload) = poll.reply(&self.binding, &call, reply)?;
            budget.record_reply(&payload)?;
            if payload["status"] != "pending" {
                policy::validate_reply(tool, &payload)?;
                return Ok(payload);
            }
        }
        anyhow::bail!("fixture metric has no terminal result within authorized polls");
    }

    fn allocate_id(&mut self) -> u64 {
        // At most eight admitted calls in this binding. No correlation ID is reused.
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

fn negotiate_packet(
    scope: &mut Scope<'_>,
    fd: BorrowedFd<'_>,
    host_poll: bool,
    idle: &[BorrowedFd<'_>],
) -> Result<()> {
    scope.send_packet(fd, &protocol::initialize(), &[], idle)?;
    ensure!(
        scope.recv_packet(fd, idle)? == protocol::server_info(protocol::LITE_SERVER),
        "fixture Lite MCP initialization mismatch"
    );
    scope.send_packet(fd, &protocol::initialized(), &[], idle)?;
    scope.send_packet(fd, &protocol::list(), &[], idle)?;
    protocol::check_listing(scope.recv_packet(fd, idle)?, host_poll)
}

fn negotiate_shim(
    scope: &mut Scope<'_>,
    fd: BorrowedFd<'_>,
    lines: &mut Lines,
    idle: &[BorrowedFd<'_>],
) -> Result<()> {
    scope.send_line(fd, &protocol::initialize(), idle)?;
    ensure!(
        lines.read(scope, fd, idle)? == protocol::server_info(protocol::SHIM_SERVER),
        "fixture shim MCP initialization mismatch"
    );
    lines.unbuffered()?;
    scope.send_line(fd, &protocol::initialized(), idle)?;
    scope.send_line(fd, &protocol::list(), idle)?;
    protocol::check_listing(lines.read(scope, fd, idle)?, false)?;
    lines.unbuffered()
}

#[cfg(test)]
#[path = "broker/tests.rs"]
mod tests;
