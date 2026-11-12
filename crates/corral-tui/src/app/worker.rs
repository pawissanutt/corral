//! Background synchronization transport.
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use super::*;
use corral_ops::loop_pacing::{SYNC_EVERY, periodic_wait, process_cpu};

pub(super) enum WorkerMsg {
    Synced(std::result::Result<(Vec<Change>, Box<crate::board::BoardSnapshot>), String>),
    Herdr(bool),
}

impl WorkerMsg {
    /// One board pass: in process while this executable is current; once an update latched it
    /// off, through `installed`, so autopilot keeps running until the board restarts (T-587).
    pub(super) fn pass(ctx: &Ctx, messages: &Sender<Message>, installed: &Path) -> Self {
        if ctx.store.loop_owned_elsewhere().unwrap_or(true) {
            Self::loaded(ctx, sync_step::run_daemon(ctx, None))
        } else if ctx.store.runtime_latched() {
            Self::loaded(ctx, sync_step::run_installed(ctx, installed))
        } else {
            Self::synced_with_attention(ctx, messages)
        }
    }

    pub(super) fn synced_with_attention(ctx: &Ctx, messages: &Sender<Message>) -> Self {
        Self::loaded(ctx, sync_step::run_with_attention(ctx, messages))
    }

    fn loaded(
        ctx: &Ctx,
        synced: Result<(Vec<Change>, crate::attention::Snapshot, String), String>,
    ) -> Self {
        Self::Synced(synced.and_then(|(changes, attention, operating_header)| {
            crate::board::BoardSnapshot::load_with_attention(ctx, attention)
                .map(|mut snapshot| {
                    snapshot.operating_header = operating_header;
                    (changes, Box::new(snapshot))
                })
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
            let mut update = if daemon_owned {
                WorkerMsg::loaded(
                    &ctx,
                    sync_step::run_daemon(&ctx, explicit.then(|| Duration::from_secs(5))),
                )
            } else {
                WorkerMsg::pass(&ctx, &messages, &installed)
            };
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
            // A handed-off pass spends its CPU in the child, so its wall time bounds the cost.
            let spent = match (ctx.store.runtime_latched(), cpu_before, process_cpu()) {
                (false, Some(before), Some(after)) => after.saturating_sub(before),
                _ => started.elapsed(),
            };
            match trigger_rx.recv_timeout(periodic_wait(spent)) {
                Ok(()) => {
                    explicit = true;
                    for () in trigger_rx.try_iter() {}
                }
                Err(RecvTimeoutError::Timeout) => {
                    explicit = false;
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });
    (trigger_tx, msg_rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_owned_tick_never_enters_in_process_sync() {
        let dir = tempfile::tempdir().unwrap();
        let owner = corral_core::Store::init(dir.path()).unwrap();
        assert!(owner.own_loop().unwrap());
        let store = corral_core::Store::open(dir.path()).unwrap();
        let ctx = Ctx {
            store,
            herdr: corral_herdr::Herdr::with_bin(dir.path().join("must-not-run"), None),
            resource_probe: corral_ops::resources::FixedResourceProbe::arc(),
        };
        // An in-process sync would check runtime and latch off on this version.
        std::fs::write(ctx.store.dir().join("state/schema-version"), "999\n").unwrap();
        let (sender, _messages) = mpsc::channel();
        let _ = WorkerMsg::pass(&ctx, &sender, Path::new("/must-not-run"));
        assert!(!ctx.store.runtime_latched());
    }
}
