//! Where each message of a mounted transcript starts, by its anchor, per session scope: the
//! rows report their place as they are laid out, the view following the transcript reads which
//! row the reader is at from its scroll offset, and each view scrolled up from its end holds
//! its reader there (at any width): every later layout that moves their row, or what is above
//! them in it, scrolls the view back to them (rows of a page above filling in after it showed,
//! too), until the reader scrolls on their own, follows the end or the view goes. The items of
//! a recorded message report their place in it too, by their stable keys, so the reader stays
//! on the item they read while rows arrive above and below it in the same update. An opened
//! group's calls report theirs by their call ids, through the parts they are drawn in (the
//! group's rows in the group, the message's column of items in it), so the reader stays on the
//! call they read while calls join the group before and after it.

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
    /// This far below the row's top, its items as they are placed now (key, top, height; as
    /// for `Held`): the reader held as rows above, or items above theirs, grow or shrink later.
    /// The reader moves with the last item that started at or above them, whatever its height
    /// does (what grows in it grows below its top), else with the row's top.
    Kept {
        into: f32,
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
            Self::Kept { into, items } => {
                let held = items
                    .iter()
                    .filter(|(_, was_top, _)| *was_top <= *into)
                    .rev()
                    .find_map(|(key, was_top, _)| Some((*was_top, now(key)?.0)));
                return top + into + held.map_or(0.0, |(was_top, now_top)| now_top - was_top);
            }
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
    /// Where each view of the scope scrolled up from its end holds its reader, by the view.
    holds: BTreeMap<u64, Hold>,
    /// A layout correction issued to that scroller, until its next scroll event.
    restores: BTreeMap<u64, f32>,
    /// Places were reported since the holds were last settled.
    dirty: bool,
}

/// A reader a view holds in place, scrolled up from its end, until they scroll on their own,
/// the view follows the end again or shows another scope, or it goes. Every layout that moves
/// their row or the items in it scrolls the view back to them, then takes their place again
/// from that layout, so later layouts are read against the one the reader saw last.
struct Hold {
    /// The row the reader is at.
    anchor: String,
    /// Where in that row.
    mark: Mark,
    /// The view's scroll offset, as it last reported it or was last scrolled to; none until
    /// either.
    at: Option<f32>,
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

    /// The row at scroll offset `offset` and how far into it, as `at` says.
    fn at(&self, offset: f32) -> Option<(String, f32)> {
        let offset = offset - self.prefix();
        let tops = self.tops.iter().map(|(anchor, (_, top, _))| (anchor, *top));
        tops.clone()
            .filter(|(_, top)| *top <= offset + 1.0)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .or_else(|| tops.min_by(|a, b| a.1.total_cmp(&b.1)))
            .map(|(anchor, top)| (anchor.clone(), offset - top))
    }

    /// The placed items of row `row` (key, top, height in the row), sorted by top.
    fn items_of(&self, row: &str) -> Vec<(String, f32, f32)> {
        let mut items: Vec<_> = self
            .items
            .iter()
            .filter(|(_, item)| item.row == row)
            .filter_map(|(key, _)| {
                let (top, height) = self.item(row, key)?;
                Some((key.clone(), top, height))
            })
            .collect();
        items.sort_by(|a, b| a.1.total_cmp(&b.1));
        items
    }

    /// A place was reported: the holds read it once this layout's places are all in.
    fn touched(&mut self) -> bool {
        self.dirty |= !self.holds.is_empty();
        self.dirty
    }
}

/// Settles `scope`'s holds after this layout when `touched` says they wait for it. Each report
/// asks, as the task runs only while the owner that asked lives (a row can go first); the
/// first to run settles and the others find nothing to do. Only views that scroll hold
/// readers, so with none mounted there is nothing to ask.
fn queue(scope: &str, touched: bool) {
    if touched && SCROLLERS.with(|scrollers| !scrollers.borrow().is_empty()) {
        let scope = scope.to_owned();
        after_answer(move || {
            for (view, at) in settle(&scope) {
                let scroller = SCROLLERS.with(|scrollers| scrollers.borrow().get(&view).copied());
                if let Some(scroller) = scroller {
                    scroller.scroll_to_offset(px(at), false);
                }
            }
        });
    }
}

thread_local! {
    static SCOPES: RefCell<BTreeMap<String, Rows>> = const { RefCell::new(BTreeMap::new()) };
    static TOKENS: Cell<u64> = const { Cell::new(0) };
    /// What each mounted view holding readers scrolls, by the view.
    static SCROLLERS: RefCell<BTreeMap<u64, ElementRef>> = const { RefCell::new(BTreeMap::new()) };
}

/// A view that holds its readers by scrolling `scroller`, for as long as the current owner
/// lives: the token its holds go by.
pub(super) fn view(scroller: ElementRef) -> u64 {
    let view = token();
    SCROLLERS.with(|scrollers| scrollers.borrow_mut().insert(view, scroller));
    cleanup(move || {
        SCROLLERS.with(|scrollers| scrollers.borrow_mut().remove(&view));
    });
    view
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
    let touched = SCOPES.with(|scopes| {
        let mut scopes = scopes.borrow_mut();
        let rows = scopes.entry(scope.into()).or_default();
        rows.origin = Some((token, y));
        rows.touched()
    });
    queue(scope, touched);
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

/// Row `anchor` of `scope` starts at `top` and is `height` high; the readers held in the scope
/// are kept where they were once this layout's places are all in (the row's items report
/// theirs after it).
pub(super) fn placed(scope: &str, anchor: &str, token: u64, top: f32, height: f32) {
    let touched = SCOPES.with(|scopes| {
        let mut scopes = scopes.borrow_mut();
        let rows = scopes.entry(scope.to_owned()).or_default();
        rows.tops.insert(anchor.to_owned(), (token, top, height));
        rows.touched()
    });
    queue(scope, touched);
}

/// The scroll offset of `mark` in row `anchor` of `scope`, as the row and its items are placed
/// now; none while the row is not.
#[cfg(test)]
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
    let touched = SCOPES.with(|scopes| {
        let mut scopes = scopes.borrow_mut();
        let rows = scopes.entry(scope.to_owned()).or_default();
        let placed = if part {
            &mut rows.parts
        } else {
            &mut rows.items
        };
        placed.insert(key.to_owned(), item);
        rows.touched()
    });
    queue(scope, touched);
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

/// View `view` of `scope` holds its reader `offset` into row `anchor`:
/// it scrolls there once the row is placed (now when it is), and keeps them there.
pub(super) fn restore(scope: &str, view: u64, anchor: String, offset: f32) {
    let touched = SCOPES.with(|scopes| {
        let mut scopes = scopes.borrow_mut();
        let rows = scopes.entry(scope.to_owned()).or_default();
        let hold = Hold {
            anchor,
            mark: Mark::Top(offset),
            at: None,
        };
        rows.holds.insert(view, hold);
        rows.touched()
    });
    queue(scope, touched);
}

/// View `view` of `scope` holds its reader `offset` into row `anchor`, scrolling back there
/// once the row is placed again: rows were added above the reader, before the row or at its
/// own front, so where it was last placed no longer holds. Kept on the item read, by its key,
/// which replies joining the row below it in the same update leave where it was too.
pub(super) fn hold(scope: &str, view: u64, anchor: String, offset: f32) {
    let mark = mark(scope, &anchor, offset);
    SCOPES.with(|scopes| {
        let mut scopes = scopes.borrow_mut();
        let rows = scopes.entry(scope.to_owned()).or_default();
        let at = rows.holds.get(&view).and_then(|hold| hold.at);
        let hold = Hold { anchor, mark, at };
        rows.holds.insert(view, hold);
    });
}

/// The reader `offset` into row `anchor` of `scope`, by the row and its items as last placed;
/// the row's place is forgotten, as rows were added above it.
pub(super) fn mark(scope: &str, anchor: &str, offset: f32) -> Mark {
    SCOPES.with(|scopes| {
        let mut scopes = scopes.borrow_mut();
        let rows = scopes.entry(scope.to_owned()).or_default();
        match rows.tops.remove(anchor) {
            Some((_, _, height)) => Mark::Held {
                into: offset,
                height,
                items: rows.items_of(anchor),
            },
            None => Mark::Top(offset),
        }
    })
}

/// View `view` of `scope` holds its reader no more: it follows the end, shows another scope or
/// went.
pub(super) fn release(scope: &str, view: u64) {
    SCOPES.with(|scopes| {
        if let Some(rows) = scopes.borrow_mut().get_mut(scope) {
            rows.holds.remove(&view);
            rows.restores.remove(&view);
        }
    });
}

/// Was this scroll the correction just requested by layout, rather than an edge gesture?
/// Consume even a mismatch: a later user scroll to that offset is not the old correction.
pub(super) fn restored(scope: &str, holder: u64, offset: f32) -> bool {
    SCOPES.with(|scopes| {
        scopes
            .borrow_mut()
            .get_mut(scope)
            .and_then(|rows| rows.restores.remove(&holder))
            .is_some_and(|at| (at - offset).abs() <= 1.0)
    })
}

/// A reader's geometry is private to its mount; do not retain it after disposal.
pub(super) fn forget_geometry(scope: &str) {
    SCOPES.with(|scopes| scopes.borrow_mut().remove(scope));
}

/// View `view` of `scope`, scrolled up from its end, is at `offset`.
/// Unless it is where the view was last scrolled to, the reader scrolled on their own: they
/// are held where they are now, by the row they are at and its items as placed now, and a
/// place held from before (a row still to be placed again, as rows arrived above) holds
/// nothing any more.
pub(super) fn scrolled(scope: &str, view: u64, offset: f32) {
    SCOPES.with(|scopes| {
        let mut scopes = scopes.borrow_mut();
        let rows = scopes.entry(scope.to_owned()).or_default();
        let ours = rows.holds.get(&view).and_then(|hold| hold.at);
        if ours.is_some_and(|at| (at - offset).abs() <= 1.0) {
            return;
        }
        match rows.at(offset) {
            Some((anchor, into)) => {
                let items = rows.items_of(&anchor);
                let hold = Hold {
                    anchor,
                    mark: Mark::Kept { into, items },
                    at: Some(offset),
                };
                rows.holds.insert(view, hold);
            }
            None => {
                rows.holds.remove(&view);
            }
        }
    });
}

/// Settles the holds of `scope` after a layout that reported places: each view whose reader's
/// row is placed and moved (with what is above them in it) is scrolled back to them, given as
/// the view and the offset; then where every held reader is, is taken again from this
/// layout. Nothing when no place was reported since the last time.
pub(super) fn settle(scope: &str) -> Vec<(u64, f32)> {
    SCOPES.with(|scopes| {
        let mut scopes = scopes.borrow_mut();
        let Some(rows) = scopes.get_mut(scope) else {
            return Vec::new();
        };
        if !std::mem::take(&mut rows.dirty) {
            return Vec::new();
        }
        let prefix = rows.prefix();
        let mut moves = Vec::new();
        let views: Vec<u64> = rows.holds.keys().copied().collect();
        for view in views {
            let hold = &rows.holds[&view];
            let Some(&(_, top, height)) = rows.tops.get(&hold.anchor) else {
                continue;
            };
            let now = |key: &str| rows.item(&hold.anchor, key);
            let at = (prefix + hold.mark.offset(top, height, now)).max(0.0);
            let items = rows.items_of(&hold.anchor);
            let Some(hold) = rows.holds.get_mut(&view) else {
                continue;
            };
            hold.mark = Mark::Kept {
                into: at - prefix - top,
                items,
            };
            if hold.at.is_none_or(|was| (was - at).abs() > 0.5) {
                hold.at = Some(at);
                rows.restores.insert(view, at);
                moves.push((view, at));
            }
        }
        moves
    })
}

/// Source record at the viewport edge, without disturbing layout holds. Consecutive
/// records can share a bubble; the stable item key names its own source record.
/// Nested tool rows without a source key are covered by the outer item's placement.
pub(super) fn source_at(scope: &str, offset: f32) -> Option<String> {
    SCOPES.with(|scopes| {
        let scopes = scopes.borrow();
        let rows = scopes.get(scope)?;
        let (anchor, into) = rows.at(offset)?;
        let source = rows
            .items_of(&anchor)
            .into_iter()
            .rev()
            .find_map(|(key, top, _)| {
                let (source, suffix) = key.rsplit_once('|')?;
                let ordinal = suffix.split('/').next()?;
                (top <= into && ordinal.parse::<usize>().is_ok()).then(|| source.to_owned())
            });
        Some(source.unwrap_or(anchor))
    })
}

/// The row of `scope` the reader is at with the view scrolled to `offset`: the last row that
/// starts at or above it, and how far into that row the view starts. Above the first row (the
/// control for earlier messages) it is the first row, the view starting before it, so rows
/// that arrive above keep it in place too.
pub(super) fn at(scope: &str, offset: f32) -> Option<(String, f32)> {
    SCOPES.with(|scopes| scopes.borrow().get(scope)?.at(offset))
}
