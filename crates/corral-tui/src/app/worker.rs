//! Background synchronization transport.
use std::path::Path;

use super::*;

pub(super) enum WorkerMsg {
    Synced(std::result::Result<(Vec<Change>, Box<crate::board::BoardSnapshot>), String>),
    Herdr(bool),
}

impl WorkerMsg {
    /// One board pass: in process while this executable is current; once an update latched it
    /// off, through `installed`, so autopilot keeps running until the board restarts (T-587).
    pub(super) fn pass(ctx: &Ctx, messages: &Sender<Message>, installed: &Path) -> Self {
        if ctx.store.runtime_latched() {
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
pub(super) fn spawn_worker(
    mut ctx: Ctx,
    messages: Sender<Message>,
) -> (Sender<()>, Receiver<WorkerMsg>) {
    let (trigger_tx, trigger_rx) = mpsc::channel::<()>();
    let (msg_tx, msg_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _messages = corral_ops::capture_messages(messages.clone());
        let installed = ctx.store.installed_executable().to_path_buf();
        let mut herdr_checked: Option<Instant> = None;
        loop {
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
            if msg_tx
                .send(WorkerMsg::pass(&ctx, &messages, &installed))
                .is_err()
            {
                return;
            }
            // A handed-off pass spends its CPU in the child, so its wall time bounds the cost.
            let spent = match (ctx.store.runtime_latched(), cpu_before, process_cpu()) {
                (false, Some(before), Some(after)) => after.saturating_sub(before),
                _ => started.elapsed(),
            };
            match trigger_rx.recv_timeout(periodic_wait(spent)) {
                Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });
    (trigger_tx, msg_rx)
}

/// The longest wait between periodic syncs; it bounds how late autopilot reacts on its own.
const SYNC_MAX_WAIT: Duration = Duration::from_secs(15);

/// The wait before the next periodic sync: nine times the CPU the last pass cost, between
/// [`SYNC_EVERY`] and [`SYNC_MAX_WAIT`], so an idle board on a costly store stays near a tenth of
/// a core (T-547). A trigger from a user action still syncs at once.
pub(super) fn periodic_wait(pass_cpu: Duration) -> Duration {
    pass_cpu.saturating_mul(9).clamp(SYNC_EVERY, SYNC_MAX_WAIT)
}

/// CPU time used so far by this whole process (every thread), where /proc provides it.
fn process_cpu() -> Option<Duration> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // utime and stime are fields 14 and 15; the first field after the parenthesized name is 3.
    let mut fields = stat.rsplit_once(')')?.1.split_whitespace();
    let utime: u64 = fields.nth(11)?.parse().ok()?;
    let stime: u64 = fields.next()?.parse().ok()?;
    // Linux reports these in USER_HZ, which is 100 on every architecture.
    Some(Duration::from_millis(
        utime.saturating_add(stime).saturating_mul(10),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periodic_sync_waits_nine_times_pass_cpu_between_two_and_fifteen_seconds() {
        let ms = Duration::from_millis;
        assert_eq!(periodic_wait(Duration::ZERO), SYNC_EVERY);
        assert_eq!(periodic_wait(ms(100)), SYNC_EVERY);
        assert_eq!(periodic_wait(ms(500)), ms(4_500));
        assert_eq!(periodic_wait(ms(1_500)), ms(13_500));
        assert_eq!(periodic_wait(ms(2_000)), SYNC_MAX_WAIT);
        assert_eq!(periodic_wait(Duration::MAX), SYNC_MAX_WAIT);
    }

    #[test]
    fn process_cpu_is_read_and_never_runs_backwards() {
        let before = process_cpu();
        if cfg!(target_os = "linux") {
            assert!(before.is_some());
        }
        assert!(process_cpu() >= before);
    }
}
