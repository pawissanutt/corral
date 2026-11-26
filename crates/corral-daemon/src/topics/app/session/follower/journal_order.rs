use std::collections::VecDeque;

use super::{JournalAnchor, Record};

// A page from the host may lag the journal. Keep recent IDs for those anchors, but once
// resolved, the current anchor's rank stays valid until the journal is read again.
const RECENT: usize = 512;

#[derive(Default)]
pub(super) struct Order {
    recent: VecDeque<(String, u64)>,
    pub(super) rank: u64,
    root: bool,
}

impl Order {
    pub(super) fn snapshot(&mut self, root: bool, records: &[Record]) {
        self.root = root;
        self.append(records);
    }

    pub(super) fn append<'a>(&mut self, records: impl IntoIterator<Item = &'a Record>) -> u64 {
        let before = self.rank;
        for record in records {
            self.rank += u64::from(record.assistant);
            if let Some(id) = &record.entry_id {
                self.recent.push_back((id.clone(), self.rank));
            }
        }
        before
    }

    pub(super) fn trim(&mut self) {
        self.recent
            .drain(..self.recent.len().saturating_sub(RECENT));
    }
}

pub(super) struct Exact {
    anchor: JournalAnchor,
    rank: Option<u64>,
    /// A successful read did not find the boundary. Keep the turn identity so the
    /// next recording notice can resume it at a newer, retained boundary.
    missing: bool,
}

impl Exact {
    pub(super) fn new(anchor: JournalAnchor) -> Self {
        Self {
            anchor,
            rank: None,
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
        self.rank = None;
    }

    pub(super) fn resolve(&mut self, order: &Order) {
        if self.rank.is_none() {
            self.rank = match &self.anchor.entry_id {
                Some(id) => order
                    .recent
                    .iter()
                    .rev()
                    .find(|(seen, _)| seen == id)
                    .map(|(_, rank)| *rank),
                None => order.root.then_some(0),
            };
        }
        self.missing = self.rank.is_none();
    }

    pub(super) fn missing(&self) -> bool {
        self.missing
    }

    pub(super) fn before_turn(&self) -> Option<u64> {
        self.rank?.checked_sub(self.anchor.message)
    }

    pub(super) fn ordinal(&self, rank: u64) -> Option<u64> {
        let at = self.rank?;
        if rank >= at {
            Some(self.anchor.message.saturating_add(rank - at))
        } else {
            self.anchor.message.checked_sub(at - rank)
        }
    }

    pub(super) fn includes(&self, rank: u64) -> bool {
        self.ordinal(rank).is_some_and(|ordinal| ordinal != 0)
    }
}
