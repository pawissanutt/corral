//! The isolated driver can select one compiled-in experiment, never a product message.
use anyhow::{Context, Result, ensure};
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    io::Errno,
    net::{RecvAncillaryBuffer, RecvFlags, ReturnFlags, SendFlags, recvmsg, send},
};
use std::{
    cell::RefCell,
    io::IoSliceMut,
    os::fd::OwnedFd,
    time::{Duration, Instant},
};

pub(super) const FD: i32 = 9;
pub(super) const MANIFEST_PATH: &str = "provenance/corral-fault-v1.md";
pub(super) const CONTRACT: &[u8] =
    include_bytes!("../../../../docs/contracts/lite-analyst-fault-v1/README.md");
const CAP: Duration = Duration::from_secs(15);
const STEP: Duration = Duration::from_millis(25);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Case {
    StartCommitted,
    AckCommitted,
    FinishBefore,
    FinishAfter,
    SettleDuplicate,
    ForeignSession,
    BrokerAfterAdmission,
    SandboxDenied,
}

impl Case {
    const ALL: [Self; 8] = [
        Self::StartCommitted,
        Self::AckCommitted,
        Self::FinishBefore,
        Self::FinishAfter,
        Self::SettleDuplicate,
        Self::ForeignSession,
        Self::BrokerAfterAdmission,
        Self::SandboxDenied,
    ];

    pub(super) fn parse(name: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|case| case.packet().strip_suffix(b"\n") == Some(name.as_bytes()))
            .context("unknown Corral fixture fault case")
    }

    fn packet(self) -> &'static [u8] {
        match self {
            Self::StartCommitted => b"corral.start.committed\n",
            Self::AckCommitted => b"corral.ack.committed\n",
            Self::FinishBefore => b"corral.finish.before\n",
            Self::FinishAfter => b"corral.finish.after\n",
            Self::SettleDuplicate => b"corral.settle.duplicate\n",
            Self::ForeignSession => b"corral.session.foreign\n",
            Self::BrokerAfterAdmission => b"corral.broker.after_admission\n",
            Self::SandboxDenied => b"corral.sandbox.denied\n",
        }
    }

    fn action(self) -> &'static [u8] {
        match self {
            Self::StartCommitted | Self::AckCommitted | Self::BrokerAfterAdmission => b"drop\n",
            _ => b"continue\n",
        }
    }
}

pub(super) struct FaultControl {
    case: Case,
    fd: RefCell<Option<OwnedFd>>,
}

impl FaultControl {
    pub(super) fn new(case: Case, fd: OwnedFd) -> Self {
        Self {
            case,
            fd: RefCell::new(Some(fd)),
        }
    }

    pub(super) fn armed(&self, at: Case) -> bool {
        self.case == at && self.fd.borrow().is_some()
    }

    /// A selected barrier consumes its channel even on failure. The caller may poll
    /// real work/lifetime during the wait, but no product data travels on this fd.
    pub(super) fn reach(
        &self,
        at: Case,
        deadline: Instant,
        mut pump: impl FnMut() -> Result<()>,
    ) -> Result<bool> {
        if self.case != at {
            return Ok(false);
        }
        let Some(fd) = self.fd.borrow_mut().take() else {
            return Ok(false);
        };
        let deadline = deadline.min(Instant::now() + CAP);
        wait(&fd, PollFlags::OUT, deadline, &mut pump)?;
        ensure!(
            send(&fd, at.packet(), SendFlags::NOSIGNAL).context("send fixture barrier")?
                == at.packet().len(),
            "short fixture barrier send"
        );
        wait(&fd, PollFlags::IN, deadline, &mut pump)?;
        let mut bytes = [0; 10];
        let msg = recvmsg(
            &fd,
            &mut [IoSliceMut::new(&mut bytes)],
            &mut RecvAncillaryBuffer::default(),
            RecvFlags::CMSG_CLOEXEC,
        )
        .context("read fixture barrier action")?;
        ensure!(
            !msg.flags
                .intersects(ReturnFlags::CTRUNC | ReturnFlags::TRUNC)
                && bytes.get(..msg.bytes) == Some(at.action()),
            "invalid fixture barrier action"
        );
        pump()?;
        ensure!(
            Instant::now() < deadline,
            "fixture barrier deadline elapsed"
        );
        Ok(true)
    }
}

fn wait(
    fd: &OwnedFd,
    events: PollFlags,
    deadline: Instant,
    pump: &mut impl FnMut() -> Result<()>,
) -> Result<()> {
    let mut fds = [PollFd::new(fd, events)];
    loop {
        pump()?;
        let remaining = deadline.saturating_duration_since(Instant::now()).min(STEP);
        ensure!(!remaining.is_zero(), "fixture barrier deadline elapsed");
        let ts = Timespec {
            tv_sec: 0,
            tv_nsec: remaining.subsec_nanos().into(),
        };
        match poll(&mut fds, Some(&ts)) {
            Ok(0) | Err(Errno::INTR) => continue,
            Ok(_) => {
                let got = fds[0].revents();
                ensure!(
                    got.contains(events)
                        && !got.intersects(PollFlags::ERR | PollFlags::NVAL)
                        && (events == PollFlags::IN || !got.contains(PollFlags::HUP)),
                    "fixture barrier disconnected"
                );
                ensure!(
                    Instant::now() < deadline,
                    "fixture barrier deadline elapsed"
                );
                return Ok(());
            }
            Err(e) => return Err(e).context("poll fixture barrier"),
        }
    }
}

impl super::Session<'_> {
    pub(super) fn duplicate_settlement(
        &mut self,
        settlement: &super::turns::SettleTurn,
        deadline: Instant,
    ) -> Result<super::wire::Result<super::turns::SettlementReceipt>> {
        use super::wire::{self, Message};
        use serde_json::Value;

        let request = Message::Request {
            v: 1,
            id: wire::fresh_id()?,
            op: "settle_turn".into(),
            params: serde_json::to_value(settlement)?,
        };
        let original = match self.exchange::<Value>(&request, deadline)? {
            Ok(original) => original,
            Err(fault) => return Ok(Err(fault)),
        };
        let typed = wire::params(original.clone())
            .map_err(|_| anyhow::anyhow!("invalid fixture settlement receipt"))?;
        if self
            .work
            .barrier(Case::SettleDuplicate, deadline, || self.work.check_live())?
        {
            let replay = self.exchange::<Value>(&request, deadline)?;
            ensure!(
                replay == Ok(original),
                "fixture duplicate settlement receipt changed"
            );
        }
        Ok(Ok(typed))
    }

    pub(super) fn probe_foreign_session(&mut self, deadline: Instant) -> Result<()> {
        use super::wire::{self, Fault, Fence, FenceReason, Fenced};

        let deadline = deadline.min(Instant::now() + super::FRAME_TIMEOUT);
        if self
            .work
            .barrier(Case::ForeignSession, deadline, || self.work.check_live())?
        {
            let absent = wire::fresh_id()?;
            ensure!(
                matches!(self.store.read(&absent), Err(Fault::NotFound)),
                "foreign fixture ID is not absent"
            );
            let reply = self.call::<Fenced>(
                "fence_session",
                &Fence {
                    run_id: absent,
                    generation: 1,
                    reason: FenceReason::Stopped,
                },
                deadline,
            )?;
            ensure!(
                matches!(reply, Err(Fault::NotFound)),
                "foreign fixture session was not refused"
            );
        }
        Ok(())
    }

    pub(super) fn finish_barrier(&mut self, at: Case, deadline: Instant) -> Result<()> {
        let work = self.work;
        let deadline = deadline.min(Instant::now() + CAP);
        work.barrier(at, deadline, || {
            self.tick()?;
            // Stop may itself await a real work reply. Service it while paused,
            // then let the unchanged final admission gate choose the winner.
            if work.readable(Instant::now())? {
                let request_deadline = deadline.min(Instant::now() + super::FRAME_TIMEOUT);
                let handled = work
                    .receive_message(request_deadline)
                    .and_then(|request| self.handle(request, request_deadline));
                if handled.is_err() {
                    work.poison();
                }
                handled?;
            }
            Ok(())
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
