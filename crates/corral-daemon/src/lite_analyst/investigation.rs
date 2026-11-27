//! The one-shot fixture investigation. The outer session remains the only
//! lifecycle/permit decision maker, even while waiting for either child.
use super::{
    Fixture, SendAdmission, Session,
    broker::Broker,
    isolation::{Payload, Sandbox},
    policy,
    provider::Provider,
    reports::{Finish, Sealed},
    turns::{Outcome, ReserveTurn},
    wire::{self, Binding, FenceReason, StartBody},
    work::FRAME_TIMEOUT,
};
#[cfg(feature = "lite-analyst-faults")]
use super::{faults::Case, isolation::probe_supervisor_denial};
use anyhow::{Context, Result, ensure};
use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    io::Errno,
    net::{RecvAncillaryBuffer, RecvFlags, ReturnFlags, SendFlags, recvmsg, send},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashSet,
    io::IoSliceMut,
    os::unix::net::UnixStream,
    time::{Duration, Instant},
};

const IPC_MAX: usize = 524_288;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const STOP_GRACE: Duration = Duration::from_secs(5);
const STEP: Duration = Duration::from_millis(25);

#[derive(Default)]
struct Children {
    shim: Option<Sandbox>,
    pi: Option<Sandbox>,
    broker: Option<Broker>,
    io: Option<PiIo>,
}

impl Children {
    fn stop(mut self) -> Result<()> {
        // Close authority-bearing streams before waiting on wrappers. Reap
        // both children even when one wrapper's teardown is uncertain.
        drop(self.broker.take());
        drop(self.io.take());
        let deadline = Instant::now() + STOP_GRACE;
        let pi = self
            .pi
            .as_mut()
            .map(|child| child.reap(deadline))
            .transpose();
        let shim = self
            .shim
            .as_mut()
            .map(|child| child.reap(deadline))
            .transpose();
        pi.and(shim).map(|_| ())
    }
}

impl Session<'_> {
    pub(super) fn investigate(
        &mut self,
        fixture: Fixture<'_>,
        binding: Binding,
        body: StartBody,
    ) -> Result<()> {
        let before_submit = self.pump_run(&binding, "admitted");
        if before_submit.is_err() {
            // A cancelling request can win just before launch. Do not take the
            // enrolled link offline for a normal cancel.
            self.work.check_live()?;
            ensure!(
                self.store
                    .read(&binding.run_id)
                    .is_ok_and(|s| s.state != "admitted"),
                "fixture launch admission unavailable"
            );
            return Ok(());
        }
        let now_ms = self.clock.now()?;
        let remaining = body.deadline_ms.saturating_sub(now_ms);
        if remaining == 0 {
            return self.stop_local(&binding.run_id, binding.generation, FenceReason::Expired);
        }
        let deadline = Instant::now() + Duration::from_millis(remaining);
        // C2 persists the single retained launch identity before any spawn.
        let launch_id = self
            .store
            .submit_launch(&binding.run_id, binding.generation, now_ms)
            .map_err(|_| anyhow::anyhow!("fixture launch submission refused"))?;
        let mut children = Children::default();
        let _ = self.converse(
            fixture,
            &binding,
            &body,
            &launch_id,
            deadline,
            &mut children,
        );
        let before_stop = self.store.read(&binding.run_id);
        let initiated =
            before_stop.is_ok_and(|s| matches!(s.state.as_str(), "admitted" | "working"));
        let reason = if Instant::now() >= deadline {
            FenceReason::Expired
        } else {
            FenceReason::Stopped
        };
        let fence = self.stop_local(&binding.run_id, binding.generation, reason);
        let reaped = children.stop();
        fence?;
        reaped?;
        // A committed report survives cancellation, peer loss and even failure
        // to hand its local success receipt back to Pi. Teardown releases only
        // the process slot; it cannot retract a terminal winner.
        self.store
            .confirm_teardown(
                &binding.run_id,
                binding.generation,
                &launch_id,
                initiated.then_some("unavailable"),
            )
            .map_err(|_| anyhow::anyhow!("fixture teardown could not commit"))?;
        self.work.check_live()?;
        self.flush_local_fence()
    }

    fn converse(
        &mut self,
        fixture: Fixture<'_>,
        binding: &Binding,
        body: &StartBody,
        launch_id: &str,
        deadline: Instant,
        children: &mut Children,
    ) -> Result<()> {
        let startup = deadline.min(Instant::now() + STARTUP_TIMEOUT);
        self.pump_run(binding, "admitted")?;
        let (shim, stdio) = Sandbox::spawn(Payload::Shim)?;
        children.shim = Some(shim);
        children
            .shim
            .as_mut()
            .context("fixture shim missing")?
            .confirm_identity(startup, &mut || self.pump_run(binding, "admitted"))?;
        let work = self.work;
        children.broker = Some(work.during_broker_attach(|| {
            Broker::attach(
                fixture.resources.broker(),
                fixture.enrollment,
                binding,
                stdio,
                startup,
                &mut || self.pump_run(binding, "admitted"),
            )
        })?);
        let broker = children.broker.as_mut().context("fixture broker missing")?;
        self.pump_run(binding, "admitted")?;
        broker.check_live()?;
        #[cfg(feature = "lite-analyst-faults")]
        let sandbox_probe = work.fault_armed(Case::SandboxDenied);
        #[cfg(feature = "lite-analyst-faults")]
        let payload = if sandbox_probe {
            Payload::PiProbe
        } else {
            Payload::Pi
        };
        #[cfg(not(feature = "lite-analyst-faults"))]
        let payload = Payload::Pi;
        let (pi, io) = Sandbox::spawn(payload)?;
        children.pi = Some(pi);
        children.io = Some(PiIo {
            io,
            buffer: Vec::new(),
        });
        children
            .pi
            .as_mut()
            .context("fixture Pi wrapper missing")?
            .confirm_identity(startup, &mut || {
                self.pump_run(binding, "admitted")?;
                broker.check_live()
            })?;
        let io = children.io.as_mut().context("fixture Pi missing")?;
        #[cfg(feature = "lite-analyst-faults")]
        if sandbox_probe {
            probe_supervisor_denial()?;
            ensure!(
                matches!(
                    io.receive(startup, &mut || {
                        self.pump_run_before(binding, "admitted", Some(startup))?;
                        broker.check_live()
                    })?,
                    PiFrame::SandboxDenied
                ) && io.buffer.is_empty(),
                "fixture Pi denial was not observed"
            );
            work.barrier(Case::SandboxDenied, startup, |cutoff| {
                self.pump_run_before(binding, "admitted", Some(cutoff))?;
                broker.check_live()
            })?;
            anyhow::bail!("fixture sandbox denial probe ended");
        }
        let mut starting = || self.pump_run(binding, "admitted");
        io.send(
            &json!({"type":"start", "question":body.question, "prompt":policy::prompt()?,
            "tools":policy::model_descriptors()?}),
            startup,
            &mut starting,
        )?;
        io.negotiate_shim(broker, startup, &mut starting)?;
        broker.set_limits(&body.limits)?;
        let PiFrame::Turn { body: initial } = io.receive(startup, &mut || {
            starting()?;
            broker.check_live()
        })?
        else {
            anyhow::bail!("fixture Pi did not expose initial registry");
        };
        let mut transcript = Vec::new();
        let mut outgoing = policy::finalize(&body.question, &initial, &transcript)?;
        self.pump_run(binding, "admitted")?;
        self.store
            .attach_launch(
                &binding.run_id,
                binding.generation,
                launch_id,
                self.clock.now()?,
            )
            .map_err(|_| anyhow::anyhow!("fixture launch attachment refused"))?;
        #[cfg(feature = "lite-analyst-faults")]
        self.probe_foreign_session(deadline)?;
        let mut evidence = Vec::new();
        let mut call_ids = HashSet::new();
        for turn in 1..=body.limits.turns {
            let (message, calls) = self.provider_turn(
                fixture.resources.credential(),
                binding,
                launch_id,
                deadline,
                &outgoing,
                broker,
                io,
                &mut call_ids,
            )?;
            ensure!(
                !calls.is_empty(),
                "fixture provider ended without local finish"
            );
            let mut working = || {
                self.pump_run(binding, "working")?;
                broker.check_live()
            };
            io.send(
                &json!({"type":"provider", "message":message}),
                deadline,
                &mut working,
            )?;
            transcript.push(message);
            for call in calls {
                let PiFrame::Tool { id, name, args } = io.receive(deadline, &mut || {
                    self.pump_run(binding, "working")?;
                    broker.check_live()
                })?
                else {
                    anyhow::bail!("fixture Pi did not dispatch the provider call");
                };
                ensure!(
                    id == call.id && name == call.name && args == call.args,
                    "fixture Pi dispatch disagrees with provider call"
                );
                policy::validate_args(&name, &args)?;
                self.pump_run(binding, "working")?;
                if name == "finish_report" {
                    let report = Sealed::new(args)
                        .map_err(|_| anyhow::anyhow!("fixture report arguments refused"))?;
                    #[cfg(feature = "lite-analyst-faults")]
                    self.finish_barrier(Case::FinishBefore, deadline)?;
                    broker.prove_live(deadline, &mut || self.pump_run(binding, "working"))?;
                    self.pump_run(binding, "working")?;
                    let offer = self
                        .store
                        .finish_report(
                            Finish {
                                run_id: &binding.run_id,
                                generation: binding.generation,
                                launch_id,
                                report,
                                evidence: &evidence,
                            },
                            || {
                                self.work.check_live()?;
                                broker.check_live()?;
                                io.check_waiting()?;
                                self.clock.now()
                            },
                        )
                        .map_err(|_| anyhow::anyhow!("fixture report publication refused"))?;
                    #[cfg(feature = "lite-analyst-faults")]
                    self.finish_barrier(Case::FinishAfter, deadline)?;
                    // Host-local acknowledgement only. Its loss cannot undo the
                    // committed winner, and no subsequent turn may see it.
                    let mut finishing = || self.work.check_live();
                    let deadline = Instant::now() + FRAME_TIMEOUT;
                    let content = String::from_utf8(wire::canonical(&json!({"offer":offer}))?)?;
                    io.send(
                        &json!({"type":"result", "content":content}),
                        deadline,
                        &mut finishing,
                    )?;
                    ensure!(
                        matches!(io.receive(deadline, &mut finishing)?, PiFrame::Done)
                            && io.buffer.is_empty(),
                        "fixture Pi did not end after local finish"
                    );
                    return Ok(());
                }
                let result = broker.invoke(&name, &args, deadline, &mut || {
                    self.pump_run(binding, "working")
                })?;
                self.pump_run(binding, "working")?;
                let content = String::from_utf8(wire::canonical_signed(&result.payload)?)?;
                io.send(
                    &json!({"type":"result", "content":content}),
                    deadline,
                    &mut || {
                        self.pump_run(binding, "working")?;
                        broker.check_live()
                    },
                )?;
                evidence.extend(result.receipts);
                transcript.push(json!({"role":"tool", "tool_call_id":id, "content":content}));
            }
            if turn == body.limits.turns {
                break;
            }
            let PiFrame::Turn { body: next } = io.receive(deadline, &mut || {
                self.pump_run(binding, "working")?;
                broker.check_live()
            })?
            else {
                anyhow::bail!("fixture Pi did not request a next turn");
            };
            outgoing = policy::finalize(&body.question, &next, &transcript)?;
        }
        anyhow::bail!("fixture turns exhausted without local finish");
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "named peers and authorities at the send boundary"
    )]
    fn provider_turn(
        &mut self,
        credential: &[u8],
        binding: &Binding,
        launch_id: &str,
        deadline: Instant,
        body: &[u8],
        broker: &mut Broker,
        io: &PiIo,
        call_ids: &mut HashSet<String>,
    ) -> Result<(Value, Vec<Call>)> {
        let mut endpoint = Provider::connect(
            credential,
            deadline.min(Instant::now() + FRAME_TIMEOUT),
            &mut || {
                self.pump_run(binding, "working")?;
                broker.check_live()?;
                io.check_waiting()
            },
        )?;
        let (reserve, lease) = self
            .reserve_turn(&binding.run_id, binding.generation, launch_id, body)?
            .map_err(|_| anyhow::anyhow!("fixture permit unavailable"))?;
        #[cfg(feature = "lite-analyst-faults")]
        let work = self.work;
        let admission = (|| {
            broker.prove_live(deadline, &mut || self.pump_run(binding, "working"))?;
            io.check_waiting()?;
            self.send_once(
                &lease,
                body,
                || {
                    #[cfg(feature = "lite-analyst-faults")]
                    if work.barrier(
                        Case::BrokerAfterAdmission,
                        deadline.min(lease.send_deadline()),
                        |_| work.check_live(),
                    )? {
                        broker.disconnect_for_fault()?;
                    }
                    broker.check_live()?;
                    io.check_waiting()
                },
                |exact| endpoint.send(exact),
            )
        })();
        let received = self.settle_exchange(&reserve, admission, |session| {
            let message = endpoint.receive(deadline, &mut || {
                session.pump_run(binding, "working")?;
                broker.check_live()?;
                io.check_waiting()
            })?;
            // Cross-turn correlation is part of exchange validity, not
            // a dispatch-time check after settling the completed tariff.
            let calls = if message["content"].is_string() {
                Vec::new()
            } else {
                plan_calls(&message, call_ids)?
            };
            Ok((message, calls))
        })?;
        self.pump_run(binding, "working")?;
        broker.check_live()?;
        received
    }

    // Keep a settled exchange failure distinct from failure to record settlement.
    fn settle_exchange<T>(
        &mut self,
        reserve: &ReserveTurn,
        admission: Result<SendAdmission<Result<()>>>,
        receive: impl FnOnce(&mut Self) -> Result<T>,
    ) -> Result<Result<T>> {
        let (outcome, received) = match admission {
            Ok(SendAdmission::HandedOff(Ok(()))) => {
                let reply = receive(self);
                let outcome = if reply.is_ok() {
                    Outcome::Completed
                } else {
                    Outcome::Indeterminate
                };
                (outcome, reply)
            }
            Ok(SendAdmission::ConsumedWithoutHandoff | SendAdmission::HandedOff(Err(_))) => (
                Outcome::Indeterminate,
                Err(anyhow::anyhow!("fixture provider handoff uncertain")),
            ),
            // send_once only returns Err before consuming. No policy or transport
            // refusal may be promoted to a new paid step after this result.
            Ok(SendAdmission::Refused(_)) | Err(_) => (
                Outcome::NotSent,
                Err(anyhow::anyhow!("fixture provider handoff refused")),
            ),
        };
        self.settle_turn(reserve, outcome)?;
        Ok(received)
    }

    pub(super) fn pump_work(&mut self, cutoff: Option<Instant>) -> Result<()> {
        self.tick()?;
        if self.work.readable(Instant::now())? {
            let frame_deadline = Instant::now() + FRAME_TIMEOUT;
            let deadline = cutoff.map_or(frame_deadline, |cutoff| cutoff.min(frame_deadline));
            let handled = self
                .work
                .receive_message(deadline)
                .and_then(|request| self.handle(request, deadline));
            if handled.is_err() {
                self.work.poison();
            }
            handled?;
        }
        Ok(())
    }

    fn pump_run(&mut self, binding: &Binding, state: &str) -> Result<()> {
        self.pump_run_before(binding, state, None)
    }

    fn pump_run_before(
        &mut self,
        binding: &Binding,
        state: &str,
        cutoff: Option<Instant>,
    ) -> Result<()> {
        self.pump_work(cutoff)?;
        let snapshot = self
            .store
            .read(&binding.run_id)
            .map_err(|_| anyhow::anyhow!("fixture run unavailable"))?;
        ensure!(
            snapshot.generation == binding.generation && snapshot.state == state,
            "fixture generation is no longer active"
        );
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum PiFrame {
    Mcp {
        request: Value,
    },
    Turn {
        body: Value,
    },
    Tool {
        id: String,
        name: String,
        args: Value,
    },
    Done,
    #[cfg(feature = "lite-analyst-faults")]
    SandboxDenied,
}

struct PiIo {
    io: UnixStream,
    buffer: Vec<u8>,
}

fn wait(
    io: &UnixStream,
    interest: PollFlags,
    deadline: Instant,
    pump: &mut dyn FnMut() -> Result<()>,
) -> Result<PollFlags> {
    let mut fds = [PollFd::new(io, interest | PollFlags::RDHUP)];
    loop {
        pump()?;
        let time = deadline.saturating_duration_since(Instant::now()).min(STEP);
        ensure!(!time.is_zero(), "fixture Pi deadline elapsed");
        let timeout = Timespec {
            tv_sec: 0,
            tv_nsec: time.subsec_nanos().into(),
        };
        match poll(&mut fds, Some(&timeout)) {
            Ok(0) | Err(Errno::INTR) => continue,
            Ok(_) => {
                pump()?;
                ensure!(Instant::now() < deadline, "fixture Pi deadline elapsed");
                return Ok(fds[0].revents());
            }
            Err(error) => return Err(error).context("poll fixture Pi"),
        }
    }
}

impl PiIo {
    fn negotiate_shim(
        &mut self,
        broker: &mut Broker,
        deadline: Instant,
        pump: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        for _ in 0..3 {
            let PiFrame::Mcp { request } = self.receive(deadline, &mut || {
                pump()?;
                broker.check_live()
            })?
            else {
                anyhow::bail!("fixture Pi skipped MCP negotiation");
            };
            if let Some(response) = broker.negotiate_stdio(&request, deadline, pump)? {
                self.send(
                    &json!({"type":"mcp", "response":response}),
                    deadline,
                    &mut || {
                        pump()?;
                        broker.check_live()
                    },
                )?;
            }
        }
        Ok(())
    }

    fn check_waiting(&self) -> Result<()> {
        let mut fds = [PollFd::new(&self.io, PollFlags::IN | PollFlags::RDHUP)];
        poll(&mut fds, Some(&Timespec::default())).context("check waiting fixture Pi")?;
        ensure!(
            self.buffer.is_empty() && fds[0].revents().is_empty(),
            "fixture Pi disconnected or pipelined"
        );
        Ok(())
    }

    fn send(
        &self,
        value: &Value,
        deadline: Instant,
        pump: &mut dyn FnMut() -> Result<()>,
    ) -> Result<()> {
        let mut bytes = serde_json::to_vec(value)?;
        ensure!(bytes.len() <= IPC_MAX, "fixture Pi frame too large");
        bytes.push(b'\n');
        let mut remaining = bytes.as_slice();
        while !remaining.is_empty() {
            let events = wait(&self.io, PollFlags::OUT, deadline, pump)?;
            ensure!(events == PollFlags::OUT, "fixture Pi write disconnected");
            match send(&self.io, remaining, SendFlags::NOSIGNAL) {
                Ok(n) if n != 0 => remaining = &remaining[n..],
                Err(Errno::AGAIN | Errno::INTR) => continue,
                _ => anyhow::bail!("fixture Pi write failed"),
            }
        }
        Ok(())
    }

    fn receive(
        &mut self,
        deadline: Instant,
        pump: &mut dyn FnMut() -> Result<()>,
    ) -> Result<PiFrame> {
        let mut scanned = 0;
        loop {
            if let Some(pos) = self.buffer[scanned..].iter().position(|b| *b == b'\n') {
                pump()?;
                ensure!(Instant::now() < deadline, "fixture Pi deadline elapsed");
                let end = scanned + pos;
                let frame = wire::decode(&self.buffer[..end])?;
                self.buffer.drain(..=end);
                return Ok(frame);
            }
            scanned = self.buffer.len();
            ensure!(scanned <= IPC_MAX, "fixture Pi frame too large");
            let events = wait(&self.io, PollFlags::IN, deadline, pump)?;
            ensure!(
                events.contains(PollFlags::IN),
                "fixture Pi read disconnected"
            );
            let mut bytes = [0; 8192];
            let max = bytes.len().min(IPC_MAX + 1 - scanned);
            match recvmsg(
                &self.io,
                &mut [IoSliceMut::new(&mut bytes[..max])],
                &mut RecvAncillaryBuffer::default(),
                RecvFlags::CMSG_CLOEXEC,
            ) {
                Ok(msg) => {
                    // A zero-sized ancillary buffer causes the kernel to drop passed rights.
                    ensure!(
                        msg.bytes > 0
                            && !msg
                                .flags
                                .intersects(ReturnFlags::CTRUNC | ReturnFlags::TRUNC),
                        "fixture Pi EOF or ancillary data"
                    );
                    self.buffer.extend_from_slice(&bytes[..msg.bytes]);
                }
                Err(Errno::AGAIN | Errno::INTR) => continue,
                Err(error) => return Err(error).context("read fixture Pi"),
            }
        }
    }
}

struct Call {
    id: String,
    name: String,
    args: Value,
}

fn plan_calls(message: &Value, seen: &mut HashSet<String>) -> Result<Vec<Call>> {
    let calls = message["tool_calls"]
        .as_array()
        .context("fixture provider did not issue calls")?;
    ensure!(
        (1..=9).contains(&calls.len()),
        "fixture provider tool batch invalid"
    );
    let mut out = Vec::with_capacity(calls.len());
    for (i, call) in calls.iter().enumerate() {
        let id = call["id"]
            .as_str()
            .filter(|id| wire::valid_id(id))
            .context("fixture call ID invalid")?;
        let name = call["function"]["name"]
            .as_str()
            .context("fixture call name invalid")?;
        let args = wire::decode(
            call["function"]["arguments"]
                .as_str()
                .context("fixture call args invalid")?
                .as_bytes(),
        )?;
        ensure!(
            !seen.contains(id) && out.iter().all(|c: &Call| c.id != id),
            "fixture call ID reused"
        );
        ensure!(
            matches!(
                name,
                "search_materials" | "read_evidence" | "calculate_metric" | "finish_report"
            ),
            "fixture provider tool forbidden"
        );
        ensure!(
            name != "finish_report" || i + 1 == calls.len(),
            "fixture finish must be last"
        );
        policy::validate_args(name, &args)?;
        out.push(Call {
            id: id.to_owned(),
            name: name.to_owned(),
            args,
        });
    }
    seen.extend(out.iter().map(|call| call.id.clone()));
    Ok(out)
}

#[cfg(test)]
#[path = "investigation_tests.rs"]
mod tests;
