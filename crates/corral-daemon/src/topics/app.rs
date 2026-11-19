//! Pin each replacement walk until its last page; observations may coalesce between walks.
use std::{collections::BTreeSet, sync::Arc};

use anyhow::{Context, Result, ensure};
use corral_protocol::{
    app::APP_CONTROL_MAX,
    frame::Frame,
    subscription::{BoardCursor, SubOpen, Topic},
};

use crate::{state::Shared, views::Rows};

mod terminal;
pub(crate) use terminal::TerminalCache;
pub(super) use terminal::TerminalTopics;

mod session;
pub(super) use session::SessionTopics;
pub(crate) use session::{SessionLive, join_session_live, start_session_live};

const ROWS_PER_PAGE: usize = 100;

pub(super) struct Subscriber {
    pub topic: Topic,
    epoch: u64,
    reset: bool,
    rev: u64,
    pending: Option<BoardCursor>,
    delivered: Option<Arc<Rows>>,
    walk: Option<Walk>,
}

struct Walk {
    next: Arc<Rows>,
    row: usize,
    left: Vec<String>,
    removal: usize,
}

impl Subscriber {
    pub(super) fn new(epoch: u64, open: SubOpen) -> Self {
        Self {
            topic: open.topic,
            epoch,
            // The daemon does not keep replay history for disconnected callers.
            reset: open.since.is_some_and(|since| {
                since.epoch != epoch || since.cursor != BoardCursor::default()
            }),
            rev: 0,
            pending: None,
            delivered: None,
            walk: None,
        }
    }

    pub(super) fn ack(&mut self, cursor: BoardCursor) -> Result<()> {
        ensure!(
            self.pending.as_ref() == Some(&cursor),
            "ack does not match the in-flight cursor"
        );
        self.pending = None;
        Ok(())
    }

    pub(super) fn next(
        &mut self,
        shared: &Shared,
        build_rows: bool,
    ) -> Result<Option<(Frame, bool)>> {
        if self.pending.is_some() {
            return Ok(None);
        }
        if self.reset || self.epoch != shared.epoch() {
            return Ok(Some((super::reset(self.topic, shared.epoch())?, true)));
        }
        if self.walk.is_none() {
            let next = if self.topic == Topic::Assistant && build_rows {
                shared.row_cache.assistant_with_builds(shared)?
            } else {
                shared.row_cache.get(shared, self.topic)?
            };
            if self
                .delivered
                .as_ref()
                .is_some_and(|old| Arc::ptr_eq(old, &next))
            {
                return Ok(None);
            }
            let left = if let Some(old) = &self.delivered {
                let ids: BTreeSet<_> = (0..next.len()).map(|i| next.id(i)).collect();
                (0..old.len())
                    .map(|i| old.id(i))
                    .filter(|id| !ids.contains(id))
                    .map(|id| id.into_owned())
                    .collect()
            } else {
                Vec::new()
            };
            self.rev = self
                .rev
                .checked_add(1)
                .context("topic revision exhausted")?;
            self.walk = Some(Walk {
                next,
                row: 0,
                left,
                removal: 0,
            });
        }
        self.page().map(|frame| Some((frame, false)))
    }

    fn page(&mut self) -> Result<Frame> {
        let walk = self.walk.as_mut().context("topic walk is unavailable")?;
        let rows_remaining = walk.next.len() - walk.row;
        let left_remaining = walk.left.len() - walk.removal;
        let sending_rows = rows_remaining > 0;
        let mut count = if sending_rows {
            rows_remaining
        } else {
            left_remaining
        }
        .min(ROWS_PER_PAGE);
        loop {
            let (rows, left) = if sending_rows { (count, 0) } else { (0, count) };
            let row = walk.row + rows;
            let removal = walk.removal + left;
            let more = rows < rows_remaining || left < left_remaining;
            let cursor = BoardCursor {
                rev: self.rev,
                after: more.then(|| format!("{row}:{removal}")),
            };
            let frame = walk.next.frame(
                self.topic,
                self.epoch,
                cursor.clone(),
                walk.row..row,
                &walk.left[walk.removal..removal],
                more,
            )?;
            if frame.payload.len() <= APP_CONTROL_MAX {
                walk.row = row;
                walk.removal = removal;
                self.pending = Some(cursor);
                if !more {
                    self.delivered = self.walk.take().map(|walk| walk.next);
                }
                return Ok(frame);
            }
            ensure!(count > 1, "single topic row exceeds CONTROL budget");
            count /= 2;
        }
    }
}

#[cfg(test)]
mod tests;
