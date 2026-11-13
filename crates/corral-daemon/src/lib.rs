//! The workspace's single synchronization loop owner.
mod classic;
#[cfg(unix)]
mod commands;
mod health;
mod projector;
mod reader;
#[cfg(unix)]
mod server;
mod state;

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use corral_core::{Actor, DaemonPass, Store};
use corral_ops::{
    Ctx, FullReason, PassMemory, PassReport, PassRequest, StepId, daemon_client,
    loop_pacing::{periodic_wait, process_cpu, quiet_wait},
};
use jiff::Timestamp;

use state::Shared;

/// While this file exists beside the daemon socket, passes are at least `audit` ([`DaemonPass`]).
const SYNC_AUDIT_SWITCH: &str = "sync-audit-on";
/// While this file exists beside the daemon socket, passes are `full` ([`DaemonPass`]).
const SYNC_FULL_SWITCH: &str = "sync-full-on";

/// The pass mode the daemon's next pass uses: the most conservative of `[daemon] pass` in
/// the store's loaded config and the switch files beside the daemon socket.
pub fn pass_mode(store: &Store) -> DaemonPass {
    let runtime = daemon_client::runtime_dir(store);
    effective_pass(
        store.config.daemon.pass,
        runtime.join(SYNC_AUDIT_SWITCH).exists(),
        runtime.join(SYNC_FULL_SWITCH).exists(),
    )
}

/// `corral daemon status` line naming a mode other than `incremental` and what set it.
pub fn pass_mode_line(store: &Store) -> Option<String> {
    let mode = pass_mode(store);
    let source = match mode {
        DaemonPass::Incremental => return None,
        _ if store.config.daemon.pass == mode => "[daemon] pass",
        DaemonPass::Audit => SYNC_AUDIT_SWITCH,
        DaemonPass::Full => SYNC_FULL_SWITCH,
    };
    Some(format!("sync passes: {} ({source})", mode.as_str()))
}

/// A switch file can only make passes more conservative: full > audit > incremental.
fn effective_pass(config: DaemonPass, audit_file: bool, full_file: bool) -> DaemonPass {
    let file = if full_file {
        DaemonPass::Full
    } else if audit_file {
        DaemonPass::Audit
    } else {
        DaemonPass::Incremental
    };
    config.max(file)
}

pub struct RunOptions {
    pub once: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Stopped,
    AlreadyRunning,
    Upgrade,
}

impl Exit {
    pub fn code(&self) -> u8 {
        match self {
            Self::Stopped | Self::AlreadyRunning => 0,
            Self::Upgrade => 75,
        }
    }
}

pub fn run(start: &Path, options: RunOptions) -> Result<Exit> {
    let ctx = Ctx::open_for_sync(start).context("opening daemon workspace")?;
    let shared = Arc::new(Shared::new());
    // Incremental state lives only in this loop: a restart begins with a full pass.
    let mut memory = PassMemory::new();
    let pacing = Cell::new(Pacing::default());
    run_loop(
        ctx,
        options,
        |ctx, supervisor| pass(ctx, supervisor, &shared, &mut memory, &pacing),
        Arc::clone(&shared),
        |spent| pacing.get().wait(spent),
    )
}

#[cfg(test)]
fn run_with_pass(
    ctx: Ctx,
    options: RunOptions,
    execute: impl FnMut(&mut Ctx, bool) -> Option<String>,
) -> Result<Exit> {
    run_with_state(ctx, options, execute, Arc::new(Shared::new()))
}

#[cfg(test)]
fn run_with_state(
    ctx: Ctx,
    options: RunOptions,
    execute: impl FnMut(&mut Ctx, bool) -> Option<String>,
    shared: Arc<Shared>,
) -> Result<Exit> {
    run_loop(ctx, options, execute, shared, periodic_wait)
}

/// `pace` turns the CPU time of the pass just finished into the wait before the next one;
/// a nudge still starts the next pass at once.
fn run_loop(
    mut ctx: Ctx,
    options: RunOptions,
    mut execute: impl FnMut(&mut Ctx, bool) -> Option<String>,
    shared: Arc<Shared>,
    mut pace: impl FnMut(Duration) -> Duration,
) -> Result<Exit> {
    if !ctx.store.own_loop().context("claiming daemon loop lease")? {
        if options.once {
            bail!("corrald daemon owns the loop; use corral daemon nudge");
        }
        return Ok(Exit::AlreadyRunning);
    }
    let mut cleanup = Cleanup {
        store: ctx.store.clone(),
        socket: None,
        shared: Arc::clone(&shared),
        projector: None,
    };
    ctx.store
        .set_attribution(Actor::System, "daemon")
        .context("attributing daemon writes")?;
    prepare_runtime(&ctx.store)?;
    if options.once {
        let mut timing_before = corral_ops::timing::snapshot();
        shared.begin();
        let (cpu, error) = measured_pass(&mut ctx, false, &mut execute, &mut timing_before);
        shared.finish(cpu, error);
        if ctx.store.runtime_latched() {
            return Ok(Exit::Upgrade);
        }
        if let Some(error) = &shared.lock().last_error {
            bail!("daemon one-shot pass failed: {error}");
        }
        return Ok(Exit::Stopped);
    }
    #[cfg(not(unix))]
    bail!("daemon Unix sockets are unavailable on this platform");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixListener;
        let socket = daemon_client::socket_path(&ctx.store);
        match std::fs::remove_file(&socket) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("removing stale daemon socket"),
        }
        let listener = UnixListener::bind(&socket).context("binding daemon socket")?;
        cleanup.socket = Some(socket.clone());
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))
            .context("securing daemon socket")?;
        cleanup.projector = Some(projector::start(&ctx, &shared)?);
        reader::start(&ctx, &shared)?;
        commands::start(&ctx, &shared)?;
        server::start(listener, Arc::clone(&shared), ctx.store.dir())?;
        let transitions = Arc::new(Mutex::new(health::Transitions::new()));
        transitions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .started(&ctx.store, &shared.health())?;
        health::start(&ctx, &shared, Arc::clone(&transitions))?;
        let mut timing_before = corral_ops::timing::snapshot();
        while shared.begin() {
            let (spent, error) = measured_pass(&mut ctx, true, &mut execute, &mut timing_before);
            shared.finish(spent, error);
            if ctx.store.runtime_latched() {
                shared.stop(true);
            }
            let snapshot = shared.health();
            if let Err(error) = transitions
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .update(&ctx.store, &snapshot)
            {
                eprintln!("corrald health transition: {error:#}");
            }
            if shared.lock().stop {
                break;
            }
            shared.wait(pace(spent), true);
        }
        let upgrading = shared.lock().upgrading;
        if !upgrading {
            transitions
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .stopped(&ctx.store, &shared.health())?;
        }
        drop(cleanup);
        if let Some(error) = shared.lock().server_error.take() {
            bail!("daemon server failed: {error}");
        }
        Ok(if upgrading {
            Exit::Upgrade
        } else {
            Exit::Stopped
        })
    }
}

fn measured_pass(
    ctx: &mut Ctx,
    supervisor: bool,
    execute: &mut impl FnMut(&mut Ctx, bool) -> Option<String>,
    timing_before: &mut corral_ops::timing::Snapshot,
) -> (Duration, Option<String>) {
    let started = Instant::now();
    let before = process_cpu();
    let error = execute(ctx, supervisor);
    let spent = match (before, process_cpu()) {
        (Some(before), Some(after)) => after.saturating_sub(before),
        _ => started.elapsed(),
    };
    *timing_before = corral_ops::timing::pass(ctx.store.dir(), started, timing_before);
    (spent, error)
}

/// What the loop needs from the pass just finished to pace the next one.
#[derive(Debug, Clone, Copy, Default)]
struct Pacing {
    /// An incremental pass that ran no gated step: it only checked its inputs.
    quiet: bool,
}

impl Pacing {
    /// A full pass is never quiet; a failed pass (no report) is paced like a full one.
    fn of(report: Option<&PassReport>) -> Self {
        Self {
            quiet: report.is_some_and(|report| report.quiet && report.full.is_none()),
        }
    }

    /// The CPU-proportional wait only: `report.next_due` never shortens it. Each pass waits at
    /// least 19 times its own CPU time, so paced passes stay under about 5% of a core.
    fn wait(self, spent: Duration) -> Duration {
        if self.quiet {
            quiet_wait(spent)
        } else {
            periodic_wait(spent)
        }
    }
}

/// A `cli`/`board` nudge or the `full` pass mode (config or `sync-full-on`) asks for a full pass.
fn pass_request((full, nudged): (bool, bool), forced: bool) -> PassRequest {
    if full {
        PassRequest::Full(FullReason::Owner)
    } else if forced {
        PassRequest::Full(FullReason::Switch)
    } else if nudged {
        PassRequest::Nudged
    } else {
        PassRequest::Paced
    }
}

/// Daemon extras D1-D3 follow the planner's rules; after a failed pass they all run.
fn extra_due(
    memory: &mut PassMemory,
    report: Option<&PassReport>,
    step: StepId,
    now: Timestamp,
) -> bool {
    report.is_none_or(|report| memory.extra_due(step, report, now))
}

fn pass(
    ctx: &mut Ctx,
    supervisor: bool,
    shared: &Shared,
    memory: &mut PassMemory,
    pacing: &Cell<Pacing>,
) -> Option<String> {
    let taken = shared.take_request();
    let (sender, diagnostics) = std::sync::mpsc::channel();
    let mut errors = Vec::new();
    let mut attention = None;
    let mut report = None;
    {
        let _capture = corral_core::capture_messages(sender);
        step(
            ctx.reload_config().map(|_| ()),
            "reloading config",
            &mut errors,
        );
        // After the reload, so a `[daemon] pass` edit applies from this pass on.
        let mode = pass_mode(&ctx.store);
        memory.set_audit(mode == DaemonPass::Audit);
        let request = pass_request(taken, mode == DaemonPass::Full);
        step(
            corral_ops::sync_incremental(ctx, memory, request).map(|(_, board, pass_report)| {
                if let Some(board) = board
                    && let Ok(view) = ctx.store.view()
                {
                    attention = Some((view.seq().0, board));
                }
                report = Some(pass_report);
            }),
            "synchronizing",
            &mut errors,
        );
        if report.is_none() {
            // The failed pass already consumed its feed delta; plan the next one from scratch.
            *memory = PassMemory::new();
        }
        let now = Timestamp::now();
        if extra_due(memory, report.as_ref(), StepId::Usage, now) {
            corral_ops::refresh_active_usage(&ctx.store);
        }
        if supervisor && extra_due(memory, report.as_ref(), StepId::Supervise, now) {
            step(corral_ops::supervise(ctx), "supervising PM", &mut errors);
        }
        if extra_due(memory, report.as_ref(), StepId::MaintainDatabase, now) {
            step(
                ctx.store.maintain_database(),
                "maintaining database",
                &mut errors,
            );
        }
        step(
            touch_heartbeat(&ctx.store),
            "touching heartbeat",
            &mut errors,
        );
    }
    if attention.is_some() || report.as_ref().is_none_or(|report| report.attention) {
        // Later pass stages may write too. Lease bookkeeping is harmless, but any
        // other intervening change makes the reader load its own attention board.
        let attention = attention.filter(|(seq, _)| {
            ctx.store
                .view()
                .is_ok_and(|view| reader::unchanged(view.as_ref(), *seq))
        });
        *shared
            .attention
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = attention;
    } else {
        keep_attention(ctx, shared);
    }
    if taken.0
        && report
            .as_ref()
            .is_some_and(|report| report.full == Some(FullReason::Owner))
    {
        // Only an owner's request (not `sync-full-on`) forces the frame; set before `finish`
        // publishes the pass, so the reader sees both together.
        shared.classic_force.store(true, Ordering::Relaxed);
    }
    pacing.set(Pacing::of(report.as_ref()));
    for message in diagnostics.try_iter() {
        for line in message.text.lines() {
            eprintln!("{}: {line}", message.source);
        }
    }
    if errors.is_empty() {
        None
    } else {
        Some(errors.join("; "))
    }
}

/// The pass skipped attention: the previous board stays current while the feed is unchanged
/// since it was computed.
fn keep_attention(ctx: &Ctx, shared: &Shared) {
    let Some(seq) = shared
        .attention
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
        .map(|(seq, _)| *seq)
    else {
        return;
    };
    // Scan without holding the mutex; the reader may advance the board's sequence meanwhile.
    if ctx
        .store
        .view()
        .is_ok_and(|view| reader::unchanged(view.as_ref(), seq))
    {
        return;
    }
    let mut attention = shared
        .attention
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if attention
        .as_ref()
        .is_some_and(|(current, _)| *current == seq)
    {
        *attention = None;
    }
}

fn step(result: Result<()>, source: &str, errors: &mut Vec<String>) {
    if let Err(error) = result {
        let error = format!("{source}: {error:#}");
        for line in error.lines() {
            eprintln!("{line}");
        }
        errors.push(error);
    }
}

fn prepare_runtime(store: &Store) -> Result<()> {
    let dir = daemon_client::runtime_dir(store);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .context("creating daemon runtime directory")?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .context("securing daemon runtime directory")?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(&dir).context("creating daemon runtime directory")?;
    Ok(())
}

fn touch_heartbeat(store: &Store) -> Result<()> {
    let path = daemon_client::heartbeat_path(store);
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
        .context("opening daemon heartbeat")?;
    Ok(())
}

struct Cleanup {
    store: Store,
    socket: Option<PathBuf>,
    shared: Arc<Shared>,
    projector: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let upgrading = self.shared.lock().upgrading;
        self.shared.stop(upgrading);
        self.shared.close();
        if let Some(projector) = self.projector.take()
            && projector.join().is_err()
        {
            eprintln!("corrald projector thread failed");
        }
        self.shared.wait_workers(Duration::from_secs(2));
        if let Some(socket) = &self.socket
            && let Err(error) = std::fs::remove_file(socket)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("corrald removing socket: {error:#}");
        }
        self.store.release_loop();
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
