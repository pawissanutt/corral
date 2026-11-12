//! The workspace's single synchronization loop owner.
mod health;
mod projector;
#[cfg(unix)]
mod server;
mod state;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use corral_core::{Actor, Store};
use corral_ops::{
    Ctx, daemon_client,
    loop_pacing::{periodic_wait, process_cpu},
};

use state::Shared;

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
    run_with_pass(ctx, options, pass)
}

fn run_with_pass(
    ctx: Ctx,
    options: RunOptions,
    execute: impl FnMut(&mut Ctx, bool) -> Option<String>,
) -> Result<Exit> {
    run_with_state(ctx, options, execute, Arc::new(Shared::new()))
}

fn run_with_state(
    mut ctx: Ctx,
    options: RunOptions,
    mut execute: impl FnMut(&mut Ctx, bool) -> Option<String>,
    shared: Arc<Shared>,
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
        shared.begin();
        let (cpu, error) = measured_pass(&mut ctx, false, &mut execute);
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
        server::start(listener, Arc::clone(&shared), ctx.store.dir())?;
        let transitions = Arc::new(Mutex::new(health::Transitions::new()));
        transitions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .started(&ctx.store, &shared.health())?;
        health::start(&ctx, &shared, Arc::clone(&transitions))?;
        while shared.begin() {
            let (spent, error) = measured_pass(&mut ctx, true, &mut execute);
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
            shared.wait(periodic_wait(spent), true);
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
) -> (Duration, Option<String>) {
    let started = Instant::now();
    let before = process_cpu();
    let error = execute(ctx, supervisor);
    let spent = match (before, process_cpu()) {
        (Some(before), Some(after)) => after.saturating_sub(before),
        _ => started.elapsed(),
    };
    (spent, error)
}

fn pass(ctx: &mut Ctx, supervisor: bool) -> Option<String> {
    let (sender, diagnostics) = std::sync::mpsc::channel();
    let mut errors = Vec::new();
    {
        let _capture = corral_core::capture_messages(sender);
        step(
            ctx.reload_config().map(|_| ()),
            "reloading config",
            &mut errors,
        );
        step(
            corral_ops::sync_with_attention(ctx).map(|_| ()),
            "synchronizing",
            &mut errors,
        );
        corral_ops::refresh_active_usage(&ctx.store);
        if supervisor {
            step(corral_ops::supervise(ctx), "supervising PM", &mut errors);
        }
        step(
            ctx.store.maintain_database(),
            "maintaining database",
            &mut errors,
        );
        step(
            touch_heartbeat(&ctx.store),
            "touching heartbeat",
            &mut errors,
        );
    }
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
