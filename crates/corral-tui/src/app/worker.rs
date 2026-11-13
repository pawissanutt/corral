//! Background synchronization transport.
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use super::*;
use corral_ops::classic::{ClassicCache, ClassicInputs};
use corral_ops::loop_pacing::{SYNC_EVERY, periodic_wait, process_cpu};

/// An explicit refresh waits this long for the daemon's pass before reading its frame.
const EXPLICIT_WAIT: Duration = Duration::from_secs(5);
/// With a classic watch, the frame is re-read this often without a notice (a cheap re-read of
/// the daemon's cached document, as a safety net).
const WATCHED_TICK: Duration = Duration::from_secs(60);
/// How long one `ClassicWatch::next` waits before the watch rechecks loop ownership.
#[cfg(unix)]
const WATCH_NEXT: Duration = Duration::from_secs(30);
/// A failed classic subscription is reopened after this delay.
#[cfg(unix)]
const WATCH_RETRY: Duration = Duration::from_secs(2);

pub(super) enum WorkerMsg {
    Synced(std::result::Result<(Vec<Change>, Box<crate::board::BoardSnapshot>), String>),
    Herdr(bool),
}

/// Why the worker starts its next update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wake {
    /// The app asked for a refresh.
    Explicit,
    /// The daemon announced a new classic frame.
    #[cfg_attr(not(unix), allow(dead_code))]
    Pushed,
    /// The wait ran out, or the app closed its trigger.
    Tick,
}

impl WorkerMsg {
    /// One board pass with a fresh frame cache; the worker loop keeps one cache instead.
    #[cfg(test)]
    pub(super) fn pass(ctx: &Ctx, messages: &Sender<Message>, installed: &Path) -> Self {
        Self::pass_cached(ctx, messages, installed, &mut ClassicCache::new(), false)
    }

    /// One board update. Daemon-owned: the daemon's classic frame. Otherwise a pass in process
    /// while this executable is current; once an update latched it off, through `installed`,
    /// so autopilot keeps running until the board restarts (T-587). `explicit` asks the daemon
    /// for a pass first and recomputes every part of a local frame.
    fn pass_cached(
        ctx: &Ctx,
        messages: &Sender<Message>,
        installed: &Path,
        classic: &mut ClassicCache,
        explicit: bool,
    ) -> Self {
        if ctx.store.loop_owned_elsewhere().unwrap_or(true) {
            Self::daemon(ctx, classic, explicit.then_some(EXPLICIT_WAIT))
        } else if ctx.store.runtime_latched() {
            Self::local(
                ctx,
                classic,
                sync_step::run_installed(ctx, installed),
                explicit,
            )
        } else {
            Self::local(
                ctx,
                classic,
                sync_step::run_with_attention(ctx, messages),
                explicit,
            )
        }
    }

    #[cfg(test)]
    pub(super) fn synced_with_attention(ctx: &Ctx, messages: &Sender<Message>) -> Self {
        Self::local(
            ctx,
            &mut ClassicCache::new(),
            sync_step::run_with_attention(ctx, messages),
            false,
        )
    }

    /// The daemon's frame, applied without store derivation. A missing, refused or invalid
    /// read computes the frame in this process with the same code.
    fn daemon(ctx: &Ctx, classic: &mut ClassicCache, wait: Option<Duration>) -> Self {
        let Some(frame) = sync_step::daemon_frame(ctx, wait) else {
            let attention = sync_step::local_attention(ctx);
            return Self::local(ctx, classic, Ok((Vec::new(), attention)), wait.is_some());
        };
        let _timing = corral_core::timing::span("board.frame.apply");
        Self::Synced(
            crate::board::BoardSnapshot::from_frame(ctx, frame)
                .map(|snapshot| (Vec::new(), Box::new(snapshot)))
                .map_err(|error| corral_herdr::friendly_error(&error)),
        )
    }

    /// Computes the frame through this process's cache: only parts whose inputs changed.
    fn local(
        ctx: &Ctx,
        classic: &mut ClassicCache,
        synced: sync_step::Synced,
        force: bool,
    ) -> Self {
        Self::Synced(synced.and_then(|(changes, attention)| {
            let _timing = corral_core::timing::span("board.snapshot");
            crate::board::BoardSnapshot::load_cached(
                ctx,
                classic,
                ClassicInputs {
                    now: jiff::Timestamp::now(),
                    attention: Some(attention),
                    force,
                },
            )
            .map(|snapshot| (changes, Box::new(snapshot)))
            .map_err(|error| corral_herdr::friendly_error(&error))
        }))
    }

    #[cfg(test)]
    pub(super) fn synced(ctx: &Ctx, changes: Result<Vec<Change>, String>) -> Self {
        Self::Synced(changes.and_then(|changes| {
            crate::board::BoardSnapshot::load(ctx)
                .map(|mut snapshot| {
                    snapshot.operating_header = operating::readback(ctx);
                    (changes, Box::new(snapshot))
                })
                .map_err(|error| corral_herdr::friendly_error(&error))
        }))
    }
}

/// Whether the classic watch thread runs, and whether it holds an open subscription.
#[derive(Default)]
struct WatchState {
    running: AtomicBool,
    subscribed: AtomicBool,
}

/// Clears the watch flags however the watch thread ends.
struct Watching(Arc<WatchState>);

impl Drop for Watching {
    fn drop(&mut self) {
        self.0.subscribed.store(false, Ordering::Release);
        self.0.running.store(false, Ordering::Release);
    }
}

/// Starts `corral-classic-watch` unless it runs. A thread that cannot start drops its
/// `Watching` guard, which leaves the worker on periodic ticks.
fn start_watch(
    store: &corral_core::Store,
    wakes: &Sender<Wake>,
    state: &Arc<WatchState>,
    closed: &Arc<AtomicBool>,
) {
    if state.running.swap(true, Ordering::AcqRel) {
        return;
    }
    let (store, wakes, watching, closed) = (
        store.clone(),
        wakes.clone(),
        Watching(state.clone()),
        closed.clone(),
    );
    let _ = std::thread::Builder::new()
        .name("corral-classic-watch".into())
        .spawn(move || watch_classic(&store, &wakes, &watching.0, &closed));
}

/// Turns the daemon's classic notices into wakes until the board closes, the board takes the
/// loop over, or the daemon has no `classic.v1` (the worker then falls back to periodic ticks
/// and starts a new watch on a later update).
#[cfg(unix)]
fn watch_classic(
    store: &corral_core::Store,
    wakes: &Sender<Wake>,
    state: &WatchState,
    closed: &AtomicBool,
) {
    let wanted = || !closed.load(Ordering::Acquire) && store.loop_owned_elsewhere().unwrap_or(true);
    while wanted() {
        match corral_ops::daemon_client::ClassicWatch::open(store) {
            Ok(Some(mut watch)) => {
                state.subscribed.store(true, Ordering::Release);
                loop {
                    match watch.next(WATCH_NEXT) {
                        Ok(Some(_)) => {
                            if wakes.send(Wake::Pushed).is_err() {
                                return;
                            }
                        }
                        Ok(None) if wanted() => {}
                        Ok(None) => return,
                        Err(_) => break,
                    }
                }
                state.subscribed.store(false, Ordering::Release);
            }
            Ok(None) => return,
            Err(_) => {}
        }
        std::thread::sleep(WATCH_RETRY);
    }
}

/// Daemon sockets are unix-only: the worker stays on periodic ticks.
#[cfg(not(unix))]
fn watch_classic(_: &corral_core::Store, _: &Sender<Wake>, _: &WatchState, _: &AtomicBool) {}

/// Sync runs off the UI thread so slow herdr calls never freeze input; a `()` on the returned sender triggers an
/// immediate sync. The thread ends when the app drops its channel ends.
#[cfg(test)]
pub(super) fn spawn_worker(
    ctx: Ctx,
    messages: Sender<Message>,
) -> (Sender<()>, Receiver<WorkerMsg>) {
    spawn_worker_with_status(ctx, messages, Arc::new(AtomicBool::new(false)))
}

pub(super) fn spawn_worker_with_status(
    mut ctx: Ctx,
    messages: Sender<Message>,
    syncing: Arc<AtomicBool>,
) -> (Sender<()>, Receiver<WorkerMsg>) {
    let (trigger_tx, trigger_rx) = mpsc::channel::<()>();
    let (msg_tx, msg_rx) = mpsc::channel();
    let (wake_tx, wake_rx) = mpsc::channel::<Wake>();
    let closed = Arc::new(AtomicBool::new(false));
    {
        // The app's `()` trigger becomes an explicit wake; its end closes the worker.
        let (wakes, closed) = (wake_tx.clone(), closed.clone());
        std::thread::spawn(move || {
            for () in trigger_rx {
                if wakes.send(Wake::Explicit).is_err() {
                    return;
                }
            }
            closed.store(true, Ordering::Release);
            let _ = wakes.send(Wake::Tick);
        });
    }
    std::thread::spawn(move || {
        let _messages = corral_ops::capture_messages(messages.clone());
        // This independent, low-frequency worker can revoke expired idle views even while
        // sync/autopilot is waiting on a long external command or the writer is busy.
        let (maintenance_stop, stopped) = mpsc::channel::<()>();
        let maintenance_store = ctx.store.clone();
        let maintenance_messages = messages.clone();
        std::thread::spawn(move || {
            let _capture = corral_ops::capture_messages(maintenance_messages.clone());
            while matches!(
                stopped.recv_timeout(SYNC_EVERY),
                Err(RecvTimeoutError::Timeout)
            ) {
                if maintenance_store.loop_owned_elsewhere().unwrap_or(true) {
                    continue;
                }
                if let Err(error) = maintenance_store.maintain_database() {
                    let _ = maintenance_messages.send(Message {
                        task: None,
                        file: None,
                        source: "Database maintenance",
                        text: format!("{error:#}"),
                    });
                }
            }
        });
        let _maintenance_stop = maintenance_stop;
        let installed = ctx.store.installed_executable().to_path_buf();
        let mut classic = ClassicCache::new();
        let watch = Arc::new(WatchState::default());
        let mut herdr_checked: Option<Instant> = None;
        let mut daemon_checked: Option<Instant> = None;
        let mut daemon_notice: Option<String> = None;
        let mut explicit = false;
        loop {
            syncing.store(true, Ordering::Release);
            let (started, cpu_before) = (Instant::now(), process_cpu());
            if let Err(error) = ctx.reload_config()
                && messages
                    .send(Message {
                        task: None,
                        file: None,
                        source: "Config not reloaded",
                        text: corral_herdr::friendly_error(&error),
                    })
                    .is_err()
            {
                return;
            }
            if herdr_checked.is_none_or(|t| t.elapsed() >= HERDR_CHECK_EVERY) {
                herdr_checked = Some(Instant::now());
                if msg_tx
                    .send(WorkerMsg::Herdr(ctx.herdr.available()))
                    .is_err()
                {
                    return;
                }
            }
            let daemon_owned = ctx.store.loop_owned_elsewhere().unwrap_or(true);
            if daemon_owned {
                start_watch(&ctx.store, &wake_tx, &watch, &closed);
                if daemon_checked.is_none_or(|at| at.elapsed() >= Duration::from_secs(10)) {
                    daemon_checked = Some(Instant::now());
                    let health = corral_ops::daemon_client::health(&ctx.store);
                    daemon_notice =
                        (!matches!(health.daemon, corral_protocol::DaemonState::Healthy))
                            .then(|| health.line());
                }
            } else {
                daemon_checked = None;
                daemon_notice = None;
            }
            let mut update =
                WorkerMsg::pass_cached(&ctx, &messages, &installed, &mut classic, explicit);
            if let (Some(notice), WorkerMsg::Synced(Ok((_, snapshot)))) =
                (&daemon_notice, &mut update)
            {
                snapshot.operating_header = format!("{notice}\n{}", snapshot.operating_header);
            }
            if !daemon_owned && let Err(error) = ctx.store.maintain_database() {
                let _ = messages.send(Message {
                    task: None,
                    file: None,
                    source: "Database maintenance",
                    text: format!("{error:#}"),
                });
            }
            syncing.store(false, Ordering::Release);
            if msg_tx.send(update).is_err() {
                return;
            }
            let wait = if daemon_owned && watch.subscribed.load(Ordering::Acquire) {
                WATCHED_TICK
            } else {
                // A handed-off pass spends its CPU in the child, so its wall time bounds the cost.
                let spent = match (ctx.store.runtime_latched(), cpu_before, process_cpu()) {
                    (false, Some(before), Some(after)) => after.saturating_sub(before),
                    _ => started.elapsed(),
                };
                periodic_wait(spent)
            };
            let wake = match wake_rx.recv_timeout(wait) {
                Ok(wake) => wake,
                Err(RecvTimeoutError::Timeout) => Wake::Tick,
                Err(RecvTimeoutError::Disconnected) => return,
            };
            explicit = wake == Wake::Explicit;
            for wake in wake_rx.try_iter() {
                explicit |= wake == Wake::Explicit;
            }
            if closed.load(Ordering::Acquire) {
                return;
            }
        }
    });
    (trigger_tx, msg_rx)
}

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;
