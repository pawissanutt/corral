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
/// The records kept as their own messages beside the calls they were folded into.
const FOLDED_BYTES: usize = 256 * 1024;

/// A result folded into the held message `call`: its record as its own message, and the
/// last record put before it.
struct Folded {
    call: String,
    own: SizedOp,
    after: Option<String>,
}

pub(super) type SizedOp = (Op, usize);

#[derive(Default)]
pub(super) struct TranscriptBuffer {
    pub(super) next: u64,
    pub(super) revision: u64,
    pub(super) operations: VecDeque<SizedOp>,
    bytes: usize,
    older: Option<String>,
    rows: Vec<SizedOp>,
    folded: Vec<Folded>,
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
                    match (own, self.holds(&message.id)) {
                        (Some(own), false) => {
                            op = own;
                            if let Op::Put { message, .. } = &op {
                                if whole.contains(&message.id) {
                                    continue;
                                }
                                whole.push(message.id.clone());
                            }
                        }
                        // Held now, but the rows may begin after it later.
                        (Some(own), true) => self.fold(message.id.clone(), own),
                        (None, _) => {}
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
                    self.folded.clear();
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
                    self.unfold();
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
        self.at(id).is_some()
    }

    fn at(&self, id: &str) -> Option<usize> {
        self.rows
            .iter()
            .position(|(row, _)| matches!(row, Op::Put { message, .. } if message.id == id))
    }

    /// A result was folded into the held message `call`. `own` is the result's record as
    /// its own message: it is kept for when the rows begin after `call`.
    fn fold(&mut self, call: String, own: Op) {
        let Op::Put { message, .. } = &own else {
            return;
        };
        let cost = json_len(message).map_or(usize::MAX, |size| size.saturating_add(1));
        // The record follows every record put so far.
        let after = self.rows.iter().rev().find_map(|(row, _)| match row {
            Op::Put {
                message,
                open: false,
                ..
            } => Some(message.id.clone()),
            _ => None,
        });
        self.folded.push(Folded {
            call,
            own: (own, cost),
            after,
        });
        let mut bytes = self
            .folded
            .iter()
            .fold(0usize, |bytes, fold| bytes.saturating_add(fold.own.1));
        while bytes > FOLDED_BYTES && !self.folded.is_empty() {
            bytes = bytes.saturating_sub(self.folded.remove(0).own.1);
        }
    }

    /// The rows are what a subscriber that joins is given, and the pages before them are
    /// read from where the first begins. When a call's message leaves the rows, a result
    /// folded into it from a record after that point would be in neither: it takes its
    /// place among the rows as its own message, as a history page that parts them has it.
    fn unfold(&mut self) {
        // Newest first, so that results after the same row stay in the journal's order.
        for fold in std::mem::take(&mut self.folded).into_iter().rev() {
            if self.holds(&fold.call) {
                self.folded.insert(0, fold);
                continue;
            }
            let Op::Put { message, .. } = &fold.own.0 else {
                continue;
            };
            let after = fold.after.as_deref().filter(|after| *after != fold.call);
            if let Some(own) = self.at(&message.id) {
                // What the fold left of the record is among the rows: the record whole.
                self.rows[own] = fold.own;
            } else if let Some(after) = after.and_then(|after| self.at(after)) {
                self.rows.insert(after + 1, fold.own);
            }
            // Else the record is before the first row, where a page before them reads it.
        }
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
