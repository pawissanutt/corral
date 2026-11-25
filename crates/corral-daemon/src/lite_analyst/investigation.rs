//! The one-shot fixture investigation. The outer session remains the only
//! lifecycle/permit decision maker, even while waiting for either child.
use super::{
    Fixture, SendAdmission, Session,
    broker::Broker,
    isolation::{Payload, Sandbox},
    policy,
    provider::Provider,
    turns::Outcome,
    wire::{self, Binding, FenceReason, StartBody},
    work::FRAME_TIMEOUT,
};
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
        let result = self.converse(
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
        // C4 owns durable publish/fetch/ack. A validated local draft is not
        // a finished investigation or a report offer; discard it here.
        let failure = initiated.then_some(if result.is_ok() {
            "report_unavailable"
        } else {
            "unavailable"
        });
        self.store
            .confirm_teardown(&binding.run_id, binding.generation, &launch_id, failure)
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
    ) -> Result<Vec<u8>> {
        let startup = deadline.min(Instant::now() + STARTUP_TIMEOUT);
        self.pump_run(binding, "admitted")?;
        let (shim, stdio) = Sandbox::spawn(Payload::Shim)?;
        children.shim = Some(shim);
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
        broker.set_limits(&body.limits)?;
        self.pump_run(binding, "admitted")?;
        broker.check_live()?;
        let (pi, io) = Sandbox::spawn(Payload::Pi)?;
        children.pi = Some(pi);
        children.io = Some(PiIo {
            io,
            buffer: Vec::new(),
        });
        let io = children.io.as_mut().context("fixture Pi missing")?;
        let mut starting = || {
            self.pump_run(binding, "admitted")?;
            broker.check_live()
        };
        io.send(
            &json!({"type":"start", "question":body.question, "prompt":policy::prompt()?,
            "tools":policy::model_descriptors()?}),
            startup,
            &mut starting,
        )?;
        let PiFrame::Turn { body: initial } = io.receive(startup, &mut starting)? else {
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
        let mut citations = HashSet::new();
        let mut call_ids = HashSet::new();
        for turn in 1..=body.limits.turns {
            let message = self.provider_turn(
                fixture.resources.credential(),
                binding,
                launch_id,
                deadline,
                &outgoing,
                broker,
                io,
            )?;
            // Verify every resulting dispatch before starting the first one.
            let calls = plan_calls(&message, &mut call_ids)?;
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
                    broker.check_live()?;
                    let draft = finish_draft(args, &citations)?;
                    let mut finishing = || {
                        self.pump_run(binding, "working")?;
                        broker.check_live()
                    };
                    // Local terminal acknowledgement to Pi only; never enter MCP
                    // or a subsequent provider body. No report is offered in C3.
                    io.send(
                        &json!({"type":"result", "content":"{\"status\":\"ok\"}"}),
                        deadline,
                        &mut finishing,
                    )?;
                    ensure!(
                        matches!(io.receive(deadline, &mut finishing)?, PiFrame::Done)
                            && io.buffer.is_empty(),
                        "fixture Pi did not end after local finish"
                    );
                    return Ok(draft);
                }
                let result = broker.invoke(&name, &args, deadline, &mut || {
                    self.pump_run(binding, "working")
                })?;
                self.pump_run(binding, "working")?;
                if let Some(handle) = result.get("citation").and_then(Value::as_str) {
                    citations.insert(handle.to_owned());
                }
                let content = String::from_utf8(wire::canonical_signed(&result)?)?;
                io.send(
                    &json!({"type":"result", "content":content}),
                    deadline,
                    &mut || {
                        self.pump_run(binding, "working")?;
                        broker.check_live()
                    },
                )?;
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
        broker: &Broker,
        io: &PiIo,
    ) -> Result<Value> {
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
        let admission = (|| {
            broker.prove_live(deadline, &mut || self.pump_run(binding, "working"))?;
            io.check_waiting()?;
            self.send_once(&lease, body, |exact| endpoint.send(exact))
        })();
        let (outcome, received) = match admission {
            Ok(SendAdmission::HandedOff(Ok(()))) => {
                let reply = endpoint.receive(deadline, &mut || {
                    self.pump_run(binding, "working")?;
                    broker.check_live()?;
                    io.check_waiting()
                });
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
        self.settle_turn(&reserve, outcome)?;
        self.pump_run(binding, "working")?;
        broker.check_live()?;
        received
    }

    fn pump_run(&mut self, binding: &Binding, state: &str) -> Result<()> {
        self.tick()?;
        if self.work.readable(Instant::now())? {
            let deadline = Instant::now() + FRAME_TIMEOUT;
            let handled = self
                .work
                .receive_message(deadline)
                .and_then(|request| self.handle(request, deadline));
            if handled.is_err() {
                self.work.poison();
            }
            handled?;
        }
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
    Turn {
        body: Value,
    },
    Tool {
        id: String,
        name: String,
        args: Value,
    },
    Done,
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

fn finish_draft(args: Value, issued: &HashSet<String>) -> Result<Vec<u8>> {
    #[derive(Deserialize, serde::Serialize, PartialEq, Eq, Ord, PartialOrd)]
    #[serde(deny_unknown_fields)]
    struct Citation {
        start_byte: usize,
        end_byte: usize,
        handle: String,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Draft {
        text: String,
        citations: Vec<Citation>,
    }
    let Draft {
        text,
        mut citations,
    } = serde_json::from_value(args)?;
    for citation in &citations {
        ensure!(
            citation.start_byte < citation.end_byte
                && text.is_char_boundary(citation.start_byte)
                && text.is_char_boundary(citation.end_byte)
                && issued.contains(&citation.handle),
            "fixture report has an unissued citation or invalid span"
        );
    }
    citations.sort_unstable();
    ensure!(
        citations.windows(2).all(|w| w[0] != w[1]),
        "duplicate fixture report citation"
    );
    let bytes = wire::canonical(
        &json!({"v":1, "kind":"investigation_report", "text":text, "citations":citations}),
    )?;
    ensure!(bytes.len() <= 65536, "fixture report too large");
    Ok(bytes)
}

#[cfg(test)]
#[path = "investigation_tests.rs"]
mod tests;
