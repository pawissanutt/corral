use std::collections::VecDeque;

use super::{JournalAnchor, Record};

// A page from the host may lag the journal. Keep recent IDs for those anchors, but once
// resolved, the current anchor stays valid until the journal is read again.
const RECENT: usize = 512;

#[derive(Default)]
pub(super) struct Order {
    recent: VecDeque<Record>,
    root: bool,
}

impl Order {
    pub(super) fn snapshot(&mut self, root: bool, records: &[Record]) {
        self.root = root;
        self.append(records);
    }

    pub(super) fn append<'a>(&mut self, records: impl IntoIterator<Item = &'a Record>) {
        self.recent.extend(
            records
                .into_iter()
                .filter(|record| record.entry_id.is_some() || record.reply.is_some())
                .cloned(),
        );
    }

    pub(super) fn trim(&mut self) {
        self.recent
            .drain(..self.recent.len().saturating_sub(RECENT));
    }

    fn holds(&self, id: Option<&str>) -> bool {
        match id {
            Some(id) => self
                .recent
                .iter()
                .any(|record| record.entry_id.as_deref() == Some(id)),
            None => self.root,
        }
    }

    fn recorded(&self, turn: u64) -> u64 {
        self.recent
            .iter()
            .filter_map(|record| record.reply)
            .filter(|reply| reply.turn == turn)
            .map(|reply| reply.message)
            .max()
            .unwrap_or(0)
    }
}

pub(super) struct Exact {
    anchor: JournalAnchor,
    resolved: bool,
    /// A successful read did not find the boundary. Keep the turn identity so the
    /// next recording notice can resume it at a newer, retained boundary.
    missing: bool,
}

impl Exact {
    pub(super) fn new(anchor: JournalAnchor) -> Self {
        Self {
            anchor,
            resolved: false,
            missing: false,
        }
    }

    pub(super) fn update(exact: &mut Option<Self>, anchor: &JournalAnchor) {
        if exact
            .as_ref()
            .is_none_or(|old| old.anchor.message < anchor.message)
        {
            let missing = exact.as_ref().is_some_and(Self::missing);
            *exact = Some(Self {
                missing,
                ..Self::new(anchor.clone())
            });
        }
    }

    pub(super) fn invalidate(&mut self) {
        self.resolved = false;
    }

    pub(super) fn resolve(&mut self, order: &Order) {
        self.resolved |= order.holds(self.anchor.entry_id.as_deref());
        self.missing = !self.resolved;
    }

    pub(super) fn missing(&self) -> bool {
        self.missing
    }

    pub(super) fn recorded(&self, turn: u64, order: &Order) -> Option<u64> {
        // The actor can append other assistant objects. Only the host's durable reply
        // marker (or the ID named by a recording notice) identifies a turn ordinal.
        self.resolved
            .then(|| self.anchor.message.max(order.recorded(turn)))
    }

    pub(super) fn matches(&self, id: &str) -> Option<u64> {
        (self.anchor.message != 0 && self.anchor.entry_id.as_deref() == Some(id))
            .then_some(self.anchor.message)
    }
}
