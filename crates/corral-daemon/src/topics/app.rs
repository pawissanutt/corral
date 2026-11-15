//! App topics use pinned replacement walks, not an unbounded per-client delta queue.
//! Every walk upserts all current rows then explicitly removes all unseen old IDs.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, mpsc};

use super::notification;
use crate::{
    state::Shared,
    views::{self, Key, Rows, Snapshot, ThreadPage, Watch},
};
use anyhow::{Context, Result, ensure};
use corral_protocol::{
    self as wire,
    app::APP_CONTROL_MAX,
    board::BoardCursor,
    frame::Frame,
    subscription::{LiveText, SubAck, SubOpen, SubReset, Topic},
};

pub(crate) mod session;
mod terminal;

#[derive(Default)]
pub(super) struct AppTopics {
    subscribers: BTreeMap<Key, Subscriber>,
    sessions: session::SessionTopics,
    terminals: terminal::TerminalTopics,
}

struct Subscriber {
    watch: Watch,
    epoch: u64,
    reset: bool,
    cursor: BoardCursor,
    in_flight: bool,
    delivered: u64,
    transfer: Option<Transfer>,
    known: BTreeSet<String>,
    live: Option<LivePosition>,
    pending_live: Option<LivePosition>,
    serial: u64,
    live_next: bool,
}

#[derive(Clone)]
struct LivePosition {
    entry: String,
    turn: u64,
    end: usize,
    done: bool,
}

struct Transfer {
    rev: u64,
    page: Option<Arc<Snapshot>>,
    index: usize,
    pending: Option<mpsc::Receiver<Result<ThreadPage>>>,
    next: Option<u64>,
    head: Option<u64>,
    last: bool,
    remaining: BTreeSet<String>,
    seen: BTreeSet<String>,
    left: Option<Vec<String>>,
    left_index: usize,
}

impl AppTopics {
    pub fn open(&mut self, shared: &Shared, open: SubOpen) -> Result<()> {
        if open.topic == Topic::Terminal {
            ensure!(
                self.terminals.contains(open.scope.as_deref())
                    || self.subscribers.len() + self.sessions.len() + self.terminals.len()
                        < session::SESSION_SUBSCRIPTIONS,
                "too many topic subscriptions"
            );
            return self.terminals.open(shared, open);
        }
        if open.topic == Topic::Session {
            ensure!(
                self.sessions.contains(open.scope.as_deref())
                    || self.subscribers.len() + self.sessions.len() + self.terminals.len()
                        < session::SESSION_SUBSCRIPTIONS,
                "too many topic subscriptions"
            );
            return self.sessions.open(shared, open);
        }
        let key = Key::new(open.topic, open.scope)?;
        ensure!(
            self.subscribers.contains_key(&key)
                || self.subscribers.len() + self.sessions.len() + self.terminals.len()
                    < session::SESSION_SUBSCRIPTIONS,
            "too many topic subscriptions"
        );
        // Drop the replaced watch first; counters count subscriptions, never open calls.
        self.subscribers.remove(&key);
        let watch = views::watch(shared, key.clone())?;
        let reset = open.since.as_ref().is_some_and(|since| {
            since.epoch != shared.epoch() || since.cursor != BoardCursor::default()
        });
        self.subscribers.insert(
            key,
            Subscriber {
                watch,
                epoch: shared.epoch(),
                reset,
                cursor: BoardCursor::default(),
                in_flight: false,
                delivered: 0,
                transfer: None,
                known: BTreeSet::new(),
                live: None,
                pending_live: None,
                serial: 0,
                live_next: true,
            },
        );
        Ok(())
    }

    pub fn close(&mut self, topic: Topic, scope: Option<String>) {
        if topic == Topic::Terminal {
            self.terminals.close(scope);
            return;
        }
        if topic == Topic::Session {
            self.sessions.close(scope);
            return;
        }
        if let Ok(key) = Key::new(topic, scope) {
            self.subscribers.remove(&key);
        }
    }

    pub fn ack(&mut self, ack: SubAck) -> Result<()> {
        if ack.topic == Topic::Terminal {
            return self.terminals.ack(ack);
        }
        if ack.topic == Topic::Session {
            return self.sessions.ack(ack);
        }
        let key = Key::new(ack.topic, ack.scope)?;
        let sub = self
            .subscribers
            .get_mut(&key)
            .context("topic is not open")?;
        ensure!(
            sub.in_flight && sub.cursor == ack.cursor,
            "ack does not match the in-flight cursor"
        );
        sub.in_flight = false;
        if let Some(position) = sub.pending_live.take() {
            sub.live = Some(position);
        }
        Ok(())
    }

    pub fn flush(&mut self, shared: &Shared, conn: &mut dyn crate::conn::Conn) -> Result<()> {
        self.sessions.flush(shared, conn)?;
        self.terminals.flush(conn)?;
        let mut closed = Vec::new();
        for (key, subscriber) in &mut self.subscribers {
            match subscriber.next(shared) {
                Ok(Some((frame, close))) => {
                    conn.send_frame(&frame).context("pushing app topic frame")?;
                    if close {
                        closed.push(key.clone());
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    // Never claim an old snapshot is fresh after a failed read.
                    // Reset makes the client retain old data as stale until it reopens.
                    let reset = notification(
                        wire::SUB_RESET,
                        SubReset {
                            topic: key.topic(),
                            scope: key.scope.clone(),
                            epoch: shared.epoch(),
                        },
                    )?;
                    conn.send_frame(&reset)
                        .context("pushing unavailable topic reset")?;
                    let mut message = format!("app topic unavailable: {error:#}");
                    while message.len() > 1024 {
                        message.pop();
                    }
                    crate::conn::error(
                        conn,
                        serde_json::Value::Null,
                        wire::rpc::ErrorCode::Unavailable,
                        &message,
                    )?;
                    closed.push(key.clone());
                }
            }
        }
        for key in closed {
            self.subscribers.remove(&key);
        }
        Ok(())
    }
}

impl Subscriber {
    fn next(&mut self, shared: &Shared) -> Result<Option<(Frame, bool)>> {
        if self.in_flight {
            return Ok(None);
        }
        if self.reset || self.epoch != shared.epoch() {
            return Ok(Some((
                notification(
                    wire::SUB_RESET,
                    SubReset {
                        topic: self.watch.key.topic(),
                        scope: self.watch.key.scope.clone(),
                        epoch: shared.epoch(),
                    },
                )?,
                true,
            )));
        }
        let Some(snapshot) = self.watch.snapshot()? else {
            return Ok(None);
        };
        if self.transfer.is_none() && snapshot.rev != self.delivered {
            let thread = self.watch.key.topic() == Topic::Thread;
            self.transfer = Some(Transfer {
                rev: snapshot.rev,
                page: if thread { None } else { Some(snapshot) },
                index: 0,
                pending: if thread {
                    Some(views::thread(
                        shared,
                        self.watch
                            .key
                            .scope
                            .clone()
                            .context("thread scope unavailable")?,
                        None,
                    )?)
                } else {
                    None
                },
                next: None,
                head: None,
                last: !thread,
                remaining: self.known.clone(),
                seen: BTreeSet::new(),
                left: None,
                left_index: 0,
            });
        }
        if self.transfer.is_some() {
            // Long history walks must not hide the current answer. Alternate live
            // chunks with replacement pages, preserving the replacement's stale bit.
            if self.live_next
                && let Some(frame) = self.next_live(shared)?
            {
                self.live_next = false;
                return Ok(Some(frame));
            }
            let frame = self.next_transfer(shared)?;
            if frame.is_some() {
                self.live_next = true;
            }
            return Ok(frame);
        }
        self.next_live(shared)
    }

    fn next_transfer(&mut self, shared: &Shared) -> Result<Option<(Frame, bool)>> {
        let key = &self.watch.key;
        let transfer = self.transfer.as_mut().context("replacement unavailable")?;
        if let Some(receiver) = transfer.pending.as_ref() {
            match receiver.try_recv() {
                Ok(result) => {
                    let page = result?;
                    if let Some(head) = transfer.head {
                        ensure!(
                            head == page.head,
                            "thread changed during replacement; reopen for a fresh walk"
                        );
                    }
                    transfer.head = Some(page.head);
                    transfer.next = page.next;
                    transfer.last = page.next.is_none();
                    transfer.page = Some(Arc::new(Snapshot {
                        rev: transfer.rev,
                        rows: Rows::Thread(page.rows),
                    }));
                    transfer.index = 0;
                    transfer.pending = None;
                }
                Err(mpsc::TryRecvError::Empty) => return Ok(None),
                Err(mpsc::TryRecvError::Disconnected) => {
                    anyhow::bail!("thread reader disconnected")
                }
            }
        }
        let page = transfer
            .page
            .as_ref()
            .context("replacement page unavailable")?;
        if transfer.index == page.rows.len() && !transfer.last {
            transfer.pending = Some(views::thread(
                shared,
                key.scope.clone().context("thread scope unavailable")?,
                transfer.next,
            )?);
            return Ok(None);
        }
        let rows_phase = transfer.index < page.rows.len();
        if !rows_phase && transfer.left.is_none() {
            transfer.left = Some(transfer.remaining.iter().cloned().collect());
        }
        let available = if rows_phase {
            page.rows.len() - transfer.index
        } else {
            transfer.left.as_ref().map_or(0, Vec::len) - transfer.left_index
        };
        let mut count = available.min(100);
        loop {
            // A final explicit completion page follows the row pages. This keeps the
            // replacement stale until every removal has actually been delivered.
            let more = rows_phase || count < available;
            self.serial = self
                .serial
                .checked_add(1)
                .context("topic page serial exhausted")?;
            let cursor = BoardCursor {
                rev: transfer.rev,
                after: more.then(|| format!("page:{}", self.serial)),
            };
            let range = if rows_phase {
                transfer.index..transfer.index + count
            } else {
                0..0
            };
            let left = if rows_phase {
                Vec::new()
            } else {
                transfer.left.as_ref().context("removal page unavailable")?
                    [transfer.left_index..transfer.left_index + count]
                    .to_vec()
            };
            let frame = notification(
                wire::SUB_DELTA,
                page.rows.delta(
                    self.epoch,
                    cursor.clone(),
                    range,
                    left,
                    more,
                    key.scope.as_deref(),
                    None,
                ),
            )?;
            if frame.payload.len() > APP_CONTROL_MAX {
                ensure!(count > 1, "single app row exceeds CONTROL budget");
                count = (count / 2).max(1);
                continue;
            }
            if rows_phase {
                for index in transfer.index..transfer.index + count {
                    let id = page.rows.id(index);
                    transfer.remaining.remove(id);
                    ensure!(
                        transfer.seen.insert(id.to_owned()),
                        "duplicate row id in replacement"
                    );
                }
                transfer.index += count;
            } else {
                transfer.left_index += count;
            }
            self.cursor = cursor;
            self.in_flight = true;
            if !more {
                self.delivered = transfer.rev;
                self.known = std::mem::take(&mut transfer.seen);
                self.transfer = None;
            }
            return Ok(Some((frame, false)));
        }
    }

    fn next_live(&mut self, shared: &Shared) -> Result<Option<(Frame, bool)>> {
        let Some(scope) = self.watch.key.scope.as_deref() else {
            return Ok(None);
        };
        let Some(live) = crate::assistant::live(shared, scope) else {
            return Ok(None);
        };
        ensure!(
            live.offset == 0,
            "live source must be cumulative from offset zero"
        );
        let prior = self
            .live
            .as_ref()
            .filter(|old| old.entry == live.entry && old.turn == live.turn);
        let start = prior.map_or(0, |old| old.end);
        ensure!(
            start <= live.text.len() && live.text.is_char_boundary(start),
            "live source lost acknowledged text"
        );
        if start == live.text.len() && prior.is_some_and(|old| old.done == live.done) {
            return Ok(None);
        }
        let mut end = (start + 32 * 1024).min(live.text.len());
        while !live.text.is_char_boundary(end) {
            end -= 1;
        }
        loop {
            let done = live.done && end == live.text.len();
            let chunk = LiveText {
                entry: live.entry.clone(),
                turn: live.turn,
                offset: start as u64,
                text: live.text[start..end].to_owned(),
                done,
            };
            self.serial = self
                .serial
                .checked_add(1)
                .context("topic page serial exhausted")?;
            let revision = self
                .transfer
                .as_ref()
                .map_or(self.delivered, |walk| walk.rev);
            let cursor = BoardCursor {
                rev: revision,
                after: Some(format!("live:{}", self.serial)),
            };
            let rows = Rows::Thread(Vec::new());
            let frame = notification(
                wire::SUB_DELTA,
                rows.delta(
                    self.epoch,
                    cursor.clone(),
                    0..0,
                    Vec::new(),
                    self.transfer.is_some(),
                    Some(scope),
                    Some(chunk),
                ),
            )?;
            if frame.payload.len() > APP_CONTROL_MAX {
                ensure!(end > start, "live envelope exceeds CONTROL budget");
                end = start + (end - start) / 2;
                while !live.text.is_char_boundary(end) {
                    end -= 1;
                }
                continue;
            }
            self.pending_live = Some(LivePosition {
                entry: live.entry.clone(),
                turn: live.turn,
                end,
                done,
            });
            self.cursor = cursor;
            self.in_flight = true;
            return Ok(Some((frame, false)));
        }
    }
}

#[cfg(test)]
mod tests;
