//! A scope owns one bridge cursor; connections own only acknowledged buffer positions.
use std::{
    sync::{Arc, Mutex, MutexGuard},
    thread::JoinHandle,
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use corral_protocol::app::APP_CONTROL_MAX;

use crate::state::Shared;

mod buffer;
mod reader;
mod subscriber;

use buffer::Watches;
use reader::{Sources, poll};
pub(in crate::topics) use subscriber::SessionTopics;

pub(crate) const SESSION_POLL: Duration = Duration::from_millis(100);
pub(crate) const SESSION_SCOPES: usize = 128;
pub(crate) const SESSION_SUBSCRIPTIONS: usize = 32;
pub(crate) const SESSION_SCOPE_BYTES: usize = 512;
pub(crate) const SESSION_BUFFER_EVENTS: usize = 512;
pub(crate) const SESSION_BUFFER_BYTES: usize = 512 * 1024;
pub(crate) const SESSION_PAGE_EVENTS: usize = 32;
// A quoted 512-byte scope needs at most 1,024 bytes; this also reserves the RPC
// envelope, scalar fields, cursor and optional gap without serializing twice.
pub(crate) const SESSION_ENVELOPE_BYTES: usize = 4 * 1024;
pub(crate) const SESSION_PAGE_BYTES: usize = APP_CONTROL_MAX - SESSION_ENVELOPE_BYTES;

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|error| error.into_inner())
}

#[derive(Default)]
pub(crate) struct SessionLive {
    watches: Arc<Watches>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

pub(crate) fn start_session_live(shared: &Arc<Shared>) -> Result<()> {
    let mut slot = lock(&shared.session_live.thread);
    ensure!(slot.is_none(), "session live worker already started");
    let shared_worker = Arc::clone(shared);
    let thread = std::thread::Builder::new()
        .name("corrald-session-live".into())
        .spawn(move || {
            let mut sources = Sources::default();
            while !shared_worker.lock().closed {
                poll(
                    &shared_worker.session_live.watches,
                    &mut sources,
                    |scope, reader| reader.read(&shared_worker, scope),
                    || shared_worker.lock().closed,
                );
                // Sleep after the complete pass: wakeups of unrelated topics cannot exceed 10 Hz.
                std::thread::sleep(SESSION_POLL);
            }
        })
        .context("starting session live worker")?;
    *slot = Some(thread);
    Ok(())
}

pub(crate) fn join_session_live(shared: &Shared) {
    let thread = lock(&shared.session_live.thread).take();
    if let Some(thread) = thread
        && thread.join().is_err()
    {
        eprintln!("corrald session live worker failed");
    }
}

#[cfg(test)]
#[path = "session/session_tests.rs"]
mod tests;
