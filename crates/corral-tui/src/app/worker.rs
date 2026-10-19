//! Background synchronization transport.
use super::*;

pub(super) enum WorkerMsg {
    Synced(std::result::Result<(Vec<Change>, Box<crate::board::BoardSnapshot>), String>),
    Herdr(bool),
}

impl WorkerMsg {
    pub(super) fn synced(ctx: &Ctx, changes: Result<Vec<Change>, String>) -> Self {
        Self::Synced(changes.and_then(|changes| {
            crate::board::BoardSnapshot::load(ctx)
                .map(|snapshot| (changes, Box::new(snapshot)))
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
        let mut herdr_checked: Option<Instant> = None;
        loop {
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
            let synced = sync_step::run(&ctx);
            if msg_tx.send(WorkerMsg::synced(&ctx, synced)).is_err() {
                return;
            }
            match trigger_rx.recv_timeout(SYNC_EVERY) {
                Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });
    (trigger_tx, msg_rx)
}
