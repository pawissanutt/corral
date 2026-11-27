//! Opt-in, fixture-only daemon entry point. Does not enter the workspace/owner daemon.
mod bootstrap;
mod broker;
mod clock;
#[cfg(feature = "lite-analyst-faults")]
mod faults;
mod investigation;
mod isolation;
mod policy;
mod provider;
mod reports;
mod store;
mod turns;
mod wire;
mod work;

use anyhow::ensure;
use bootstrap::{Bootstrap, Enrollment, Resources};
use clock::Clock;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    path::Path,
    time::{Duration, Instant},
};
use store::Store;
use turns::{Outcome, PermitLease, ReserveTurn, SettleTurn};
use wire::{Fault, Fence, FenceReason, Message};
use work::{FRAME_TIMEOUT, Work};

/// Enter the launcher-provisioned fixture daemon only on explicit opt-in.
///
/// # Safety
/// In the opted-in mode the caller must be the fresh, single-threaded CLI with sole
/// ownership of any open descriptors in slots 3–8, provisioned by the trusted launcher.
#[doc(hidden)]
pub unsafe fn run_fixture(enabled: bool) -> anyhow::Result<()> {
    // SAFETY: as required by run_fixture's documented caller contract.
    unsafe {
        run_fixture_configured(
            enabled,
            #[cfg(feature = "lite-analyst-faults")]
            None,
        )
    }
}

/// Enter a separately built fixture with one named experiment.
///
/// # Safety
/// Same as `run_fixture`, additionally transferring exclusive ownership of slot 9.
#[cfg(feature = "lite-analyst-faults")]
#[doc(hidden)]
pub unsafe fn run_fixture_with_fault(enabled: bool, case: &str, fd: i32) -> anyhow::Result<()> {
    ensure!(
        fd == faults::FD,
        "wrong Corral fixture fault descriptor slot"
    );
    let case = faults::Case::parse(case)?;
    // SAFETY: the caller also transfers the fixed barrier slot.
    unsafe { run_fixture_configured(enabled, Some(case)) }
}

unsafe fn run_fixture_configured(
    enabled: bool,
    #[cfg(feature = "lite-analyst-faults")] case: Option<faults::Case>,
) -> anyhow::Result<()> {
    ensure!(
        enabled,
        "public Lite fixture requires explicit per-launch opt-in"
    );
    // SAFETY: the CLI transfers its preconnected fixture-only descriptor table here.
    let bootstrap = unsafe {
        Bootstrap::inherit(
            #[cfg(feature = "lite-analyst-faults")]
            case,
        )
    };
    // Do not log private paths, peer bytes, tokens, or SQL details to the driver.
    bootstrap
        .and_then(serve)
        .map_err(|_| anyhow::anyhow!("public Lite fixture stopped or unavailable"))
}

pub fn run_shim() -> anyhow::Result<()> {
    broker::shim_main()
        .map_err(|_| anyhow::anyhow!("public Lite fixture shim stopped or unavailable"))
}

fn serve(mut bootstrap: Bootstrap) -> anyhow::Result<()> {
    isolation::protect_supervisor()?;
    provider::restrict_egress()?;
    policy::prompt()?;
    policy::model_descriptors()?;
    let e = &bootstrap.enrollment;
    let work = Work::new(bootstrap.work, bootstrap.lifetime)?
        .with_broker(bootstrap.resources.broker_handle()?);
    #[cfg(feature = "lite-analyst-faults")]
    let work = work.with_faults(bootstrap.faults);
    let bootstrap_deadline = bootstrap.resources.deadline();
    work.send(&e.link_ready(), bootstrap_deadline)?;
    e.check_link_ready(wire::decode(&work.receive(bootstrap_deadline)?)?)?;
    let Message::Request { id, op, params, .. } = work.receive_message(bootstrap_deadline)? else {
        anyhow::bail!("fixture expected hello");
    };
    let hello = if op == "hello" {
        wire::params(params).and_then(|args| wire::hello(&e.corral_instance, args))
    } else {
        Err(Fault::Unsupported)
    };
    let Ok(hello) = hello else {
        work.respond(id, hello, bootstrap_deadline)?;
        anyhow::bail!("fixture hello refused");
    };
    let (mut store, clock) = Store::enroll_with(
        Path::new("/state"),
        e,
        |db| {
            let clock = Clock::load(db)?;
            let (earliest, _) = clock.times()?;
            Ok((clock, earliest))
        },
        || work.check_live(),
    )?;
    let outcome = (|| {
        work.respond(id, Ok(hello), bootstrap_deadline)?;
        bootstrap.resources.ready(e, || work.check_live())?;
        match (Session {
            store: &mut store,
            work: &work,
            clock,
            pending_bind: None,
            pending_fence: None,
            fixture: Some(Fixture {
                enrollment: e,
                resources: &bootstrap.resources,
            }),
        })
        .run()
        {
            Err(e) if e.is::<work::LifetimeEnded>() => Ok(()),
            other => other,
        }
    })();
    let fenced = store
        .disconnected()
        .map_err(|_| anyhow::anyhow!("fixture disconnect fence failed"));
    fenced.and(outcome)
}

// One serialized supervisor gate. Bounded reverse calls pump incoming requests before
// returning their matched reply, so cancel/fence can win while a permit/binding is in flight.
#[derive(Debug, PartialEq, Eq)]
enum SendAdmission<T> {
    Refused(Fault),
    ConsumedWithoutHandoff,
    HandedOff(T),
}

#[derive(Clone, Copy)]
struct Fixture<'a> {
    enrollment: &'a Enrollment,
    resources: &'a Resources,
}

struct PendingStart {
    id: String,
    body: wire::StartBody,
}

struct Session<'a> {
    store: &'a mut Store,
    work: &'a Work,
    clock: Clock,
    pending_bind: Option<PendingStart>,
    pending_fence: Option<Fence>,
    fixture: Option<Fixture<'a>>,
}

impl Session<'_> {
    fn run(&mut self) -> anyhow::Result<()> {
        self.replay_settlements()?;
        loop {
            self.drain_control()?;
            if let Some(start) = self.pending_bind.take() {
                if let Some(binding) = self.bind_at_lite(&start.id)? {
                    let fixture = self
                        .fixture
                        .ok_or_else(|| anyhow::anyhow!("fixture launch unavailable"))?;
                    self.investigate(fixture, binding, start.body)?;
                }
                continue;
            }
            if self
                .work
                .readable(Instant::now() + Duration::from_millis(200))?
            {
                let deadline = Instant::now() + FRAME_TIMEOUT;
                let request = self.work.receive_message(deadline)?;
                self.handle(request, deadline)?;
            }
        }
    }

    fn replay_settlements(&mut self) -> anyhow::Result<()> {
        // Only already sealed receipt reconciliation is allowed after launcher re-exec.
        let pending = self
            .store
            .pending_settlements(self.clock.times()?.0)
            .map_err(|_| anyhow::anyhow!("read fixture settlement ledger"))?;
        for settlement in pending {
            self.record_settlement(&settlement)?;
        }
        Ok(())
    }

    fn tick(&mut self) -> anyhow::Result<()> {
        self.work.check_live()?;
        let (earliest, latest) = self.clock.times()?;
        let notice = self
            .store
            .maintenance(latest)
            .map_err(|_| anyhow::anyhow!("maintain fixture lifecycle"))?;
        if let Some(notice) = notice {
            self.queue_fence(notice)?;
        }
        self.store
            .finalize_turns(earliest)
            .map(|_| ())
            .map_err(|_| anyhow::anyhow!("maintain fixture accounting"))
    }

    // An idle channel sends local fences before starting any unrelated reverse RPC.
    // call() only ticks while in flight; it must never drain recursively.
    fn drain_control(&mut self) -> anyhow::Result<()> {
        self.tick()?;
        self.flush_local_fence()
    }

    fn queue_fence(&mut self, notice: Fence) -> anyhow::Result<()> {
        // Only one bound launch slot; no new bind is submitted before draining it.
        ensure!(self.pending_fence.is_none(), "undelivered fixture fence");
        self.pending_fence = Some(notice);
        Ok(())
    }

    fn stop_local(&mut self, id: &str, generation: u64, reason: FenceReason) -> anyhow::Result<()> {
        if let Some(notice) = self
            .store
            .stop_local(id, generation, reason)
            .map_err(|_| anyhow::anyhow!("commit local fixture fence"))?
        {
            self.queue_fence(notice)?;
        }
        Ok(())
    }

    // Run only when no reverse request is in flight. The row is already stopped;
    // an absent/refused receipt closes work, never rolls back the local fence.
    fn flush_local_fence(&mut self) -> anyhow::Result<()> {
        if let Some(notice) = self.pending_fence.take() {
            let reply = self.call::<wire::Fenced>(
                "fence_session",
                &notice,
                Instant::now() + FRAME_TIMEOUT,
            )?;
            ensure!(
                matches!(reply, Ok(wire::Fenced { fenced: true })),
                "fixture fence receipt refused"
            );
        }
        Ok(())
    }

    fn handle(&mut self, request: Message, deadline: Instant) -> anyhow::Result<()> {
        let Message::Request { id, op, params, .. } = request else {
            anyhow::bail!("unexpected fixture response");
        };
        ensure!(
            !matches!(
                op.as_str(),
                "hello" | "bind_session" | "reserve_turn" | "settle_turn"
            ),
            "fixture request direction or phase is invalid"
        );
        self.work.check_live()?;
        #[cfg(feature = "lite-analyst-faults")]
        let already_pending = self.pending_bind.is_some();
        let result = self.apply(&op, params, self.clock.times()?);
        #[cfg(feature = "lite-analyst-faults")]
        if result.is_ok() {
            let case = match op.as_str() {
                "start_investigation" if !already_pending && self.pending_bind.is_some() => {
                    Some(faults::Case::StartCommitted)
                }
                "ack_report" => Some(faults::Case::AckCommitted),
                _ => None,
            };
            if let Some(case) = case
                && self
                    .work
                    .barrier(case, deadline, || self.work.check_live())?
            {
                self.work.drop_response()?;
                anyhow::bail!("fixture committed response intentionally dropped");
            }
        }
        self.work.respond(id, result, deadline)
    }

    fn apply(&mut self, op: &str, value: Value, bounds: (u64, u64)) -> wire::Result<Value> {
        fn as_value(result: impl Serialize) -> wire::Result<Value> {
            serde_json::to_value(result).map_err(|_| Fault::Unavailable)
        }
        match op {
            "start_investigation" => {
                let request: wire::Start = wire::params(value)?;
                let start = self
                    .store
                    .start_with(&request, bounds, || self.work.check_live())?;
                if start.fresh {
                    self.pending_bind = Some(PendingStart {
                        id: start.snapshot.run_id.clone(),
                        body: request.body()?,
                    });
                }
                as_value(start.snapshot)
            }
            "read_investigation" => {
                let request: wire::Read = wire::params(value)?;
                as_value(self.store.read(&request.run_id)?)
            }
            "cancel_investigation" => {
                let request: wire::Cancel = wire::params(value)?;
                as_value(self.store.stop(
                    &request.run_id,
                    request.generation,
                    FenceReason::Stopped,
                )?)
            }
            "fetch_report" => self.store.fetch_report(wire::params(value)?),
            "ack_report" => self.store.ack_report(wire::params(value)?),
            "fence_session" => {
                let request: wire::Fence = wire::params(value)?;
                self.store
                    .stop(&request.run_id, request.generation, request.reason)?;
                Ok(json!({"fenced": true}))
            }
            _ => Err(Fault::Unsupported),
        }
    }

    fn call<T: DeserializeOwned>(
        &mut self,
        op: &str,
        params: &impl Serialize,
        deadline: Instant,
    ) -> anyhow::Result<wire::Result<T>> {
        let request = Message::Request {
            v: 1,
            id: wire::fresh_id()?,
            op: op.to_owned(),
            params: serde_json::to_value(params)?,
        };
        self.exchange(&request, deadline)
    }

    fn exchange<T: DeserializeOwned>(
        &mut self,
        request: &Message,
        deadline: Instant,
    ) -> anyhow::Result<wire::Result<T>> {
        let result = self.exchange_inner(request, deadline);
        if result
            .as_ref()
            .is_err_and(|e| !e.is::<work::LifetimeEnded>())
        {
            self.work.poison();
        }
        result
    }

    fn exchange_inner<T: DeserializeOwned>(
        &mut self,
        request: &Message,
        deadline: Instant,
    ) -> anyhow::Result<wire::Result<T>> {
        let Message::Request { id, .. } = request else {
            anyhow::bail!("invalid fixture outgoing work request");
        };
        self.work.send(request, deadline)?;
        loop {
            // No DB transaction is held over I/O. A new incoming request is serviced
            // while the opposite-direction request is outstanding; no nested calls.
            self.tick()?;
            let message = self.work.receive_message(deadline)?;
            self.work.check_live()?;
            match message {
                Message::Success {
                    id: got, result, ..
                } if got == *id => {
                    let typed = wire::params(result)
                        .map_err(|_| anyhow::anyhow!("invalid fixture reverse result"))?;
                    return Ok(Ok(typed));
                }
                Message::Failure { id: got, error, .. } if got == *id => return Ok(Err(error)),
                request @ Message::Request { .. } => self.handle(request, deadline)?,
                _ => anyhow::bail!("uncorrelated fixture response"),
            }
        }
    }

    fn bind_at_lite(&mut self, id: &str) -> anyhow::Result<Option<wire::Binding>> {
        self.drain_control()?;
        let binding = match self.store.binding(id) {
            Ok(b) => b,
            Err(Fault::Stopped) => return Ok(None),
            Err(_) => anyhow::bail!("fixture binding unavailable"),
        };
        let result =
            self.call::<wire::Bound>("bind_session", &binding, Instant::now() + FRAME_TIMEOUT)?;
        // A stop might have been handled in call(); a positive reply must not revive it.
        if matches!(result, Ok(wire::Bound { bound: true })) {
            match self.store.bind(id, binding.generation, self.clock.now()?) {
                Ok(()) => return Ok(Some(binding)),
                Err(Fault::Stopped) => {
                    // Lite installed its binding but local stop/expiry won before we
                    // could record bound_epoch; there is no stored binding to notify.
                    let reason = match self
                        .store
                        .read(id)
                        .map_err(|_| anyhow::anyhow!("read fenced fixture"))?
                        .reason
                        .as_deref()
                    {
                        Some("stale_scope") => FenceReason::StaleScope,
                        Some("stopped") => FenceReason::Stopped,
                        _ => FenceReason::Expired,
                    };
                    self.store
                        .stop(id, binding.generation, reason)
                        .map_err(|_| anyhow::anyhow!("commit fence for late fixture bind"))?;
                    self.queue_fence(Fence {
                        run_id: id.to_owned(),
                        generation: binding.generation,
                        reason,
                    })?;
                    return Ok(None);
                }
                Err(_) => anyhow::bail!("fixture binding could not commit"),
            }
        }
        let reason = match result {
            Err(Fault::Expired) => FenceReason::Expired,
            Err(Fault::StaleScope) => FenceReason::StaleScope,
            _ => FenceReason::Stopped,
        };
        self.stop_local(id, binding.generation, reason)?;
        Ok(None)
    }

    // Handles and the exact finalized body remain supervisor-only.
    fn reserve_turn(
        &mut self,
        run: &str,
        generation: u64,
        launch: &str,
        body: &[u8],
    ) -> anyhow::Result<wire::Result<(ReserveTurn, PermitLease)>> {
        self.drain_control()?;
        let now = Instant::now();
        let now_ms = self.clock.reserve_time()?;
        let held =
            match self
                .store
                .hold_turn_with(run, generation, launch, body, (now_ms, now), || {
                    self.work.check_live()?;
                    self.clock.reserve_time()
                }) {
                Ok(held) => held,
                Err(fault) => return Ok(Err(fault)),
            };
        let checked_at = Instant::now();
        let checked_ms = self.clock.reserve_time()?;
        if checked_ms / clock::DAY_MS != now_ms / clock::DAY_MS {
            self.stop_local(run, generation, FenceReason::Expired)?;
            self.flush_local_fence()?;
            return Ok(Err(Fault::Expired));
        }
        let request = match held.reserve_request(checked_ms, checked_at) {
            Ok(request) => request,
            Err(fault) => {
                self.stop_local(run, generation, FenceReason::Expired)?;
                self.flush_local_fence()?;
                return Ok(Err(fault));
            }
        };
        // A backpressured work socket cannot carry the reserve frame across
        // the conservative end of the UTC bucket or run deadline.
        let next_day = (checked_ms / clock::DAY_MS + 1) * clock::DAY_MS;
        let remaining = held.deadline_ms.min(next_day).saturating_sub(checked_ms);
        let deadline = (now + FRAME_TIMEOUT).min(checked_at + Duration::from_millis(remaining));
        let reply = self.call("reserve_turn", request, deadline);
        let accepted = match reply {
            Ok(Ok(permit)) => {
                self.store
                    .accept_permit(&held, permit, self.clock.now()?, Instant::now())
            }
            Ok(Err(fault)) => Err(fault),
            Err(error) => {
                // In-flight reverse request is unresolved. Do not issue another request
                // on this channel or advance the step; reconnect is bookkeeping only.
                self.store
                    .disconnected()
                    .map_err(|_| anyhow::anyhow!("fence ambiguous reserve"))?;
                return Err(error);
            }
        };
        match accepted {
            Ok(lease) => Ok(Ok((request.clone(), lease))),
            Err(fault) => {
                let reason = match fault {
                    Fault::Expired => FenceReason::Expired,
                    Fault::StaleScope => FenceReason::StaleScope,
                    _ => FenceReason::Stopped,
                };
                self.stop_local(run, generation, reason)?;
                self.flush_local_fence()?;
                Ok(Err(fault))
            }
        }
    }

    fn send_once<T>(
        &mut self,
        lease: &PermitLease,
        body: &[u8],
        check_session: impl FnOnce() -> anyhow::Result<()>,
        send: impl FnOnce(&[u8]) -> T,
    ) -> anyhow::Result<SendAdmission<T>> {
        self.tick()?;
        // Never skip a queued control frame, including a stop. Yield to the work
        // loop without consuming instead of waiting under this send gate.
        if self.work.readable(Instant::now())? {
            return Ok(SendAdmission::Refused(Fault::Stopped));
        }
        if let Err(fault) = self
            .store
            .consume_turn(lease, body, self.clock.now()?, Instant::now())
        {
            return Ok(SendAdmission::Refused(fault));
        }
        // Failures after the durable commit are distinguishable from pre-consume
        // refusal and must settle indeterminate (not not_sent).
        let checked = (|| -> anyhow::Result<bool> {
            if self.work.readable(Instant::now())? {
                return Ok(false);
            }
            // Work watches broker bootstrap, not the retained per-run Pi and
            // shim channels. Observe those locally again after the commit.
            check_session()?;
            Ok(lease
                .check_send_time(self.clock.now()?, Instant::now())
                .is_ok())
        })();
        if !matches!(checked, Ok(true)) {
            return Ok(SendAdmission::ConsumedWithoutHandoff);
        }
        Ok(SendAdmission::HandedOff(send(body)))
    }

    fn settle_turn(&mut self, request: &ReserveTurn, outcome: Outcome) -> anyhow::Result<()> {
        let sealed = self
            .store
            .seal_turn(request, outcome, self.clock.times()?.0)
            .map_err(|_| anyhow::anyhow!("seal fixture settlement"))?;
        self.record_settlement(&sealed)
    }

    fn record_settlement(&mut self, settlement: &SettleTurn) -> anyhow::Result<()> {
        self.drain_control()?;
        let deadline = Instant::now() + FRAME_TIMEOUT;
        #[cfg(feature = "lite-analyst-faults")]
        let receipt = if self.work.fault_armed(faults::Case::SettleDuplicate) {
            self.duplicate_settlement(settlement, deadline)?
        } else {
            self.call("settle_turn", settlement, deadline)?
        };
        #[cfg(not(feature = "lite-analyst-faults"))]
        let receipt = self.call("settle_turn", settlement, deadline)?;
        let receipt = receipt.map_err(|_| anyhow::anyhow!("fixture settlement was refused"))?;
        let (earliest, latest) = self.clock.times()?;
        self.store
            .observe_settlement_at(settlement, &receipt, earliest, latest)
            .map_err(|_| anyhow::anyhow!("fixture settlement receipt did not match"))
    }
}

#[cfg(test)]
#[path = "lite_analyst/tests.rs"]
mod tests;
