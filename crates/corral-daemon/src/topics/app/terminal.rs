//! A bounded replace-only terminal window. A slow reader skips intervening windows.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use corral_protocol::{
    self as wire,
    app::APP_CONTROL_MAX,
    detail::{TERMINAL_POLL_MS, TerminalSnapshot},
    frame::Frame,
    subscription::{BoardCursor, SubAck, SubOpen, Topic},
};

use crate::{conn::Conn, state::Shared, views};

const SCOPES_PER_CONNECTION: usize = 32;
const WATCHED_SCOPES: usize = 128;
const POLL: Duration = Duration::from_millis(TERMINAL_POLL_MS);

#[derive(Default)]
pub(in crate::topics) struct TerminalTopics {
    subscribers: BTreeMap<String, Subscriber>,
}

struct Subscriber {
    watch: Arc<Mutex<Capture>>,
    sent: Option<Arc<Window>>,
    pending: Option<BoardCursor>,
    rev: u64,
}

/// One latest capture per watched scope across connections. Once the last
/// subscriber drops, the registry's weak reference retains no terminal text.
#[derive(Default)]
pub(crate) struct TerminalCache {
    entries: Mutex<BTreeMap<String, Weak<Mutex<Capture>>>>,
}

#[derive(Default)]
struct Capture {
    checked: Option<Instant>,
    current: Option<Arc<Window>>,
}

impl TerminalCache {
    fn watch(&self, scope: &str) -> Result<Arc<Mutex<Capture>>> {
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(entry) = entries.get(scope).and_then(Weak::upgrade) {
            return Ok(entry);
        }
        entries.retain(|_, entry| entry.strong_count() != 0);
        ensure!(entries.len() < WATCHED_SCOPES, "too many watched terminals");
        let entry = Arc::new(Mutex::new(Capture::default()));
        entries.insert(scope.to_owned(), Arc::downgrade(&entry));
        Ok(entry)
    }
}

impl Capture {
    fn latest(&mut self, shared: &Shared, scope: &str, now: Instant) -> Result<Arc<Window>> {
        if let Some(current) = &self.current
            && self
                .checked
                .is_some_and(|at| now.saturating_duration_since(at) < POLL)
        {
            return Ok(Arc::clone(current));
        }
        self.checked = Some(now);
        let window = match shared
            .engine()
            .and_then(|engine| views::terminal(&engine, scope))
        {
            Ok(snapshot) => Window::Snapshot(snapshot),
            Err(error) => {
                let mut message = format!("{error:#}");
                message.truncate(message.floor_char_boundary(1024));
                Window::Error(message)
            }
        };
        if self
            .current
            .as_ref()
            .is_none_or(|old| !window.unchanged(old))
        {
            self.current = Some(Arc::new(window));
        }
        self.current
            .as_ref()
            .map(Arc::clone)
            .context("terminal observation unavailable")
    }
}

enum Window {
    Snapshot(TerminalSnapshot),
    Error(String),
}

impl Window {
    fn unchanged(&self, old: &Self) -> bool {
        match (self, old) {
            (Self::Snapshot(now), Self::Snapshot(old)) => {
                now.live == old.live && now.lines == old.lines && now.rows == old.rows
            }
            (Self::Error(now), Self::Error(old)) => now == old,
            _ => false,
        }
    }
}

impl TerminalTopics {
    pub fn open(&mut self, shared: &Shared, open: SubOpen) -> Result<()> {
        let scope = open.scope.context("terminal scope is required")?;
        views::run_scope(&scope)?;
        ensure!(
            self.subscribers.contains_key(&scope) || self.subscribers.len() < SCOPES_PER_CONNECTION,
            "too many terminal subscriptions"
        );
        // A replacement window is independent of any same-epoch cursor.
        let rev = open
            .since
            .filter(|since| since.epoch == shared.epoch())
            .map_or(0, |since| since.cursor.rev);
        ensure!(rev < (1_u64 << 53) - 1, "invalid terminal cursor revision");
        let watch = shared.terminal_cache.watch(&scope)?;
        self.subscribers.insert(
            scope,
            Subscriber {
                watch,
                sent: None,
                pending: None,
                rev,
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
        let scope = ack.scope.context("terminal scope is required")?;
        let sub = self
            .subscribers
            .get_mut(&scope)
            .context("terminal topic is not open")?;
        ensure!(
            sub.pending.as_ref() == Some(&ack.cursor),
            "ack does not match the in-flight cursor"
        );
        sub.pending = None;
        Ok(())
    }

    pub fn flush(&mut self, shared: &Shared, conn: &mut dyn Conn) -> Result<()> {
        let now = Instant::now();
        for (scope, sub) in &mut self.subscribers {
            if let Some(frame) = sub.next_at(shared, scope, now)? {
                conn.send_frame(&frame).context("pushing terminal window")?;
            }
        }
        Ok(())
    }
}

impl Subscriber {
    fn next_at(&mut self, shared: &Shared, scope: &str, now: Instant) -> Result<Option<Frame>> {
        if self.pending.is_some() {
            return Ok(None);
        }
        let window = self
            .watch
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .latest(shared, scope, now)?;
        if self
            .sent
            .as_ref()
            .is_some_and(|old| Arc::ptr_eq(old, &window))
        {
            return Ok(None);
        }
        let rev = self
            .rev
            .checked_add(1)
            .context("terminal revision exhausted")?;
        let cursor = BoardCursor { rev, after: None };
        let encode = |window: &Window| {
            #[derive(serde::Serialize)]
            struct Page<'a> {
                topic: Topic,
                scope: &'a str,
                epoch: u64,
                snapshot: Option<&'a TerminalSnapshot>,
                cursor: &'a BoardCursor,
                more: bool,
                #[serde(skip_serializing_if = "Option::is_none")]
                error: Option<&'a str>,
            }
            let (snapshot, error) = match window {
                Window::Snapshot(snapshot) => (Some(snapshot), None),
                Window::Error(error) => (None, Some(error.as_str())),
            };
            super::super::notification(
                wire::SUB_DELTA,
                Page {
                    topic: Topic::Terminal,
                    scope,
                    epoch: shared.epoch(),
                    snapshot,
                    cursor: &cursor,
                    more: false,
                    error,
                },
            )
        };
        let mut frame = encode(&window)?;
        if frame.payload.len() > APP_CONTROL_MAX {
            frame = encode(&Window::Error(
                "terminal window exceeds CONTROL budget".into(),
            ))?;
        }
        self.sent = Some(window);
        self.pending = Some(cursor);
        self.rev = rev;
        Ok(Some(frame))
    }
}

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod tests;
