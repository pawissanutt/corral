//! Host loss, `corral agent cancel` and `resume [--replace]` (T-252 recovery, F1.b). Resume
//! verifies the lost generation's host dead (recorded pid and start time), fences it in one
//! commit, waits up to `drain_secs` for that generation's reservation executors, reconciles,
//! classifies every dangling call of its bound Pi session file from receipts and attempt
//! evidence, opens generation n+1 in the same pane and posts one `recovery_notice`. Nothing is
//! replayed and Pi's session file is never rewritten. A call whose executor outlives the drain
//! is reported `in_progress`; sync posts its one `recovery_update` once it settles.
//! Owner: T-252 child recovery.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use corral_core::agent::{
    self, AgentRunState, ContainerRef, Executor, HostHealth, InterruptedCall, ReceiptState,
    RequestReceipt, ResumeClass, pm_state,
};
use corral_core::inbox::{Inbox, Message, MessageBinding, Post, Recipient, Urgency};
use corral_core::{ArtifactKind, Task};
use serde_json::{Value, json};

use super::admission::{BindTarget, fence_generation};
use super::bridge::BridgeClient;
use super::bridge::cancel as cancels;
use super::bridge::interrupted::{self, MUTATING, ScanLimit};
use super::bridge::liveness::{bound_host, gone};
use super::bridge::notice::{self, Unconfirmed};
use super::cli::{AgentTarget, CancelArgs, ResumeArgs};
use super::effects;
use super::launch::{CTL_CAP_FILE, HOST_ENTRY, HostSpec, SOCKET_FILE, generation_dir, prepare};
use crate::Ctx;

/// The T-252 child that lands recovery.
pub const OWNER: &str = "recovery";
pub const NOTICE_KIND: &str = "recovery_notice";
pub const UPDATE_KIND: &str = "recovery_update";

#[cfg(all(test, target_os = "linux"))]
fn receipt(
    host: &super::test_support::Host,
    call: &str,
    op: &str,
    state: &str,
    result: Value,
) -> RequestReceipt {
    serde_json::from_value(json!({
        "version": 1, "request_id": super::tool_request_id(super::bridge::fixture::SESSION, call),
        "op": op, "payload_sha256": "00", "identity": host.identity, "state": state,
        "result": result, "at": "2026-10-25T00:00:00Z",
    }))
    .unwrap()
}
/// Evidence sources on a receipt reported `in_progress`, and on its one later update.
const NOTICE_SOURCE: &str = "recovery_notice";
const UPDATE_SOURCE: &str = "recovery_update";
const REPLACE_TERM: Duration = Duration::from_secs(10);
const REPLACE_KILL: Duration = Duration::from_secs(5);

fn recipient(container: &ContainerRef) -> Recipient {
    match container {
        ContainerRef::Worker { task, run } => Recipient::Run {
            task: task.clone(),
            run: run.clone(),
        },
        ContainerRef::Reviewer { task, run, review } => Recipient::ReviewRun {
            task: task.clone(),
            run: run.clone(),
            review: review.clone(),
        },
        ContainerRef::Pm => Recipient::Pm,
    }
}

/// The container a command names: the task's current run unless `--run` names another.
pub fn resolve(ctx: &Ctx, target: &AgentTarget) -> Result<ContainerRef> {
    let AgentTarget::Task { task, run, review } = target else {
        return Ok(ContainerRef::Pm);
    };
    let run = match run {
        Some(run) => run.clone(),
        None => {
            let record = ctx.store.get(task)?;
            let current = record.current_run();
            current
                .with_context(|| format!("{task} has no run"))?
                .id
                .clone()
        }
    };
    let task = task.clone();
    Ok(match review {
        Some(review) => ContainerRef::Reviewer {
            task,
            run,
            review: review.clone(),
        },
        None => ContainerRef::Worker { task, run },
    })
}

fn snapshot(ctx: &Ctx, container: &ContainerRef) -> Result<(AgentRunState, Option<Task>)> {
    let Some(id) = container.task() else {
        let pm = pm_state::load(&ctx.store)?.context("no Pi-native PM is bound")?;
        return Ok((pm.state, None));
    };
    let task = ctx.store.get(id)?;
    let state = agent::container_state(&task, container)
        .cloned()
        .with_context(|| format!("{} has no Pi-native binding", container.key()))?;
    Ok((state, Some(task)))
}

/// An open container's binding: a closed run or review child is never cancelled or resumed.
fn open(ctx: &Ctx, container: &ContainerRef) -> Result<(AgentRunState, Option<Task>)> {
    let (state, task) = snapshot(ctx, container)?;
    if let Some(reason) = task
        .as_ref()
        .and_then(|t| agent::container_closure(t, container))
    {
        bail!(
            "container_closed: {} is closed ({}); a closed run or review child is never \
             resumed",
            container.key(),
            reason.as_str()
        );
    }
    Ok((state, task))
}

fn health(state: &AgentRunState) -> &'static str {
    match state.binding.health {
        HostHealth::Unbound => "unbound",
        HostHealth::Healthy => "healthy",
        HostHealth::Lost => "lost",
        HostHealth::Fenced => "fenced",
    }
}

/// Cancels a turn of a live host. It never fences: the run, its questions and its receipts stay
/// live, and an in-flight call keeps its own truthful receipt.
pub fn cancel(ctx: &Ctx, args: &CancelArgs) -> Result<()> {
    let container = resolve(ctx, &args.target)?;
    let (state, _) = open(ctx, &container)?;
    let n = state.binding.generation;
    ensure!(
        state.binding.health == HostHealth::Healthy,
        "host_{}: generation {n} of {} cannot take a cancel; `corral agent resume` starts the \
         next generation",
        health(&state),
        container.key()
    );
    let reply = BridgeClient::connect(&ctx.store, &recipient(&container), n)?.cancel(args.turn)?;
    // The host names the turn it cancelled. A turn not recorded yet classifies `cancelled` from
    // its own `turn_ended` report, whenever that commits.
    if reply["cancelled"] == true
        && let Some(seq) = reply["turn"].as_u64()
    {
        cancels::confirm(ctx, &container, n, seq)?;
    }
    let detail = json!({
        "container": container.key(), "generation": n, "turn": reply["turn"],
        "cancelled": reply["cancelled"], "reason": reply["reason"],
    });
    println!("{}", serde_json::to_string(&detail)?);
    Ok(())
}

pub fn resume(ctx: &Ctx, args: &ResumeArgs) -> Result<()> {
    let container = resolve(ctx, &args.target)?;
    let outcome = resume_container(ctx, &container, args.replace)?;
    println!("{}", serde_json::to_string_pretty(&outcome)?);
    Ok(())
}

/// What the relaunch of a container needs, read before anything is fenced.
struct Relaunch {
    pane: String,
    target: Option<(BindTarget, HostSpec)>,
    exit_receipt: Option<PathBuf>,
    worktree: PathBuf,
    artifact_run: Option<String>,
}

fn relaunch(ctx: &Ctx, container: &ContainerRef, state: &AgentRunState) -> Result<Relaunch> {
    let generation = state.current_generation();
    let launched = generation.map(|g| g.pane.clone());
    let Some(task) = container.task() else {
        let pm = pm_state::load(&ctx.store)?.context("no Pi-native PM is bound")?;
        return Ok(Relaunch {
            pane: launched.unwrap_or(pm.pane),
            target: None,
            exit_receipt: None,
            worktree: ctx.store.root().to_path_buf(),
            artifact_run: None,
        });
    };
    let record = ctx.store.get(task)?;
    let run = agent::container_run(&record, container)
        .with_context(|| format!("{} does not exist", container.key()))?;
    let pane = launched
        .or_else(|| run.herdr.as_ref().map(|h| h.pane_id.clone()))
        .with_context(|| format!("{} has no recorded pane", container.key()))?;
    let model = run
        .model
        .clone()
        .with_context(|| format!("{} records no model", container.key()))?;
    let (target, prompt, receipt, parent) = match container {
        ContainerRef::Worker { task, run } => (
            BindTarget::Worker {
                task: task.clone(),
                run: run.clone(),
            },
            super::hooks::surfaces::prompt_file(ctx, task, run),
            crate::launch_exit::receipt(ctx, task, run),
            run.clone(),
        ),
        ContainerRef::Reviewer { task, run, review } => (
            BindTarget::Reviewer {
                task: task.clone(),
                run: run.clone(),
                review: review.clone(),
                callback: state
                    .binding
                    .callback
                    .clone()
                    .context("the review child's binding has no callback token")?,
            },
            super::hooks::review::review_prompt_file(ctx, task, run),
            crate::launch_exit::receipt(ctx, task, &format!("{run}-{review}")),
            run.clone(),
        ),
        ContainerRef::Pm => unreachable!("the PM has no task"),
    };
    let spec = HostSpec {
        cwd: run.worktree.clone(),
        provider: super::launch::host_provider(run.vendor.as_deref(), &model),
        model,
        effort: run.effort.clone(),
        prompt_file: Some(prompt),
    };
    Ok(Relaunch {
        pane,
        target: Some((target, spec)),
        exit_receipt: Some(receipt),
        worktree: run.worktree.clone(),
        artifact_run: Some(parent),
    })
}

pub fn resume_container(ctx: &Ctx, container: &ContainerRef, replace: bool) -> Result<Value> {
    let (state, _) = open(ctx, container)?;
    let n = state.binding.generation;
    ensure!(n > 0, "{} has no generation to resume", container.key());
    // Refuse unavailable assets, profiles or scope without stopping or fencing the old host.
    let pm_profile = if matches!(container, ContainerRef::Pm) {
        let resolved = super::hooks::pm::profile(ctx)?;
        ensure!(
            super::hooks::pm::project(ctx)? == state.binding.project,
            "out_of_scope: the approved PM project differs from the existing binding"
        );
        Some(resolved)
    } else {
        None
    };
    let plan = relaunch(ctx, container, &state)?;
    if ctx.herdr.pane_get(&plan.pane)?.is_none() {
        bail!(
            "pane_gone: {} is gone; retry or discard the run (a review child whose pane is gone \
             is closed by the existing process_lost path and retried)",
            plan.pane
        );
    }
    let host = bound_host(&state);
    let mut replaced = false;
    if let Some(host) = &host
        && !gone(host)?
    {
        ensure!(
            replace,
            "host_alive: generation {n}'s host (pid {}) is still running; `corral agent resume \
             --replace` stops it first",
            host.pid
        );
        stop_host(host)?;
        replaced = true;
    }
    fence_generation(&ctx.store, container, n, "resume")?;
    remove_control(ctx, container, n)?;
    effects::failpoint("resume_fenced");
    let deadline = Instant::now() + Duration::from_secs(ctx.store.config.agent.drain_secs);
    let draining = drain(ctx, container, n, deadline)?;
    reconcile(ctx, container, n)?;
    let interrupted = classify_lost(ctx, container, n, &plan)?;
    let unconfirmed = settle_unconfirmed(ctx, container, n, &state)?;
    let command = match (&plan.target, pm_profile) {
        (Some((target, spec)), _) => prepare(ctx, target, &plan.pane, spec)?,
        (None, Some(resolved)) => super::hooks::pm::command(ctx, &plan.pane, &resolved)?,
        (None, None) => bail!("the PM relaunch command is missing"),
    };
    let command = match &plan.exit_receipt {
        Some(receipt) => crate::launch_exit::arm(&command, receipt)?,
        None => command,
    };
    let next = snapshot(ctx, container)?.0.binding.generation;
    let notice = post_notice(ctx, container, (n, next), &interrupted, &unconfirmed, &plan)?;
    for call in interrupted.calls.iter().filter(|c| c.request_id.is_some()) {
        let class = super::bridge::interrupted::class_name(call.class);
        ctx.store.log_event(
            container.task(),
            "agent_call_reconciled",
            json!({"container": container.key(), "generation": n, "request_id": call.request_id,
                "class": class, "notice": notice.id}),
        )?;
    }
    // A reviewer's hello is admitted only in Review: release its loss hold before it launches.
    if let ContainerRef::Reviewer { task, .. } = container {
        super::hooks::review::release_lost(ctx, task)?;
    }
    ctx.herdr.pane_run(&plan.pane, &command)?;
    if let Some(task) = container.task() {
        super::hooks::sync::settle_lost(ctx, task)?;
    }
    let outcome = json!({
        "container": container.key(), "fenced": n, "generation": next, "pane": plan.pane,
        "replaced": replaced, "draining": draining, "notice": notice.id,
        "calls": interrupted.calls, "unconfirmed": unconfirmed_json(&unconfirmed),
    });
    ctx.store.log_event(
        container.task(),
        "agent_resumed",
        json!({
            "container": container.key(), "fenced": n, "generation": next,
            "notice": notice.id, "calls": interrupted.calls.len(), "replaced": replaced,
        }),
    )?;
    Ok(outcome)
}

/// Fencing removes the generation's control credential and socket: nobody reaches it again.
fn remove_control(ctx: &Ctx, container: &ContainerRef, n: u32) -> Result<()> {
    let dir = generation_dir(&ctx.store, &recipient(container), n)?;
    for name in [CTL_CAP_FILE, SOCKET_FILE] {
        let path = dir.join(name);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("removing {}", path.display()));
            }
        }
    }
    Ok(())
}

fn command_line(pid: u32) -> Result<Vec<String>> {
    let path = format!("/proc/{pid}/cmdline");
    let bytes = std::fs::read(&path).with_context(|| format!("reading {path}"))?;
    let parts = bytes.split(|b| *b == 0).filter(|p| !p.is_empty());
    Ok(parts
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect())
}

/// `--replace`: signals only the recorded host, after its start time and command line prove it
/// is the Bun host this generation launched (`src/host/main.ts --launch-fd`).
#[cfg(target_os = "linux")]
fn stop_host(host: &Executor) -> Result<()> {
    use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};
    let pid = Pid::from_raw(i32::try_from(host.pid)?).context("the host pid is zero")?;
    let fd = match pidfd_open(pid, PidfdFlags::empty()) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::SRCH) => return Ok(()),
        Err(error) => return Err(error).context("opening the host's pidfd"),
    };
    if gone(host)? {
        return Ok(());
    }
    let argv = command_line(host.pid)?;
    let entry = argv.iter().any(|a| a.ends_with(HOST_ENTRY));
    let launched = argv.iter().any(|a| a == "--launch-fd");
    ensure!(
        entry && launched,
        "host_identity_unverified: pid {} runs {:?}, not the Pi host {HOST_ENTRY} --launch-fd; \
         nothing was signalled",
        host.pid,
        argv
    );
    for (signal, wait) in [(Signal::TERM, REPLACE_TERM), (Signal::KILL, REPLACE_KILL)] {
        match pidfd_send_signal(&fd, signal) {
            Ok(()) => {}
            Err(rustix::io::Errno::SRCH) => return Ok(()),
            Err(error) => return Err(error).context("signalling the host"),
        }
        if wait_gone(host, Instant::now() + wait)? {
            return Ok(());
        }
    }
    bail!("host_alive: pid {} survived SIGTERM and SIGKILL", host.pid)
}

#[cfg(not(target_os = "linux"))]
fn stop_host(host: &Executor) -> Result<()> {
    bail!(
        "host_alive: replacing pid {} needs Linux pidfds; stop it and run resume again",
        host.pid
    )
}

/// Waits until no live process has the executor's pid and start time, or the deadline passes.
/// The pidfd pins the process, so a reused pid is never waited on.
#[cfg(target_os = "linux")]
pub(crate) fn wait_gone(executor: &Executor, deadline: Instant) -> Result<bool> {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    use rustix::process::{Pid, PidfdFlags, pidfd_open};
    let Some(pid) = Pid::from_raw(i32::try_from(executor.pid)?) else {
        return Ok(true);
    };
    let fd = match pidfd_open(pid, PidfdFlags::empty()) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::SRCH) => return Ok(true),
        Err(error) => return Err(error).context("opening an executor pidfd"),
    };
    if gone(executor)? {
        return Ok(true);
    }
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return gone(executor);
        }
        let timeout = Timespec {
            tv_sec: i64::try_from(left.as_secs())?,
            tv_nsec: i64::from(left.subsec_nanos()),
        };
        let mut fds = [PollFd::new(&fd, PollFlags::IN)];
        match poll(&mut fds, Some(&timeout)) {
            Ok(_) if gone(executor)? => return Ok(true),
            Ok(_) | Err(rustix::io::Errno::INTR) => {}
            Err(error) => return Err(error).context("waiting on an executor pidfd"),
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn wait_gone(executor: &Executor, deadline: Instant) -> Result<bool> {
    while !gone(executor)? {
        if Instant::now() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(true)
}

fn receipts(ctx: &Ctx, container: &ContainerRef) -> Result<Vec<RequestReceipt>> {
    let (state, _) = snapshot(ctx, container)?;
    let mut all = state.receipts;
    if *container == ContainerRef::Pm {
        for task in ctx.store.list()? {
            all.extend(task.agent_pm_receipts);
        }
    }
    Ok(all)
}

/// Waits for every live reservation executor of generation `n`; returns the requests whose
/// executor still runs at the deadline.
fn drain(ctx: &Ctx, container: &ContainerRef, n: u32, deadline: Instant) -> Result<Vec<String>> {
    let mut running = Vec::new();
    for receipt in receipts(ctx, container)? {
        let Some(executor) = receipt.executor.as_ref() else {
            continue;
        };
        let reserved = receipt.state == ReceiptState::Reserved;
        if reserved && receipt.identity.generation == n && !wait_gone(executor, deadline)? {
            running.push(receipt.request_id.clone());
        }
    }
    Ok(running)
}

/// Settles what the fence and the drain made settleable: gate reservations from their attempt
/// evidence, PM journal entries from their markers. Nothing re-executes.
fn reconcile(ctx: &Ctx, container: &ContainerRef, n: u32) -> Result<()> {
    if let Some(task) = container.task() {
        effects::reconcile_task(ctx, task)?;
        return Ok(());
    }
    let (state, _) = snapshot(ctx, container)?;
    let reserved = state.receipts.iter().filter(|r| {
        r.state == ReceiptState::Reserved && r.identity.generation == n && r.executor.is_some()
    });
    for receipt in reserved {
        effects::reconcile_pm(ctx, &receipt.request_id)?;
    }
    Ok(())
}

/// The classified dangling calls of a lost generation and the worktree snapshot they cite.
#[derive(Debug, Clone, PartialEq)]
pub struct Interrupted {
    pub calls: Vec<InterruptedCall>,
    /// Bounds that hid session calls or results; an empty list means a complete scan.
    pub scan_limits: Vec<ScanLimit>,
    /// Requests reported `in_progress`: sync posts one `recovery_update` for each.
    pub running: Vec<String>,
}

fn classify_lost(
    ctx: &Ctx,
    container: &ContainerRef,
    n: u32,
    plan: &Relaunch,
) -> Result<Interrupted> {
    let (state, _) = snapshot(ctx, container)?;
    let Some(generation) = state.generation(n) else {
        return Ok(Interrupted {
            calls: Vec::new(),
            scan_limits: Vec::new(),
            running: Vec::new(),
        });
    };
    let Some(bound) = &generation.bound else {
        return Ok(Interrupted {
            calls: Vec::new(),
            scan_limits: Vec::new(),
            running: Vec::new(),
        });
    };
    let dangling =
        interrupted::dangling_calls(&bound.session_file, generation.n, &bound.session_id)?;
    let all = receipts(ctx, container)?;
    let needs_snapshot = dangling
        .calls
        .iter()
        .any(|c| MUTATING.contains(&c.name.as_str()));
    let snapshot = match needs_snapshot {
        true => Some(worktree_snapshot(ctx, container, n, plan)?),
        false => None,
    };
    let (mut calls, mut running) = (Vec::new(), Vec::new());
    for call in dangling.calls {
        let Some(op) = call.name.strip_prefix("corral_") else {
            let (class, detail) = interrupted::classify_upstream(&call.name, snapshot.as_deref());
            calls.push(InterruptedCall {
                tool_call_id: call.id,
                tool: call.name,
                request_id: None,
                class,
                detail,
            });
            continue;
        };
        let request = super::tool_request_id(&bound.session_id, &call.id);
        let receipt = all.iter().find(|r| r.request_id == request);
        let alive = match receipt.and_then(|r| r.executor.as_ref()) {
            Some(executor) if receipt.is_some_and(|r| r.state == ReceiptState::Reserved) => {
                !gone(executor)?
            }
            _ => false,
        };
        let (class, detail) = interrupted::classify_corral(op, receipt, alive);
        if class == ResumeClass::InProgress {
            running.push(request.clone());
        }
        calls.push(InterruptedCall {
            tool_call_id: call.id,
            tool: call.name,
            request_id: Some(request),
            class,
            detail: Some(detail),
        });
    }
    Ok(Interrupted {
        calls,
        scan_limits: dangling.limits,
        running,
    })
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output();
    match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).into_owned(),
        Ok(out) => format!(
            "git {} exited {}: {}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(error) => format!("git {} could not run: {error}", args.join(" ")),
    }
}

/// `git status --porcelain=v2` and `git diff --stat` of the container's worktree, kept as a run
/// artifact (the PM's as a file): the evidence for every upstream call reported unknown.
fn worktree_snapshot(
    ctx: &Ctx,
    container: &ContainerRef,
    n: u32,
    plan: &Relaunch,
) -> Result<String> {
    let dir = ctx.store.state_dir().join("agent-recovery");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let name = container.key().replace('/', "-");
    let path = dir.join(format!("{name}-g{n}-worktree.txt"));
    let text = format!(
        "worktree {}\n\n$ git status --porcelain=v2\n{}\n$ git diff --stat\n{}",
        plan.worktree.display(),
        git(&plan.worktree, &["status", "--porcelain=v2"]),
        git(&plan.worktree, &["diff", "--stat"])
    );
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    let (Some(task), Some(run)) = (container.task(), plan.artifact_run.as_deref()) else {
        return Ok(path.display().to_string());
    };
    let title = format!(
        "Worktree after host loss ({} generation {n})",
        container.key()
    );
    let artifact = crate::artifacts::add_artifact_for_run(
        ctx,
        task,
        run,
        &path,
        Some(ArtifactKind::Evidence),
        Some(&title),
    )?;
    Ok(format!("artifact {}", artifact.path))
}

/// The envelope generation `n` was sent and never confirmed, settled without a resend so the
/// notice is published first.
fn settle_unconfirmed(
    ctx: &Ctx,
    container: &ContainerRef,
    n: u32,
    state: &AgentRunState,
) -> Result<Vec<Unconfirmed>> {
    let Some(bound) = state.generation(n).and_then(|g| g.bound.as_ref()) else {
        return Ok(Vec::new());
    };
    let to = recipient(container);
    let endpoint = generation_dir(&ctx.store, &to, n)?.join(SOCKET_FILE);
    notice::settle_unconfirmed(ctx, &to, &endpoint, &bound.session_file)
}

fn unconfirmed_json(unconfirmed: &[Unconfirmed]) -> Value {
    let each = |m: &Unconfirmed| json!({"id": m.id, "kind": m.kind, "observed": m.observed});
    Value::Array(unconfirmed.iter().map(each).collect())
}

/// Posts the generation's one notice; `in_progress` receipts are marked in the same commit, so
/// sync later posts exactly one update for each.
fn post_notice(
    ctx: &Ctx,
    container: &ContainerRef,
    (lost, next): (u32, u32),
    interrupted: &Interrupted,
    unconfirmed: &[Unconfirmed],
    plan: &Relaunch,
) -> Result<Message> {
    let inbox = Inbox::new(&ctx.store, recipient(container))?;
    let run = plan.artifact_run.as_deref();
    let keep = |text: &str| notice::keep_complete(ctx, container, lost, run, text);
    let calls = &interrupted.calls;
    let body = notice::body(inbox.key(), lost, next, interrupted, unconfirmed, keep)?;
    let key = format!("recovery:{}/g{lost}", container.key());
    let mut subject = json!({
        "key": key, "lost": lost, "generation": next, "calls": calls,
        "unconfirmed": unconfirmed_json(unconfirmed),
    });
    if !interrupted.scan_limits.is_empty() {
        subject["scan"] = json!({"complete": false, "limits": interrupted.scan_limits});
    }
    let post = || Post {
        kind: NOTICE_KIND,
        urgency: Urgency::Wake,
        author: "corral",
        body: &body,
        binding: binding(container),
        subject: Some(subject.clone()),
        max_payload: corral_core::inbox::MAX_PAYLOAD,
    };
    let Some(task_id) = container.task() else {
        return inbox.post(post());
    };
    let _posting = inbox.lock()?;
    let mut posted = None;
    ctx.store.try_update(task_id, |task| {
        let message = inbox.reserve_in(task, post())?;
        let state =
            agent::container_state_mut(task, container).context("the agent binding disappeared")?;
        for receipt in &mut state.receipts {
            if interrupted.running.contains(&receipt.request_id) {
                let mark = json!({"outcome": "in_progress"});
                crate::review_budget::evidence(
                    receipt,
                    &mark,
                    NOTICE_SOURCE,
                    Some(message.id.clone()),
                );
            }
        }
        posted = Some(message);
        Ok(true)
    })?;
    let message = posted.context("the recovery notice was not reserved")?;
    inbox.announce(&message)?;
    Ok(message)
}

fn binding(container: &ContainerRef) -> MessageBinding {
    let run = match container {
        ContainerRef::Worker { run, .. } => Some(run.clone()),
        ContainerRef::Reviewer { review, .. } => Some(review.clone()),
        ContainerRef::Pm => None,
    };
    MessageBinding {
        run,
        ..MessageBinding::default()
    }
}

fn update_due(receipt: &RequestReceipt) -> bool {
    let noticed = receipt
        .evidence
        .iter()
        .any(|e| e.source == NOTICE_SOURCE && e.state == "in_progress");
    let updated = receipt.evidence.iter().any(|e| e.source == UPDATE_SOURCE);
    noticed && !updated && receipt.state != ReceiptState::Reserved
}

/// Every pi-native container of the task with a receipt owed its one `recovery_update`.
fn due_updates(task: &Task) -> Vec<(ContainerRef, String)> {
    let mut due = Vec::new();
    for run in &task.runs {
        let worker = ContainerRef::Worker {
            task: task.id.clone(),
            run: run.id.clone(),
        };
        let children = run.review_runs.iter().map(|c| {
            let reviewer = ContainerRef::Reviewer {
                task: task.id.clone(),
                run: run.id.clone(),
                review: c.id.clone(),
            };
            (reviewer, c)
        });
        for (container, record) in std::iter::once((worker, run)).chain(children) {
            let Some(state) = record.agent_host.as_deref() else {
                continue;
            };
            for receipt in state.receipts.iter().filter(|r| update_due(r)) {
                due.push((container.clone(), receipt.request_id.clone()));
            }
        }
    }
    due
}

/// Sync: posts one `recovery_update` for each call a notice reported `in_progress` once its
/// receipt settled, and marks the receipt in the same commit. A closed container gets none.
pub fn post_updates(ctx: &Ctx, task_id: &str) -> Result<usize> {
    let due = due_updates(&ctx.store.get(task_id)?);
    let mut posted = 0;
    for (container, request) in due {
        let inbox = Inbox::new(&ctx.store, recipient(&container))?;
        let _posting = inbox.lock()?;
        let mut message = None;
        ctx.store.try_update(task_id, |task| {
            let closed = agent::container_closure(task, &container).is_some();
            let Some(receipt) = agent::container_state(task, &container)
                .and_then(|s| s.receipt(&request))
                .filter(|r| update_due(r))
                .cloned()
            else {
                return Ok(false);
            };
            let (class, detail) = interrupted::classify_corral(&receipt.op, Some(&receipt), false);
            let reserved = match closed {
                true => None,
                false => {
                    let body = interrupted::update_body(&receipt, class, &detail);
                    Some(inbox.reserve_in(
                        task,
                        Post {
                            kind: UPDATE_KIND,
                            urgency: Urgency::Wake,
                            author: "corral",
                            body: &body,
                            binding: binding(&container),
                            subject: Some(json!({
                                "key": format!("recovery-update:{}/{request}", container.key()),
                                "request_id": request, "class": class,
                            })),
                            max_payload: corral_core::inbox::MAX_PAYLOAD,
                        },
                    )?)
                }
            };
            let state = agent::container_state_mut(task, &container)
                .context("the agent binding disappeared")?;
            let stored = state
                .receipts
                .iter_mut()
                .find(|r| r.request_id == request)
                .context("the receipt disappeared")?;
            let outcome = if closed {
                "closed"
            } else {
                interrupted::class_name(class)
            };
            let mark = json!({ "outcome": outcome });
            crate::review_budget::evidence(
                stored,
                &mark,
                UPDATE_SOURCE,
                reserved.as_ref().map(|m| m.id.clone()),
            );
            message = reserved;
            Ok(true)
        })?;
        if let Some(message) = message {
            inbox.announce(&message)?;
            posted += 1;
        }
    }
    Ok(posted)
}

#[cfg(all(test, target_os = "linux"))]
#[path = "recovery_cancel_tests.rs"]
mod cancel_tests;
#[cfg(all(test, target_os = "linux"))]
#[path = "recovery_clock_tests.rs"]
mod clock_tests;
#[cfg(all(test, target_os = "linux"))]
#[path = "recovery_crash_tests.rs"]
mod crash_tests;
#[cfg(all(test, target_os = "linux"))]
#[path = "recovery_notice_tests.rs"]
mod notice_tests;
#[cfg(all(test, target_os = "linux"))]
#[path = "recovery_review_tests.rs"]
mod review_tests;
#[cfg(all(test, target_os = "linux"))]
#[path = "recovery_tests.rs"]
mod tests;
#[cfg(all(test, target_os = "linux"))]
#[path = "recovery_turn_tests.rs"]
mod turn_tests;
