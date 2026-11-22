//! Where each message of a mounted transcript starts, by its anchor, per session scope: the
//! rows report their place as they are laid out, the view following the transcript reads which
//! row the reader is at from its scroll offset, and a reading position waiting to be restored
//! scrolls there once its row has been laid out (at any width).

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use nsh_core::prelude::*;

/// Where in its row a waiting reading position is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Mark {
    /// This far below the row's top.
    Top(f32),
    /// This far above the row's bottom: rows arriving above it, its own earlier items read
    /// from the page before included, leave what is below the reader as it was.
    Bottom(f32),
}

impl Mark {
    /// The scroll offset of this mark in a row placed at `top`, `height` high.
    pub(super) fn offset(self, top: f32, height: f32) -> f32 {
        match self {
            Self::Top(into) => top + into,
            Self::Bottom(above) => top + height - above,
        }
    }
}

#[derive(Default)]
struct Rows {
    /// Each row's top and height in the transcript, with the token of the render that placed
    /// it.
    tops: BTreeMap<String, (u64, f32, f32)>,
    /// A reading position to restore once its row is placed, and whether it holds the reader
    /// while rows arrive above (rather than reopening where they left).
    pending: Option<(String, Mark, ElementRef, bool)>,
}

thread_local! {
    static SCOPES: RefCell<BTreeMap<String, Rows>> = const { RefCell::new(BTreeMap::new()) };
    static TOKENS: Cell<u64> = const { Cell::new(0) };
}

/// A token for one render of a row, so a later render's place is not forgotten with an
/// earlier one.
pub(super) fn token() -> u64 {
    TOKENS.with(|next| {
        let token = next.get().wrapping_add(1);
        next.set(token);
        token
    })
}

fn scroll(scroller: ElementRef, at: f32) {
    after_answer(move || scroller.scroll_to_offset(px(at.max(0.0)), false));
}

/// Row `anchor` of `scope` starts at `top` and is `height` high; a reading position waiting
/// for it is restored.
pub(super) fn placed(scope: &str, anchor: &str, token: u64, top: f32, height: f32) {
    let restore = SCOPES.with(|scopes| {
        let mut scopes = scopes.borrow_mut();
        let rows = scopes.entry(scope.to_owned()).or_default();
        rows.tops.insert(anchor.to_owned(), (token, top, height));
        match &rows.pending {
            Some((key, ..)) if key == anchor => rows.pending.take(),
            _ => None,
        }
    });
    if let Some((_, mark, scroller, _)) = restore {
        scroll(scroller, mark.offset(top, height));
    }
}

/// The render `token` of row `anchor` is gone.
pub(super) fn forget(scope: &str, anchor: &str, token: u64) {
    SCOPES.with(|scopes| {
        let mut scopes = scopes.borrow_mut();
        if let Some(rows) = scopes.get_mut(scope)
            && rows
                .tops
                .get(anchor)
                .is_some_and(|(placed, ..)| *placed == token)
        {
            rows.tops.remove(anchor);
        }
    });
}

/// Scrolls `scroller` to `offset` into row `anchor` of `scope`: now when the row is placed,
/// else as soon as it is.
pub(super) fn restore(scope: &str, anchor: String, offset: f32, scroller: ElementRef) {
    let top = SCOPES.with(|scopes| {
        let mut scopes = scopes.borrow_mut();
        let rows = scopes.entry(scope.to_owned()).or_default();
        let top = rows.tops.get(&anchor).map(|(_, top, _)| *top);
        rows.pending = match top {
            Some(_) => None,
            None => Some((anchor, Mark::Top(offset), scroller, false)),
        };
        top
    });
    if let Some(top) = top {
        scroll(scroller, top + offset);
    }
}

/// Scrolls `scroller` back to `offset` into row `anchor` of `scope` once the row is placed
/// again: rows were added above the reader, before the row or at its own front, so where it
/// was last placed no longer holds. Kept from its bottom, which neither moves within the row.
pub(super) fn hold(scope: &str, anchor: String, offset: f32, scroller: ElementRef) {
    SCOPES.with(|scopes| {
        let mut scopes = scopes.borrow_mut();
        let rows = scopes.entry(scope.to_owned()).or_default();
        let mark = match rows.tops.remove(&anchor) {
            Some((_, _, height)) => Mark::Bottom(height - offset),
            None => Mark::Top(offset),
        };
        rows.pending = Some((anchor, mark, scroller, true));
    });
}

/// No reading position waits to be restored in `scope`.
pub(super) fn cancel(scope: &str) {
    SCOPES.with(|scopes| {
        if let Some(rows) = scopes.borrow_mut().get_mut(scope) {
            rows.pending = None;
        }
    });
}

/// The reader scrolled on their own in `scope`: rows that arrived above without moving theirs
/// hold nothing any more, so a later change of that row's height does not move them.
pub(super) fn moved(scope: &str) {
    SCOPES.with(|scopes| {
        if let Some(rows) = scopes.borrow_mut().get_mut(scope)
            && rows.pending.as_ref().is_some_and(|(.., held)| *held)
        {
            rows.pending = None;
        }
    });
}

/// The row of `scope` the reader is at with the view scrolled to `offset`: the last row that
/// starts at or above it, and how far into that row the view starts. Above the first row (the
/// control for earlier messages) it is the first row, the view starting before it, so rows
/// that arrive above keep it in place too.
pub(super) fn at(scope: &str, offset: f32) -> Option<(String, f32)> {
    SCOPES.with(|scopes| {
        let scopes = scopes.borrow();
        let tops = scopes
            .get(scope)?
            .tops
            .iter()
            .map(|(anchor, (_, top, _))| (anchor, *top));
        tops.clone()
            .filter(|(_, top)| *top <= offset + 1.0)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .or_else(|| tops.min_by(|a, b| a.1.total_cmp(&b.1)))
            .map(|(anchor, top)| (anchor.clone(), offset - top))
    })
}
