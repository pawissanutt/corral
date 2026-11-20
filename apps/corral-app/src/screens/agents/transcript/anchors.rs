//! Where each message of a mounted transcript starts, by its anchor, per session scope: the
//! rows report their place as they are laid out, the view following the transcript reads which
//! row the reader is at from its scroll offset, and a reading position waiting to be restored
//! scrolls there once its row has been laid out (at any width).

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use nsh_core::prelude::*;

#[derive(Default)]
struct Rows {
    /// Each row's top in the transcript, with the token of the render that placed it.
    tops: BTreeMap<String, (u64, f32)>,
    /// A reading position to restore once its row is placed.
    pending: Option<(String, f32, ElementRef)>,
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

/// Row `anchor` of `scope` starts at `top`; a reading position waiting for it is restored.
pub(super) fn placed(scope: &str, anchor: &str, token: u64, top: f32) {
    let restore = SCOPES.with(|scopes| {
        let mut scopes = scopes.borrow_mut();
        let rows = scopes.entry(scope.to_owned()).or_default();
        rows.tops.insert(anchor.to_owned(), (token, top));
        match &rows.pending {
            Some((key, _, _)) if key == anchor => rows.pending.take(),
            _ => None,
        }
    });
    if let Some((_, offset, scroller)) = restore {
        scroll(scroller, top + offset);
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
                .is_some_and(|(placed, _)| *placed == token)
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
        let top = rows.tops.get(&anchor).map(|(_, top)| *top);
        rows.pending = match top {
            Some(_) => None,
            None => Some((anchor, offset, scroller)),
        };
        top
    });
    if let Some(top) = top {
        scroll(scroller, top + offset);
    }
}

/// No reading position waits to be restored in `scope`.
pub(super) fn cancel(scope: &str) {
    SCOPES.with(|scopes| {
        if let Some(rows) = scopes.borrow_mut().get_mut(scope) {
            rows.pending = None;
        }
    });
}

/// The row of `scope` the reader is at with the view scrolled to `offset`: the last row that
/// starts at or above it, and how far into that row the view starts.
pub(super) fn at(scope: &str, offset: f32) -> Option<(String, f32)> {
    SCOPES.with(|scopes| {
        let scopes = scopes.borrow();
        scopes
            .get(scope)?
            .tops
            .iter()
            .filter(|(_, (_, top))| *top <= offset + 1.0)
            .max_by(|(_, (_, a)), (_, (_, b))| a.total_cmp(b))
            .map(|(anchor, (_, top))| (anchor.clone(), offset - top))
    })
}
