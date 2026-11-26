use std::collections::VecDeque;

use corral_protocol::{
    detail::SessionItem,
    subscription::{TranscriptOp as Op, TranscriptPart as Part},
};

use super::{SESSION_BUFFER_BYTES, SESSION_BUFFER_EVENTS, SESSION_PAGE_BYTES, follower::chunks};
use crate::views::json_len;

/// Both parts of an open message at the follower's bound: the tail holds every word the
/// stream carried, so a reset and the appends after it agree.
const OPEN_BYTES: usize = 2 * super::follower::OPEN_PART_BYTES as usize;
const TAIL_BYTES: usize = 28 * 1024;

pub(super) type SizedOp = (Op, usize);

#[derive(Default)]
pub(super) struct TranscriptBuffer {
    pub(super) next: u64,
    pub(super) revision: u64,
    pub(super) operations: VecDeque<SizedOp>,
    bytes: usize,
    older: Option<String>,
    rows: Vec<SizedOp>,
}

fn sized(op: Op) -> SizedOp {
    let cost = json_len(&op).map_or(usize::MAX, |size| size.saturating_add(1));
    (op, cost)
}

impl TranscriptBuffer {
    /// `alone` has, for each `again` put of `ops` in order, the message it is of and the
    /// record whose result was folded into it, as that record's own message.
    pub(super) fn publish(&mut self, ops: Vec<Op>, mut alone: Vec<(String, Op)>) {
        // Records put as their own messages here, whole.
        let mut whole: Vec<String> = Vec::new();
        for mut op in ops {
            if let Op::Put { message, again, .. } = &op {
                if *again {
                    let own = alone.iter().position(|(of, _)| of == &message.id);
                    let own = own.map(|at| alone.remove(at).1);
                    // The rows are what every subscriber holds. When the call's message is
                    // not among them, a subscriber that joined since would ignore it put
                    // again, and no page before its messages reads the result: the result
                    // goes as its own message, as on a history page that parts them.
                    if !self.holds(&message.id)
                        && let Some(own) = own
                    {
                        op = own;
                        if let Op::Put { message, .. } = &op {
                            if whole.contains(&message.id) {
                                continue;
                            }
                            whole.push(message.id.clone());
                        }
                    }
                } else if whole.contains(&message.id) {
                    // What the fold left of a record that was put whole.
                    continue;
                }
            }
            match &op {
                Op::Reset {older} => {
                    self.revision += 1;
                    self.older.clone_from(older);
                    self.rows.clear();
                    self.operations.clear();
                    self.bytes = 0;
                }
                Op::Put {message, replaces, again, ..} => {
                    let mut entry = op.clone();
                    if let Op::Put {replaces, again, ..} = &mut entry { *replaces = None; *again = false; }
                    // The tail is measured like a history page, without per-operation
                    // envelopes. Snapshots measure those separately for delivery.
                    let entry = (entry, json_len(message).map_or(usize::MAX, |n| n + 1));
                    if let Some(at) = self.rows.iter().position(|(row, _)| matches!(row, Op::Put { message: old, .. } if old.id == message.id || replaces.as_ref() == Some(&old.id))) {
                        self.rows[at] = entry;
                    } else if !*again {
                        self.rows.push(entry);
                    }
                    // A cut message is where a reader's messages start again.
                    if let Op::Put {cut: true, before, ..} = &op
                        && let Some(at) = self.rows.iter().position(|(row, _)| matches!(row, Op::Put { message: held, .. } if held.id == message.id))
                    {
                        self.rows.drain(..at);
                        self.older.clone_from(before);
                    }
                    self.trim();
                }
                Op::Append {id, part, offset, text} => {
                    if let Some((Op::Put {message, ..}, _)) = self.rows.iter_mut().find(|(row, _)| matches!(row, Op::Put { message, open: true, .. } if &message.id == id)) {
                        apply(&mut message.items, *part, *offset, text);
                    }
                }
                Op::Drop {id} => self.rows.retain(|(row, _)| !matches!(row, Op::Put {message, ..} if &message.id == id)),
                Op::Other => continue,
            }
            let entry = sized(op);
            debug_assert!(entry.1 <= SESSION_PAGE_BYTES);
            self.next += 1;
            self.bytes += entry.1;
            self.operations.push_back(entry);
            while self.bytes > SESSION_BUFFER_BYTES || self.operations.len() > SESSION_BUFFER_EVENTS
            {
                if let Some((_, cost)) = self.operations.pop_front() {
                    self.bytes -= cost;
                }
            }
        }
    }

    fn holds(&self, id: &str) -> bool {
        self.rows
            .iter()
            .any(|(row, _)| matches!(row, Op::Put { message, .. } if message.id == id))
    }

    fn trim(&mut self) {
        let mut bytes = 1usize;
        let mut count = 0;
        let mut removed = false;
        for i in (0..self.rows.len()).rev() {
            let (op, cost) = &self.rows[i];
            if matches!(op, Op::Put { open: false, .. }) {
                count += 1;
                bytes = bytes.saturating_add(*cost);
                // Keep at least the newest complete message, even with its envelope.
                if count > 32 || (count > 1 && bytes > TAIL_BYTES) {
                    self.rows.remove(i);
                    removed = true;
                }
            }
        }
        if removed {
            self.older = self
                .rows
                .iter()
                .find_map(|(row, _)| match row {
                    Op::Put {
                        before,
                        open: false,
                        ..
                    } => Some(before.clone()),
                    _ => None,
                })
                .flatten();
        }
    }

    pub(super) fn snapshot(&self) -> VecDeque<SizedOp> {
        let mut ops = vec![Op::Reset {
            older: self.older.clone(),
        }];
        for (op, _) in &self.rows {
            let mut op = op.clone();
            if let Op::Put {
                message,
                open: true,
                ..
            } = &mut op
            {
                let id = message.id.clone();
                let items = std::mem::take(&mut message.items);
                ops.push(op);
                for item in items {
                    match item {
                        SessionItem::Text { text } => chunks(&mut ops, &id, Part::Text, 0, &text),
                        SessionItem::Thinking { text } => {
                            chunks(&mut ops, &id, Part::Thinking, 0, &text)
                        }
                        _ => {}
                    }
                }
            } else {
                ops.push(op);
            }
        }
        ops.into_iter().map(sized).collect()
    }
}

fn apply(items: &mut Vec<SessionItem>, part: Part, offset: u64, text: &str) {
    let Some(offset) = usize::try_from(offset)
        .ok()
        .filter(|&offset| offset < OPEN_BYTES)
    else {
        return;
    };
    let total = items
        .iter()
        .map(|item| match item {
            SessionItem::Text { text } | SessionItem::Thinking { text } => text.len(),
            _ => 0,
        })
        .sum::<usize>();
    let index = items.iter().position(|item| {
        matches!(
            (part, item),
            (Part::Text, SessionItem::Text { .. }) | (Part::Thinking, SessionItem::Thinking { .. })
        )
    });
    let target = if let Some(index) = index {
        &mut items[index]
    } else {
        if offset != 0 {
            return;
        }
        match part {
            Part::Text => {
                items.push(SessionItem::Text {
                    text: String::new(),
                });
                let len = items.len();
                &mut items[len - 1]
            }
            Part::Thinking => {
                items.insert(
                    0,
                    SessionItem::Thinking {
                        text: String::new(),
                    },
                );
                &mut items[0]
            }
        }
    };
    let (SessionItem::Text { text: held } | SessionItem::Thinking { text: held }) = target else {
        return;
    };
    if held.is_char_boundary(offset) {
        let end = offset + text.len();
        if end < held.len() && !held.is_char_boundary(end) {
            return;
        }
        let room = OPEN_BYTES.saturating_sub(total - held.len() + offset);
        let text = &text[..text.floor_char_boundary(room)];
        held.replace_range(offset..(offset + text.len()).min(held.len()), text);
    }
}
