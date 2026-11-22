//! Where each message of a mounted transcript starts, by its anchor, per session scope: the
//! rows report their place as they are laid out, the view following the transcript reads which
//! row the reader is at from its scroll offset, and a reading position waiting to be restored
//! scrolls there once its row has been laid out (at any width). The items of a recorded
//! message report their place in it too, by their stable keys, so the reader stays on the item
//! they read while rows arrive above and below it in the same update. An opened group's calls
//! report theirs by their call ids, through the parts they are drawn in (the group's rows in the
//! group, the message's column of items in it), so the reader stays on the call they read while
//! calls join the group before and after it.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

use nsh_core::prelude::*;

/// Where in its row a waiting reading position is.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Mark {
    /// This far below the row's top.
    Top(f32),
    /// This far below the top of a row that was `height` high, its items as they were placed
    /// (key, top, height; all in the row, sorted by top; an opened group's calls with it).
    /// Rows arrive above and below the reader, the row's own earlier items read from the page
    /// before and later replies joining it included: the reader moves with the item they read.
    Held {
        into: f32,
        height: f32,
        items: Vec<(String, f32, f32)>,
    },
}

impl Mark {
    /// The scroll offset of this mark in a row placed at `top`, `height` high, its items now
    /// placed as `now` says (by key: top and height).
    pub(super) fn offset(
        &self,
        top: f32,
        height: f32,
        now: impl Fn(&str) -> Option<(f32, f32)>,
    ) -> f32 {
        let (into, was, items) = match self {
            Self::Top(into) => return top + into,
            Self::Held {
                into,
                height,
                items,
            } => (*into, *height, items),
        };
        // Items still there as they were move as the item read does: rows arrive only at the
        // row's front and end, calls only at a group's. The last of them that started at or
        // above the reader holds them (a call of an opened group rather than the group), else
        // the first. A row whose column of items does not report its place counts its items
        // from the column's top, so one starting just under the reader can count as above; it
        // moves as the reader's does all the same.
        let same = items.iter().filter_map(|(key, was_top, was_high)| {
            let (now_top, now_high) = now(key)?;
            ((now_high - was_high).abs() < 0.5).then_some((*was_top, now_top))
        });
        let held = same
            .clone()
            .filter(|(was_top, _)| *was_top <= into)
            .last()
            .or_else(|| same.clone().next());
        match held {
            Some((was_top, now_top)) => top + into + (now_top - was_top),
            // No item to hold: what is below the reader is kept, as rows mostly arrive above.
            None => top + height - (was - into),
        }
    }
}

/// Where an item (or a part items are drawn in) of a row is placed: in the part or item
/// `within`, by its key, else in the row.
struct Item {
    row: String,
    within: Option<String>,
    token: u64,
    top: f32,
    height: f32,
}

impl Item {
    fn new(row: &str, within: Option<&str>, token: u64, top: f32, height: f32) -> Self {
        Self {
            row: row.to_owned(),
            within: within.map(str::to_owned),
            token,
            top,
            height,
        }
    }
}

#[derive(Default)]
struct Rows {
    /// Top of the transcript within the scroll content (a child header can precede it).
    origin: Option<(u64, f32)>,
    /// Each row's top and height in the transcript, with the token of the render that placed
    /// it.
    tops: BTreeMap<String, (u64, f32, f32)>,
    /// Each keyed item's place, the reader held by them.
    items: BTreeMap<String, Item>,
    /// The parts items are drawn in (a message's column of items, an opened group and its
    /// rows), by key: they only place the items within them.
    parts: BTreeMap<String, Item>,
    /// A reading position to restore once its row is placed, and whether it holds the reader
    /// while rows arrive above (rather than reopening where they left).
    pending: Option<(String, Mark, ElementRef, bool)>,
}

impl Rows {
    fn prefix(&self) -> f32 {
        self.origin.map_or(0.0, |(_, y)| y)
    }

    /// Where item `key` of row `row` is in the row (top and height), through the parts (and
    /// items) it is drawn in; none while one of them is not placed.
    fn item(&self, row: &str, key: &str) -> Option<(f32, f32)> {
        let item = self.items.get(key).filter(|item| item.row == row)?;
        let mut top = item.top;
        let mut within = item.within.as_deref();
        while let Some(key) = within {
            let part = self.parts.get(key).or_else(|| self.items.get(key));
            let part = part.filter(|part| part.row == row)?;
            top += part.top;
            within = part.within.as_deref();
        }
        Some((top, item.height))
    }
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

pub(super) fn set_origin(scope: &str, token: u64, y: f32) {
    SCOPES.with(|scopes| {
        scopes.borrow_mut().entry(scope.into()).or_default().origin = Some((token, y))
    });
}

fn clear_origin(scope: &str, token: u64) {
    SCOPES.with(|scopes| {
        if let Some(rows) = scopes.borrow_mut().get_mut(scope)
            && rows.origin.is_some_and(|(owner, _)| owner == token)
        {
            rows.origin = None;
        }
    });
}

/// Mount this view directly in the scroll content, around the transcript but after
/// anything that precedes it. Its parent-relative layout gives the conversion in
/// both directions between transcript-local rows and scroll-content offsets.
pub(crate) fn content_origin(scope: String, body: View) -> View {
    let token = token();
    let mounted = scope.clone();
    cleanup(move || clear_origin(&mounted, token));
    body.on_layout(move |frame: &Bounds<Points>| set_origin(&scope, token, frame.origin.y.0))
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
    if let Some((anchor, mark, scroller, _)) = restore {
        // Read where the row and its items are once this layout's places are all in: the
        // row's items report theirs after it.
        let scope = scope.to_owned();
        after_answer(move || {
            if let Some(at) = offset(&scope, &anchor, &mark) {
                scroller.scroll_to_offset(px(at.max(0.0)), false);
            }
        });
    }
}

/// The scroll offset of `mark` in row `anchor` of `scope`, as the row and its items are placed
/// now; none while the row is not.
pub(super) fn offset(scope: &str, anchor: &str, mark: &Mark) -> Option<f32> {
    SCOPES.with(|scopes| {
        let scopes = scopes.borrow();
        let rows = scopes.get(scope)?;
        let (_, top, height) = rows.tops.get(anchor)?;
        Some(rows.prefix() + mark.offset(*top, *height, |key| rows.item(anchor, key)))
    })
}

/// Item `key` of row `row` of `scope` starts `top` into the part `within` (else into the row)
/// and is `height` high.
pub(super) fn item_placed(
    scope: &str,
    row: &str,
    key: &str,
    within: Option<&str>,
    token: u64,
    top: f32,
    height: f32,
) {
    let item = Item::new(row, within, token, top, height);
    place(scope, key, item, false);
}

/// Part `key` of row `row` of `scope`, that items are drawn in, starts `top` into the part
/// `within` (else into the row) and is `height` high.
pub(super) fn part_placed(
    scope: &str,
    row: &str,
    key: &str,
    within: Option<&str>,
    token: u64,
    top: f32,
    height: f32,
) {
    let part = Item::new(row, within, token, top, height);
    place(scope, key, part, true);
}

fn place(scope: &str, key: &str, item: Item, part: bool) {
    SCOPES.with(|scopes| {
        let mut scopes = scopes.borrow_mut();
        let rows = scopes.entry(scope.to_owned()).or_default();
        let placed = if part {
            &mut rows.parts
        } else {
            &mut rows.items
        };
        placed.insert(key.to_owned(), item);
    });
}

/// The render `token` of item or part `key` is gone.
pub(super) fn item_forget(scope: &str, key: &str, token: u64) {
    SCOPES.with(|scopes| {
        let mut scopes = scopes.borrow_mut();
        if let Some(rows) = scopes.get_mut(scope) {
            for placed in [&mut rows.items, &mut rows.parts] {
                if placed.get(key).is_some_and(|item| item.token == token) {
                    placed.remove(key);
                }
            }
        }
    });
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
        let top = rows
            .tops
            .get(&anchor)
            .map(|(_, top, _)| rows.prefix() + *top);
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
/// was last placed no longer holds. Kept on the item read, by its key, which replies joining
/// the row below it in the same update leave where it was too.
pub(super) fn hold(scope: &str, anchor: String, offset: f32, scroller: ElementRef) {
    let mark = mark(scope, &anchor, offset);
    SCOPES.with(|scopes| {
        let mut scopes = scopes.borrow_mut();
        let rows = scopes.entry(scope.to_owned()).or_default();
        rows.pending = Some((anchor, mark, scroller, true));
    });
}

/// The reader `offset` into row `anchor` of `scope`, by the row and its items as last placed;
/// the row's place is forgotten, as rows were added above it.
pub(super) fn mark(scope: &str, anchor: &str, offset: f32) -> Mark {
    SCOPES.with(|scopes| {
        let mut scopes = scopes.borrow_mut();
        let rows = scopes.entry(scope.to_owned()).or_default();
        match rows.tops.remove(anchor) {
            Some((_, _, height)) => {
                let mut items: Vec<_> = rows
                    .items
                    .iter()
                    .filter(|(_, item)| item.row == anchor)
                    .filter_map(|(key, _)| {
                        let (top, height) = rows.item(anchor, key)?;
                        Some((key.clone(), top, height))
                    })
                    .collect();
                items.sort_by(|a, b| a.1.total_cmp(&b.1));
                Mark::Held {
                    into: offset,
                    height,
                    items,
                }
            }
            None => Mark::Top(offset),
        }
    })
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
        let rows = scopes.get(scope)?;
        let offset = offset - rows.prefix();
        let tops = rows.tops.iter().map(|(anchor, (_, top, _))| (anchor, *top));
        tops.clone()
            .filter(|(_, top)| *top <= offset + 1.0)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .or_else(|| tops.min_by(|a, b| a.1.total_cmp(&b.1)))
            .map(|(anchor, top)| (anchor.clone(), offset - top))
    })
}
