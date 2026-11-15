//! One shared replacement window per watched session, never a per-client output queue.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use corral_core::Store;
use corral_ops::Ctx;
use corral_protocol::{
    self as wire,
    app::APP_CONTROL_MAX,
    board::BoardCursor,
    detail::{DetailBody, DetailGet, DetailRef, TERMINAL_POLL_MS, TerminalSnapshot},
    frame::Frame,
    subscription::{SubAck, SubDelta, SubOpen},
};

use super::super::notification;
use crate::state::Shared;

const SCOPES: usize = 128;
const SCOPE_BYTES: usize = 512;
const ERROR_BYTES: usize = 1024;
const POLL: Duration = Duration::from_millis(TERMINAL_POLL_MS);

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|error| error.into_inner())
}

#[derive(Default)]
pub(crate) struct Service {
    entries: Mutex<BTreeMap<String, Arc<Mutex<Entry>>>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Default)]
struct Entry {
    users: usize,
    last_read: Option<Instant>,
    rev: u64,
    published: Option<Arc<Frame>>,
}

impl Service {
    pub(super) fn start(self: &Arc<Self>, ctx: &Ctx, shared: &Arc<Shared>) -> Result<()> {
        let ctx = Ctx {
            store: Store::open_unobserved(ctx.store.root())
                .context("opening terminal stream store")?,
            herdr: ctx.herdr.clone(),
            resource_probe: Arc::clone(&ctx.resource_probe),
        };
        let worker = Arc::clone(self);
        let shared = Arc::clone(shared);
        let thread = std::thread::Builder::new()
            .name("corrald-terminal-live".into())
            .spawn(move || {
                while !shared.lock().closed {
                    worker.poll(Instant::now(), shared.epoch(), |scope| {
                        let reply = corral_ops::app_read::read(
                            &ctx,
                            &DetailGet {
                                target: DetailRef::Terminal { id: scope.into() },
                                etag: None,
                                page: None,
                            },
                        )?;
                        match reply.body {
                            Some(DetailBody::Terminal(snapshot)) => Ok(snapshot),
                            _ => anyhow::bail!("terminal window unavailable"),
                        }
                    });
                    std::thread::sleep(POLL);
                }
            })
            .context("starting terminal stream worker")?;
        *lock(&self.thread) = Some(thread);
        Ok(())
    }

    pub(super) fn join(&self) {
        if let Some(thread) = lock(&self.thread).take()
            && thread.join().is_err()
        {
            eprintln!("corrald terminal stream worker failed");
        }
    }

    fn poll(
        &self,
        now: Instant,
        epoch: u64,
        mut read: impl FnMut(&str) -> Result<TerminalSnapshot>,
    ) {
        // Release the registry before I/O: opening/closing a page never waits on herdr.
        let entries: Vec<_> = lock(&self.entries)
            .iter()
            .map(|(scope, entry)| (scope.clone(), Arc::clone(entry)))
            .collect();
        for (scope, entry) in entries {
            {
                let mut entry = lock(&entry);
                if entry.users == 0
                    || entry
                        .last_read
                        .is_some_and(|at| now.saturating_duration_since(at) < POLL)
                {
                    continue;
                }
                entry.last_read = Some(now);
            }
            let result = read(&scope);
            let mut entry = lock(&entry);
            if entry.users == 0 {
                continue;
            }
            // Keep exactly one latest frame. A slow consumer skips intervening windows;
            // each window includes recent scrollback, so repeated screens never append.
            let rev = entry.rev.saturating_add(1);
            let cursor = BoardCursor { rev, after: None };
            let (snapshot, error) = match result {
                Ok(snapshot) => (Some(snapshot), None),
                Err(error) => {
                    let mut message = format!("{error:#}");
                    while message.len() > ERROR_BYTES {
                        message.pop();
                    }
                    (None, Some(message))
                }
            };
            let frame = notification(
                wire::SUB_DELTA,
                SubDelta::Terminal {
                    scope: scope.clone(),
                    epoch,
                    snapshot,
                    cursor,
                    more: false,
                    error,
                },
            )
            .and_then(|frame| {
                ensure!(
                    frame.payload.len() <= APP_CONTROL_MAX,
                    "terminal window exceeds CONTROL budget"
                );
                Ok(frame)
            });
            match frame {
                Ok(frame) => {
                    entry.rev = rev;
                    entry.published = Some(Arc::new(frame));
                }
                Err(_) => {
                    // A producer violating the wire bound must not leave a stale frame
                    // looking live. Send a bounded unavailable replacement instead.
                    if let Ok(frame) = notification(
                        wire::SUB_DELTA,
                        SubDelta::Terminal {
                            scope,
                            epoch,
                            snapshot: None,
                            cursor: BoardCursor { rev, after: None },
                            more: false,
                            error: Some("terminal window exceeds CONTROL budget".into()),
                        },
                    ) {
                        entry.rev = rev;
                        entry.published = Some(Arc::new(frame));
                    }
                }
            }
        }
    }
}

struct Watch {
    service: Arc<Service>,
    scope: String,
    entry: Arc<Mutex<Entry>>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        let mut entries = lock(&self.service.entries);
        let mut entry = lock(&self.entry);
        entry.users -= 1;
        if entry.users == 0 {
            entries.remove(&self.scope);
        }
    }
}

struct Subscriber {
    watch: Watch,
    delivered: u64,
    in_flight: Option<BoardCursor>,
}

impl Subscriber {
    fn next(&mut self) -> Option<Arc<Frame>> {
        if self.in_flight.is_some() {
            return None;
        }
        let entry = lock(&self.watch.entry);
        if entry.rev == self.delivered {
            return None;
        }
        let frame = Arc::clone(entry.published.as_ref()?);
        self.delivered = entry.rev;
        self.in_flight = Some(BoardCursor {
            rev: entry.rev,
            after: None,
        });
        Some(frame)
    }
}

#[derive(Default)]
pub(super) struct TerminalTopics {
    subscribers: BTreeMap<String, Subscriber>,
}

impl TerminalTopics {
    pub fn len(&self) -> usize {
        self.subscribers.len()
    }

    pub fn contains(&self, scope: Option<&str>) -> bool {
        scope.is_some_and(|scope| self.subscribers.contains_key(scope))
    }

    pub fn open(&mut self, shared: &Shared, open: SubOpen) -> Result<()> {
        let scope = scope(open.scope)?;
        let service = lock(&shared.session_live)
            .as_ref()
            .map(|service| Arc::clone(&service.terminal))
            .context("terminal stream service unavailable")?;
        // Drop the old watch before counting; reopening never leaks scope slots.
        self.subscribers.remove(&scope);
        let entry = {
            let mut entries = lock(&service.entries);
            ensure!(
                entries.contains_key(&scope) || entries.len() < SCOPES,
                "too many watched terminals"
            );
            let entry = entries.entry(scope.clone()).or_default();
            lock(entry).users += 1;
            Arc::clone(entry)
        };
        // Reconnect always receives a complete current window, regardless of `since`.
        self.subscribers.insert(
            scope.clone(),
            Subscriber {
                watch: Watch {
                    service,
                    scope,
                    entry,
                },
                delivered: 0,
                in_flight: None,
            },
        );
        Ok(())
    }

    pub fn close(&mut self, scope: Option<String>) {
        if let Some(scope) = scope {
            self.subscribers.remove(&scope);
        }
    }

    pub fn ack(&mut self, ack: SubAck) -> Result<()> {
        let scope = scope(ack.scope)?;
        let subscriber = self
            .subscribers
            .get_mut(&scope)
            .context("terminal topic is not open")?;
        ensure!(
            subscriber.in_flight.as_ref() == Some(&ack.cursor),
            "ack does not match the in-flight cursor"
        );
        subscriber.in_flight = None;
        Ok(())
    }

    pub fn flush(&mut self, conn: &mut dyn crate::conn::Conn) -> Result<()> {
        for subscriber in self.subscribers.values_mut() {
            if let Some(frame) = subscriber.next() {
                conn.send_frame(&frame)
                    .context("pushing terminal stream window")?;
            }
        }
        Ok(())
    }
}

fn scope(scope: Option<String>) -> Result<String> {
    let scope = scope.context("terminal scope is required")?;
    ensure!(
        !scope.is_empty() && scope.len() <= SCOPE_BYTES && !scope.chars().any(char::is_control),
        "invalid terminal scope"
    );
    Ok(scope)
}

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod tests;
