//! Shared scan accounting: a bound omits evidence explicitly, never fails a read.
use corral_protocol::detail::UsageCompleteness;
use jiff::Timestamp;

pub(super) struct Budget {
    pub(super) coverage: UsageCompleteness,
    max: usize,
    pub(super) first: Option<Timestamp>,
    exhausted: bool,
    pub(super) last: Option<Timestamp>,
}

impl Budget {
    pub(super) fn new(max: usize) -> Self {
        Self {
            coverage: UsageCompleteness {
                partial: false,
                reason: None,
                scanned_rows: 0,
                scanned_from: None,
                scanned_to: None,
                unpriced_entries: 0,
                undated_entries: 0,
            },
            max,
            exhausted: false,
            first: None,
            last: None,
        }
    }

    pub(super) fn exhausted(&self) -> bool {
        self.exhausted
    }

    pub(super) fn remaining(&self) -> usize {
        self.max.saturating_sub(self.coverage.scanned_rows as usize)
    }

    pub(super) fn admit(&mut self, count: usize) -> bool {
        if self.exhausted {
            return false;
        }
        if count > self.remaining() {
            self.partial("row_limit");
            return false;
        }
        self.coverage.scanned_rows += count as u64;
        true
    }

    pub(super) fn partial(&mut self, reason: &str) {
        let bound = matches!(reason, "row_limit" | "transcript_row_limit");
        self.exhausted |= bound;
        self.coverage.partial = true;
        if self.coverage.reason.is_none() || bound {
            self.coverage.reason = Some(reason.to_owned());
        }
    }

    pub(super) fn observe(&mut self, at: Timestamp) {
        if self.first.is_none_or(|first| at < first) {
            self.first = Some(at);
            self.coverage.scanned_from = Some(at.to_string());
        }
        if self.last.is_none_or(|last| at > last) {
            self.last = Some(at);
            self.coverage.scanned_to = Some(at.to_string());
        }
    }
}
