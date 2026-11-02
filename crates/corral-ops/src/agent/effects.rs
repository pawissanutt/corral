//! op-receipts (T-252 F1): receipts the existing operations write inside their own final
//! transactions, the post-commit effects a receipt still owes, the gate as a two-phase
//! operation settled only from its attempt's evidence, the wake subscriptions written with a
//! wait or gate reservation, the PM two-phase journal for capture and legacy replies, and the
//! open-question refusals of done and verdict. No wrapper holds a lock around an operation.
use anyhow::{Context, Result, bail, ensure};
use corral_core::agent::{
    self, AGENT_RECORD_VERSION, AgentGateRef, AgentReport, AgentReportKind, AgentRunState,
    ContainerRef, Executor, RECEIPTS_MAX, REPORTS_MAX, RESULT_MAX_BYTES, ReceiptEffect,
    ReceiptState, RequestOrigin, RequestReceipt, WAKES_MAX, WakeKind, WakeState, WakeSubscription,
    pm_state,
};
use corral_core::{Store, Task, TaskRef, Verdict};
use serde_json::{Value, json};

mod done;
pub(crate) use done::swept as swept_done;

use super::admission;
use super::call::{CallResponse, RejectCode, Rejection};
use crate::Ctx;

mod head;
pub(crate) use head::{recheck_head, record_head};

#[cfg(test)]
#[path = "effects_artifact_head_tests.rs"]
mod artifact_head_tests;
#[cfg(test)]
#[path = "effects_head_tests.rs"]
mod head_tests;
pub use crate::review_budget::{DueWake, due_gate_wakes, fire_wake, own_attempt};

/// Test-only crash and race points, a no-op in shipped builds. In-process tests install the
/// request path's hook; a child executor arms comma-separated points through
/// `CORRAL_AGENT_FAILPOINT`, announces each on stdout and blocks on stdin until its parent
/// releases or kills it.
pub fn failpoint(name: &str) {
    super::receipts::failpoint(name);
    #[cfg(test)]
    if std::env::var("CORRAL_AGENT_FAILPOINT")
        .is_ok_and(|armed| armed.split(',').any(|p| p == name))
    {
        use std::io::{BufRead, Write};
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "FAILPOINT {name}");
        let _ = out.flush();
        drop(out);
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
    }
}

/// The kernel start time of a live `pid`; `None` when no live process has it.
pub fn process_start(pid: u32) -> Result<Option<String>> {
    #[cfg(target_os = "linux")]
    {
        let path = format!("/proc/{pid}/stat");
        let stat = match read_stat(&path) {
            Ok(stat) => stat,
            Err(error) if stat_gone(&error) => return Ok(None),
            Err(error) => return Err(error).with_context(|| format!("reading {path}")),
        };
        // After the parenthesized command, state is field 3 and starttime field 22.
        let fields: Vec<&str> = stat
            .rsplit_once(')')
            .map(|(_, rest)| rest.split_whitespace().collect())
            .unwrap_or_default();
        if matches!(fields.first(), Some(&"Z" | &"X")) {
            return Ok(None);
        }
        Ok(fields.get(19).map(|start| (*start).to_owned()))
    }
    #[cfg(not(target_os = "linux"))]
    Ok(crate::native_process::inspect(pid)?.map(|process| process.started))
}

#[cfg(target_os = "linux")]
fn read_stat(path: &str) -> std::io::Result<String> {
    #[cfg(test)]
    if let Some(errno) = STAT_FAULT.with(std::cell::Cell::take) {
        return Err(std::io::Error::from_raw_os_error(errno));
    }
    std::fs::read_to_string(path)
}

// Test-only: the errno the next `/proc/<pid>/stat` read on this thread fails with, standing in
// for a process that exits mid-read.
#[cfg(all(test, target_os = "linux"))]
thread_local! {
    pub(super) static STAT_FAULT: std::cell::Cell<Option<i32>> = const { std::cell::Cell::new(None) };
}

/// A `/proc/<pid>/stat` read that failed because the process is gone: absent, or exited
/// (ESRCH) while the read was in flight.
#[cfg(target_os = "linux")]
fn stat_gone(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound
        || error.raw_os_error() == Some(rustix::io::Errno::SRCH.raw_os_error())
}

/// This `corral agent call` process, recorded on a reservation.
pub fn executor() -> Result<Executor> {
    let pid = std::process::id();
    let start_time = process_start(pid)?.context("reading this process's start time")?;
    Ok(Executor { pid, start_time })
}

/// Verified gone: no live process has the recorded pid and start time.
pub fn executor_gone(executor: &Executor) -> Result<bool> {
    Ok(process_start(executor.pid)?.as_deref() != Some(executor.start_time.as_str()))
}

fn reject(code: RejectCode, reason: impl Into<String>) -> anyhow::Error {
    Rejection::new(code, reason).into()
}

/// Raised in a swept operation's final closure when its request already committed: the
/// closure writes nothing and the caller answers with the stored receipt.
#[derive(Debug)]
pub struct Replayed(pub Box<RequestReceipt>);

impl std::fmt::Display for Replayed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "request {} already committed", self.0.request_id)
    }
}

impl std::error::Error for Replayed {}

/// Where a request's receipt lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Home {
    Container(ContainerRef),
    /// A PM receipt, in the target task's `agent_pm_receipts`.
    PmTask(String),
}

impl Home {
    pub fn of(origin: &RequestOrigin) -> Home {
        Home::Container(origin.binding.container.clone())
    }

    fn task(&self) -> Option<&str> {
        match self {
            Home::Container(container) => container.task(),
            Home::PmTask(task) => Some(task),
        }
    }
}

fn receipts<'a>(task: &'a Task, home: &Home) -> &'a [RequestReceipt] {
    match home {
        Home::Container(c) => agent::container_state(task, c).map_or(&[], |s| &s.receipts),
        Home::PmTask(_) => &task.agent_pm_receipts,
    }
}

fn receipts_mut<'a>(task: &'a mut Task, home: &Home) -> Result<&'a mut Vec<RequestReceipt>> {
    match home {
        Home::Container(container) => Ok(&mut state_mut(task, container)?.receipts),
        Home::PmTask(_) => Ok(&mut task.agent_pm_receipts),
    }
}

fn unbound() -> anyhow::Error {
    reject(
        RejectCode::ContainerNotFound,
        "the agent binding disappeared",
    )
}

fn state_mut<'a>(task: &'a mut Task, container: &ContainerRef) -> Result<&'a mut AgentRunState> {
    agent::container_state_mut(task, container).ok_or_else(unbound)
}

fn new_receipt(
    origin: &RequestOrigin,
    state: ReceiptState,
    result: Value,
    effect: Option<ReceiptEffect>,
    executor: Option<Executor>,
) -> Result<RequestReceipt> {
    let bytes = serde_json::to_vec(&result)?.len();
    ensure!(
        bytes <= RESULT_MAX_BYTES,
        "receipt result of {bytes} bytes exceeds {RESULT_MAX_BYTES}"
    );
    let now = corral_core::clock::now();
    Ok(RequestReceipt {
        version: AGENT_RECORD_VERSION,
        request_id: origin.request_id.clone(),
        op: origin.op.clone(),
        payload_sha256: origin.payload_sha256.clone(),
        identity: origin.identity.clone(),
        state,
        result,
        executor,
        effect,
        evidence: Vec::new(),
        post_pending: Vec::new(),
        at: now,
        committed_at: (state == ReceiptState::Committed).then_some(now),
    })
}

fn push(receipts: &mut Vec<RequestReceipt>, receipt: RequestReceipt) -> Result<()> {
    if receipts.len() >= RECEIPTS_MAX {
        return Err(reject(
            RejectCode::ReceiptLimit,
            format!("the container already holds {RECEIPTS_MAX} receipts"),
        ));
    }
    receipts.push(receipt);
    Ok(())
}

fn effect(kind: &str, reference: &str) -> Option<ReceiptEffect> {
    Some(ReceiptEffect {
        kind: kind.into(),
        reference: reference.into(),
    })
}

/// The first statement of a swept operation's final closure. An identical stored receipt
/// aborts it with `Replayed`; a new request repeats the liveness, fence and saved-pane check
/// (for the PM, its binding and project scope) in the operation's own transaction.
pub fn recheck(store: &Store, task: &Task, origin: &RequestOrigin, home: &Home) -> Result<()> {
    let id = &origin.request_id;
    if let Some(stored) = receipts(task, home).iter().find(|r| r.request_id == *id) {
        admission::same_request(stored, &origin.op, &origin.payload_sha256)?;
        return Err(Replayed(Box::new(stored.clone())).into());
    }
    match home {
        Home::Container(_) => admission::recheck_task(task, &origin.binding),
        Home::PmTask(_) => {
            if let Some((_, stored)) = pm_task_receipt(store, origin)? {
                return Err(Replayed(Box::new(stored)).into());
            }
            let pm = pm_state::load(store)?
                .ok_or_else(|| reject(RejectCode::ContainerNotFound, "no corral-pi PM is bound"))?;
            if let Some(stored) = pm.state.receipt(id) {
                admission::same_request(stored, &origin.op, &origin.payload_sha256)?;
                if stored.state != ReceiptState::Reserved {
                    return Err(Replayed(Box::new(stored.clone())).into());
                }
            }
            admission::recheck_pm(&pm, origin.binding.generation)?;
            if !super::hooks::pm::contains(
                store,
                &pm.state.binding.project,
                task.project.as_deref(),
            )? {
                return Err(reject(
                    RejectCode::OutOfScope,
                    "the task is outside the PM's project scope",
                ));
            }
            Ok(())
        }
    }
}

/// Task-held receipts share the PM namespace. Writers call under dispatch; authenticated
/// lookup callers can also use this to replay before checking current target liveness.
pub(crate) fn pm_task_receipt(
    store: &Store,
    origin: &RequestOrigin,
) -> Result<Option<(String, RequestReceipt)>> {
    let found = store.pm_request_receipt(&origin.request_id)?;
    if let Some((_, stored)) = &found {
        admission::same_request(stored, &origin.op, &origin.payload_sha256)?;
    }
    Ok(found)
}

/// What a swept operation commits with its receipt. `owed` names post-commit effects:
/// `effect:event` logs `result.event`; `run_effect` lists the others.
pub struct Commit {
    pub result: Value,
    pub effect: Option<(&'static str, String)>,
    pub owed: Vec<&'static str>,
}

/// The last statement of a swept operation's final closure: appends the committed receipt.
pub fn record(
    task: &mut Task,
    origin: &RequestOrigin,
    home: &Home,
    commit: Commit,
) -> Result<RequestReceipt> {
    let effect = commit
        .effect
        .and_then(|(kind, reference)| effect(kind, &reference));
    let mut receipt = new_receipt(origin, ReceiptState::Committed, commit.result, effect, None)?;
    receipt.post_pending = commit.owed.into_iter().map(String::from).collect();
    push(receipts_mut(task, home)?, receipt.clone())?;
    Ok(receipt)
}

/// `{kind, detail}` for the owed `effect:event`, tagged with the request ID readers count once.
pub fn event(kind: &str, mut detail: Value, origin: &RequestOrigin) -> Value {
    detail["request_id"] = json!(origin.request_id);
    json!({"kind": kind, "detail": detail})
}

/// done and verdict: the request plus what its AgentReport or VerdictReport carries.
pub struct Report<'a> {
    pub origin: &'a RequestOrigin,
    pub gate_ref: Option<AgentGateRef>,
    /// Audit only; never an admission input.
    pub contract_sha256: Option<String>,
}

impl Report<'_> {
    /// A gate_ref must name the run's current receipt exactly as `corral show --json` does.
    pub fn check_gate_ref(&self, ctx: &Ctx, task: &Task) -> Result<()> {
        let Some(claimed) = &self.gate_ref else {
            return Ok(());
        };
        let current = crate::gate_tier::gate_readback(ctx, task)?.gate_ref;
        ensure!(
            current.is_some_and(|c| c.attempt == claimed.attempt
                && c.tier == claimed.tier
                && c.receipt_sha256 == claimed.receipt_sha256),
            "gate_ref_stale: {} is not the run's current gate receipt",
            claimed.attempt
        );
        Ok(())
    }

    /// Commits the receipt with its report; a verdict also marks the binding closing.
    pub fn record(
        &self,
        task: &mut Task,
        kind: AgentReportKind,
        verdict: Option<Verdict>,
        commit: Commit,
    ) -> Result<RequestReceipt> {
        let receipt = record(task, self.origin, &Home::of(self.origin), commit)?;
        let state = state_mut(task, &self.origin.binding.container)?;
        if state.reports.len() >= REPORTS_MAX {
            state.reports.remove(0);
        }
        state.reports.push(AgentReport {
            version: AGENT_RECORD_VERSION,
            kind,
            request_id: self.origin.request_id.clone(),
            generation: self.origin.binding.generation,
            turn: self.origin.identity.turn,
            gate_ref: self.gate_ref.clone(),
            contract_sha256: self.contract_sha256.clone(),
            verdict,
            at: receipt.at,
        });
        if kind == AgentReportKind::Verdict {
            state.binding.closing = true;
        }
        Ok(receipt)
    }
}

/// A swept operation's answer: its receipt, committed now or replayed.
#[derive(Debug, Clone, PartialEq)]
pub struct Receipted {
    pub receipt: RequestReceipt,
    pub replayed: bool,
}

/// Runs one swept operation for `origin`. A request that already committed answers with its
/// stored receipt before the operation's own checks run; otherwise `op` commits the receipt in
/// its final transaction. Owed post-commit effects then complete before the answer.
pub fn swept(
    ctx: &Ctx,
    origin: &RequestOrigin,
    home: Home,
    op: impl FnOnce() -> Result<Option<RequestReceipt>>,
) -> Result<Receipted> {
    let task = ctx
        .store
        .head(home.task().context("swept receipts live in a task")?)?;
    let stored = match &home {
        Home::Container(container) => task.agent_receipt(container, &origin.request_id)?,
        Home::PmTask(_) => task.pm_receipt(&origin.request_id)?,
    };
    if let Some(stored) = stored {
        admission::same_request(&stored, &origin.op, &origin.payload_sha256)?;
        return finish(ctx, &home, stored, true);
    }
    match op() {
        Ok(Some(receipt)) => finish(ctx, &home, receipt, false),
        Ok(None) => bail!("{} committed no receipt", origin.op),
        Err(error) => match error.downcast::<Replayed>() {
            Ok(Replayed(stored)) => finish(ctx, &home, *stored, true),
            Err(error) => Err(error),
        },
    }
}

fn finish(
    ctx: &Ctx,
    home: &Home,
    mut receipt: RequestReceipt,
    replayed: bool,
) -> Result<Receipted> {
    failpoint("swept_after_commit");
    complete(ctx, home, &mut receipt)?;
    failpoint("swept_before_ack");
    Ok(Receipted { receipt, replayed })
}

/// Runs the effects a receipt still owes and clears only the ones that completed. An effect
/// whose exact target cannot be served now, or that failed, stays owed for replay or sync; a
/// crash before the clearing write repeats completed ones, and every event line carries the
/// request ID.
fn complete(ctx: &Ctx, home: &Home, receipt: &mut RequestReceipt) -> Result<()> {
    let owed: Vec<String> = receipt
        .post_pending
        .iter()
        .filter(|entry| entry.starts_with("effect:"))
        .cloned()
        .collect();
    if owed.is_empty() {
        return Ok(());
    }
    let task_id = home.task().context("swept receipts live in a task")?;
    let mut done = Vec::new();
    for entry in owed {
        match run_effect(ctx, task_id, home, receipt, &entry) {
            Ok(true) => done.push(entry),
            Ok(false) => {}
            Err(error) => crate::messages::report(
                Some(task_id),
                "agent effect",
                format!("{entry} of {} stays owed: {error:#}", receipt.request_id),
            ),
        }
    }
    failpoint("swept_after_post");
    if done.is_empty() {
        return Ok(());
    }
    ctx.store.try_update_head(task_id, |task| {
        let clear = |stored: &mut RequestReceipt| {
            stored.post_pending.retain(|entry| !done.contains(entry));
            Ok(())
        };
        match home {
            Home::Container(container) => {
                let Some(mut stored) = task.agent_receipt(container, &receipt.request_id)? else {
                    return Ok(false);
                };
                clear(&mut stored)?;
                task.put_agent_record(container, agent::NativeRecord::Receipt(stored))
            }
            Home::PmTask(_) => task.edit_pm_receipt(&receipt.request_id, clear),
        }
    })?;
    receipt.post_pending.retain(|entry| !done.contains(entry));
    Ok(())
}

/// One owed effect, served only from the receipt's named run or review child; never the
/// current one in its place. False keeps the effect owed.
fn run_effect(
    ctx: &Ctx,
    task_id: &str,
    home: &Home,
    receipt: &RequestReceipt,
    entry: &str,
) -> Result<bool> {
    let task = ctx.store.head(task_id)?;
    let run = receipt.identity.run.as_deref();
    let named = run.map(|id| task.run_head(id)).transpose()?.flatten();
    match entry {
        "effect:event" => {
            let event = receipt
                .result
                .get("event")
                .context("owed event detail missing")?;
            let kind = event["kind"].as_str().context("owed event kind missing")?;
            let mut detail = event["detail"].clone();
            let container = match home {
                Home::Container(container) if kind == "verdict" => container,
                _ => {
                    ctx.store.log_event(Some(task_id), kind, detail)?;
                    return Ok(true);
                }
            };
            let child = match container {
                ContainerRef::Worker { run, .. } => task.run_head(run)?.and_then(|r| r.review),
                ContainerRef::Reviewer { run, review, .. } => {
                    task.review_child(run, review)?.and_then(|r| r.review)
                }
                _ => None,
            };
            let review = child.or_else(|| named.as_ref().and_then(|r| r.review.clone()));
            let notes = review.and_then(|r| r.notes);
            ctx.store.try_update_head(task_id, |_| {
                detail["notes"] = notes
                    .as_deref()
                    .map(|s| ctx.store.event_text(s))
                    .transpose()?
                    .unwrap_or(serde_json::Value::Null);
                ctx.store.log_event(Some(task_id), kind, detail)?;
                Ok(true)
            })?;
            Ok(true)
        }
        "effect:reconcile_wait" => {
            if task.current_run.as_ref().map(|r| r.id.as_str()) != run {
                return Ok(false);
            }
            crate::obligations_sync::reconcile_one(ctx, task_id, corral_core::clock::now(), None)?;
            Ok(true)
        }
        "effect:done_audit" => match named.as_ref().map(|r| (r, &r.completion.audit)) {
            Some((run, Some(audit))) => {
                crate::review::audit_artifact(ctx, task_id, &run.id, audit, true)?;
                Ok(true)
            }
            Some((_, None)) => Ok(true),
            None => Ok(false),
        },
        "effect:capture_review" => match home {
            Home::Container(ContainerRef::Reviewer { run, review, .. }) => {
                crate::watch::capture_review_exact(ctx, &task, run, review)
            }
            _ => Ok(false),
        },
        "effect:content_index" => {
            let path = receipt.result["path"]
                .as_str()
                .context("owed artifact path missing")?;
            let artifact = task
                .latest_artifact_by_path(path)?
                .context("owed artifact registration missing")?;
            // The same incremental attachment path as non-native artifacts. The normal
            // change-feed refresh still owns other documents/events and cold reconstruction.
            crate::content_index::index_artifact(ctx, task_id, &artifact)?;
            Ok(true)
        }
        other => bail!("unknown owed effect {other}"),
    }
}

/// Pending (open or escalated) questions of a container that is still open (F4, OD6).
pub fn open_questions(task: &Task, container: &ContainerRef) -> Vec<String> {
    if agent::container_closure(task, container).is_some() {
        return Vec::new();
    }
    agent::container_state(task, container).map_or_else(Vec::new, |state| {
        let pending = state.questions.iter().filter(|q| q.recorded_pending());
        pending.map(|q| q.key.clone()).collect()
    })
}

/// Refuses with `code` and the pending keys; done and verdict call it before their
/// pre-commit effects and again inside their final update.
pub fn refuse_pending(task: &Task, container: &ContainerRef, code: &str) -> Result<()> {
    let keys = open_questions(task, container);
    ensure!(
        keys.is_empty(),
        "{code}: {} pending ({}); wait for the answers or withdraw them",
        keys.len(),
        keys.join(", ")
    );
    Ok(())
}

fn subscribe(
    state: &mut AgentRunState,
    request: &str,
    kind: WakeKind,
    target: &str,
    at: WakeState,
) {
    if state
        .wakes
        .iter()
        .any(|w| w.request_id == request && w.target == target)
    {
        return;
    }
    while state.wakes.len() >= WAKES_MAX {
        let settled = state
            .wakes
            .iter()
            .position(|w| matches!(w.state, WakeState::Fired | WakeState::Cancelled));
        state.wakes.remove(settled.unwrap_or(0));
    }
    state.wakes.push(WakeSubscription {
        key: target.into(),
        request_id: request.into(),
        kind,
        target: target.into(),
        state: at,
        at: corral_core::clock::now(),
        fired_at: None,
        message_id: None,
    });
}

/// Arm the same durable wake when a native worker registers through the CLI.
pub(crate) fn subscribe_cli_wait(run: &mut corral_core::Run, wait: &str) {
    if let Some(state) = run.agent_host.as_deref_mut() {
        subscribe(
            state,
            &format!("cli-wait:{wait}"),
            WakeKind::Wait,
            wait,
            WakeState::Armed,
        );
    }
}

/// Writes a wait's subscription in the wait's own transaction (F5).
pub fn subscribe_wait(task: &mut Task, origin: &RequestOrigin, wait: &str) -> Result<()> {
    let state = state_mut(task, &origin.binding.container)?;
    subscribe(
        state,
        &origin.request_id,
        WakeKind::Wait,
        wait,
        WakeState::Armed,
    );
    Ok(())
}

fn worker(container: &ContainerRef) -> Result<(String, String)> {
    match container {
        ContainerRef::Worker { task, run } => Ok((task.clone(), run.clone())),
        _ => Err(reject(
            RejectCode::OpNotAllowed,
            "only a worker starts a gate",
        )),
    }
}

fn take_over(
    ctx: &Ctx,
    origin: &RequestOrigin,
    seen: Option<&Executor>,
    me: &Executor,
) -> Result<bool> {
    let (task_id, _) = worker(&origin.binding.container)?;
    let taken = ctx.store.try_update(&task_id, |task| {
        admission::recheck_task(task, &origin.binding)?;
        let state = state_mut(task, &origin.binding.container)?;
        let Some(stored) = state.receipts.iter_mut().find(|r| {
            r.request_id == origin.request_id
                && r.state == ReceiptState::Reserved
                && r.executor.as_ref() == seen
        }) else {
            return Ok(false);
        };
        stored.executor = Some(me.clone());
        Ok(true)
    })?;
    Ok(taken.is_some())
}

/// corral_gate (F1.a). A new request commits a reservation naming its attempt, its executor
/// and a `pending_start` wake subscription, then launches through `gate_start_request`, whose
/// one fence sits in `ReviewLaunch::submit`; the receipt is settled from attempt evidence. A
/// replay reconciles first and re-executes only a Prepared attempt under a live binding.
pub fn gate(ctx: &Ctx, origin: &RequestOrigin) -> Result<CallResponse> {
    let container = origin.binding.container.clone();
    let (task_id, run_id) = worker(&container)?;
    failpoint("gate_before_reservation");
    let me = executor()?;
    let retained = crate::review_budget::adoptable_attempt(ctx, &task_id, &run_id)?;
    let attempt = retained
        .clone()
        .unwrap_or_else(|| own_attempt(&origin.request_id));
    let mut found = None;
    ctx.store.try_update(&task_id, |task| {
        let state = agent::container_state(task, &container).ok_or_else(unbound)?;
        if let Some(stored) = state.receipt(&origin.request_id) {
            admission::same_request(stored, &origin.op, &origin.payload_sha256)?;
            found = Some(stored.clone());
            return Ok(false);
        }
        if let Some(queued) = state.receipts.iter().find(|r| {
            r.op == "gate" && r.state == ReceiptState::Queued
                && r.identity.generation == origin.identity.generation
        }) {
            return Err(reject(RejectCode::BadRequest,
                format!("gate_queued: request {} already awaits admission; end your turn and wait for its result", queued.request_id)));
        }
        admission::recheck_task(task, &origin.binding)?;
        let result =
            json!({"outcome": "reserved", "attempt": attempt, "adopted": retained.is_some()});
        let effect = effect("gate_attempt", &attempt);
        let receipt = new_receipt(
            origin,
            ReceiptState::Reserved,
            result,
            effect,
            Some(me.clone()),
        )?;
        let state = state_mut(task, &container)?;
        push(&mut state.receipts, receipt)?;
        subscribe(
            state,
            &origin.request_id,
            WakeKind::Gate,
            &attempt,
            WakeState::PendingStart,
        );
        Ok(true)
    })?;
    let Some(stored) = found else {
        failpoint("gate_reserved_before_authorization");
        return super::gate_queue::execute(ctx, origin, me, false);
    };
    let (settled, idle) = match stored.state {
        ReceiptState::Reserved => match &stored.executor {
            Some(running) if !executor_gone(running)? => (stored, false),
            seen => (
                crate::review_budget::settle(ctx, &stored, seen.as_ref(), false, None)?,
                true,
            ),
        },
        ReceiptState::Uncertain => (
            crate::review_budget::settle(ctx, &stored, None, false, None)?,
            false,
        ),
        _ => (stored, false),
    };
    let seen = settled.executor.as_ref();
    if idle && settled.state == ReceiptState::Reserved && take_over(ctx, origin, seen, &me)? {
        return super::gate_queue::execute(ctx, origin, me, true);
    }
    super::receipts::respond(ctx, &container, settled, true)
}

mod reconcile;
pub use reconcile::reconcile_task;
pub(crate) use reconcile::reconcile_task_hot;

/// The PM two-phase journal (capture, legacy reply): a reservation naming its executor commits
/// in AgentPm before the effect runs. An identical replay returns the stored receipt.
pub fn pm_reserve(
    store: &Store,
    origin: &RequestOrigin,
    kind: &str,
    target: &str,
) -> Result<Receipted> {
    let me = executor()?;
    pm_state::update(store, |pm| {
        let pm = pm
            .as_mut()
            .ok_or_else(|| reject(RejectCode::ContainerNotFound, "no corral-pi PM is bound"))?;
        if let Some(stored) = pm.state.receipt(&origin.request_id) {
            admission::same_request(stored, &origin.op, &origin.payload_sha256)?;
            let receipt = stored.clone();
            return Ok((
                Receipted {
                    receipt,
                    replayed: true,
                },
                false,
            ));
        }
        if let Some((_, receipt)) = pm_task_receipt(store, origin)? {
            return Ok((
                Receipted {
                    receipt,
                    replayed: true,
                },
                false,
            ));
        }
        admission::recheck_pm(pm, origin.binding.generation)?;
        let result = json!({"outcome": "reserved", "kind": kind, "target": target});
        let receipt = new_receipt(
            origin,
            ReceiptState::Reserved,
            result,
            effect(kind, target),
            Some(me),
        )?;
        push(&mut pm.state.receipts, receipt.clone())?;
        Ok((
            Receipted {
                receipt,
                replayed: false,
            },
            true,
        ))
    })
}

fn pm_write(
    store: &Store,
    request: &str,
    seen: Option<&Executor>,
    to: (ReceiptState, Value),
    source: &str,
) -> Result<RequestReceipt> {
    pm_state::update(store, |pm| {
        let pm = pm.as_mut().context("no corral-pi PM is bound")?;
        let stored = pm
            .state
            .receipts
            .iter_mut()
            .find(|r| r.request_id == request);
        let stored = stored.context("PM reservation disappeared")?;
        if stored.state != ReceiptState::Reserved || stored.executor.as_ref() != seen {
            return Ok((stored.clone(), false));
        }
        crate::review_budget::evidence(stored, &to.1, source, None);
        (stored.state, stored.result) = to;
        stored.executor = None;
        let now = corral_core::clock::now();
        stored.committed_at = (stored.state == ReceiptState::Committed).then_some(now);
        Ok((stored.clone(), true))
    })
}

/// The executor settles its own PM reservation after the effect.
pub fn pm_settle(
    store: &Store,
    reserved: &RequestReceipt,
    state: ReceiptState,
    result: Value,
) -> Result<RequestReceipt> {
    pm_write(
        store,
        &reserved.request_id,
        reserved.executor.as_ref(),
        (state, result),
        "executor",
    )
}

/// Reconciles a PM reservation once its executor is verified gone, from its own marker: a
/// capture commits when a task carries the request's `agent_origin` and is abandoned when none
/// does (creation and marker are one write); a legacy pane reply has no marker and stays
/// uncertain. While the executor lives the reservation is unchanged. Nothing re-executes.
pub fn reconcile_pm(ctx: &Ctx, request: &str) -> Result<RequestReceipt> {
    let pm = pm_state::load(&ctx.store)?.context("no corral-pi PM is bound")?;
    let receipt = pm
        .state
        .receipt(request)
        .cloned()
        .context("unknown PM request")?;
    if receipt.state != ReceiptState::Reserved {
        return Ok(receipt);
    }
    if let Some(running) = &receipt.executor
        && !executor_gone(running)?
    {
        return Ok(receipt);
    }
    let kind = receipt.effect.as_ref().map(|e| e.kind.as_str());
    let to = if kind == Some("capture") {
        let marked = ctx
            .store
            .heads_page(
                &corral_core::backend::HeadQuery::EdgeTo {
                    kind: "agent_origin".into(),
                    other: request.to_owned(),
                },
                1,
            )?
            .into_iter()
            .next();
        match marked {
            Some(task) => (
                ReceiptState::Committed,
                json!({"outcome": "captured", "task": task.id}),
            ),
            None => (ReceiptState::Abandoned, json!({"outcome": "not_started"})),
        }
    } else if kind == Some("pm_inbox") {
        let item = receipt.effect.as_ref().map_or("", |e| e.reference.as_str());
        crate::pm_inbox::reconciled(ctx, request, item)?
    } else {
        let reason = "a legacy pane send leaves no marker";
        (
            ReceiptState::Uncertain,
            json!({"outcome": "outcome_unknown", "reason": reason}),
        )
    };
    pm_write(
        &ctx.store,
        request,
        receipt.executor.as_ref(),
        to,
        "reconcile",
    )
}
