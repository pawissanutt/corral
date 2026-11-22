//! Following the newest rows, and the way back to them. The transcript's scroll keeps to its
//! end while the reader is there; scrolling up pauses that without moving the reading position
//! as rows arrive. While paused, a control floats at the bottom right of the transcript and
//! counts what arrived: `↓ 37 new rows · End` (`↓ Jump to latest` when nothing arrived; the
//! phone's `↓ 37 new · Latest`). Pressing it returns to the end and following resumes. Where
//! the reader is (the end, or a row and how far into it) is kept per session scope when the
//! view closes, and every view of that scope opens there again. Scrolling up near the first
//! row asks for the page before it (`older`); rows that arrive above the reader keep the row
//! they read where it was and are not counted as new, also when they are the earlier items of
//! the record the reader is in. Past the history cap the loaded pages no longer join the newest
//! one: the control then stays at the end too, reads `↓ Load latest messages`, and it and every
//! jump to the end read the latest messages again.

use std::cell::RefCell;
use std::rc::Rc;

use nsh_core::prelude::*;

use crate::model::session::transcript::{Following, Reading, Resume};
use crate::model::{Form, Id};
use crate::state::AppState;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::super::run;
use super::anchors;
use super::older::{Earlier, Reach};
use super::watch;

/// How close to the end still counts as at the end.
const END_SLACK: f32 = 24.0;

/// What the control says, by how many rows arrived since the reader left the end; past the
/// history cap the loaded pages no longer join the newest, and it reads the latest instead.
fn words(form: Form, arrived: usize, latest: bool) -> String {
    match (form, arrived) {
        (Form::Phone, _) if latest => "↓ Load latest".to_owned(),
        _ if latest => "↓ Load latest messages".to_owned(),
        (_, 0) => "↓ Jump to latest".to_owned(),
        (Form::Phone, n) => format!("↓ {n} new · Latest"),
        (_, 1) => "↓ 1 new row · End".to_owned(),
        (_, n) => format!("↓ {n} new rows · End"),
    }
}

/// Keeps `reading` as where the reader is in `scope`'s transcript.
fn save(app: AppState, scope: Id, reading: Reading) {
    let known = app
        .session
        .with_untracked(|state| state.ui(scope.as_str()).reading == reading);
    if !known {
        app.session.update(|state| {
            state.sessions.entry(scope).or_default().reading = reading;
        });
    }
}

/// Where the reader is in the scope a view shows. A plain value shared by the view's effects,
/// its scroll handler and its cleanup, never a signal or `stored` of the view's owner: NSH frees
/// what an owner made before it runs that owner's cleanups, so the cleanup that saves the place
/// as the view goes could not read those any more.
#[derive(Debug, Default)]
pub(super) struct Place {
    pub scope: Option<Id>,
    pub at_end: bool,
    /// The row count when the reader left the end.
    pub left_at: Option<usize>,
    /// The row the reader is at, and how far into it.
    pub top: Option<(String, f32)>,
    /// The transcript's row count as last seen.
    pub rows: usize,
    /// The first row shown as last seen, with its scope: when it changes in the same scope,
    /// rows arrived above the reader.
    pub first: Option<(Id, String)>,
    pub tail: Option<(String, usize)>,
    /// The scroll offset as last seen: a scroll that changes it is the reader's own.
    pub offset: Option<f32>,
}

/// What rows arriving mean for the reader.
#[derive(Debug, PartialEq)]
pub(super) enum Shown {
    /// The reader is at the end, which follows them.
    End,
    /// The reader is scrolled up: their row (scope, anchor, offset into it) stays where it is
    /// on screen when rows arrived above, and the control counts what arrived below.
    Up {
        held: Option<(Id, String, f32)>,
        arrived: usize,
    },
}

impl Place {
    /// The transcript now shows `rows` rows, `first` (with its scope) the first.
    pub fn shown(
        &mut self,
        rows: usize,
        first: Option<(Id, String)>,
        tail: Option<(String, usize)>,
        below: impl FnOnce(&str) -> Option<usize>,
    ) -> Shown {
        let previous_tail = std::mem::replace(&mut self.tail, tail);
        let before = std::mem::replace(&mut self.rows, rows);
        let above = matches!(
            (&self.first, &first),
            (Some((was, old)), Some((scope, new))) if was == scope && old != new
        );
        self.first = first;
        if self.at_end {
            self.left_at = None;
            return Shown::End;
        }
        let mut left = *self.left_at.get_or_insert(rows);
        // What arrived above is not new: the reader read past it on their way up.
        if above {
            let added_below = previous_tail
                .and_then(|(key, old)| below(&key).map(|now| now.saturating_sub(old)))
                .unwrap_or_default();
            left += rows.saturating_sub(before).saturating_sub(added_below);
            self.left_at = Some(left);
        }
        let held = above
            .then(|| self.scope.clone().zip(self.top.clone()))
            .flatten()
            .map(|(scope, (key, offset))| (scope, key, offset));
        Shown::Up {
            held,
            arrived: rows.saturating_sub(left),
        }
    }

    pub fn reading(&self) -> Reading {
        match (&self.top, self.at_end) {
            (Some((key, offset)), false) => Reading::At {
                key: key.clone(),
                offset: offset.round() as i32,
                rows: self.left_at.unwrap_or(self.rows),
            },
            _ => Reading::End,
        }
    }

    /// The scope shown and where the reader is in it, as the view leaves it.
    pub fn leave(&mut self) -> Option<(Id, Reading)> {
        let reading = self.reading();
        self.scope.take().map(|scope| (scope, reading))
    }
}

/// Saves where the reader is, through `save`, when the current owner (the view) goes.
pub(super) fn kept(place: Rc<RefCell<Place>>, save: impl Fn(Id, Reading) + 'static) {
    cleanup(move || {
        let left = place.borrow_mut().leave();
        if let Some((scope, reading)) = left {
            save(scope, reading);
        }
    });
}

/// `surface` (the transcript's scroll) kept at its newest rows while the reader is at the end,
/// with the jump control over it while they are not. `following` is what the shown transcript
/// says (`Transcript::following`: its scope, its row count, where its reader left it), tracked:
/// a new scope opens where its reader left it, each new row scrolls to the end while
/// following. `earlier` is what the transcript offers before its first row
/// (`Earlier::of`), tracked: scrolling up near the top asks for that page once. `jump` is the
/// page's jump request (End, Ctrl+End, the catalog's Jump to latest), tracked: each change does
/// what pressing the control does. Where the reader is, is saved when the view shows another
/// scope and when it goes.
pub fn follow(
    app: AppState,
    surface: Scroll,
    following: impl Fn() -> Option<Following> + 'static,
    earlier: impl Fn() -> Option<Earlier> + 'static,
    below: impl Fn(&str) -> Option<usize> + 'static,
    jump: impl Fn() -> u32 + 'static,
) -> AnyElement {
    let form = app.form;
    let scroller = element_ref();
    // Draws the jump control; `Place::at_end` says the same for the cleanup.
    let at_end = signal(true);
    // What the control counts: rows that arrived below since the reader left the end.
    let arrived = signal(0usize);
    let place = Rc::new(RefCell::new(Place {
        at_end: true,
        ..Place::default()
    }));
    let reach = Rc::new(RefCell::new(Reach::default()));
    let shown = computed(following);
    let earlier = computed(earlier);
    let count = computed(move || shown.with(|now| now.as_ref().map_or(0, |now| now.rows)));
    let scope_of = computed(move || shown.with(|now| now.as_ref().map(|now| now.scope.clone())));
    let first = computed(move || earlier.with(|now| now.as_ref()?.first.clone()));
    let latest = computed(move || {
        scope_of
            .get()
            .is_some_and(|id| watch::needs_latest(app, &id))
    });
    // Back to the end; past the history cap that reads the latest messages first.
    let to_latest = move || {
        if latest.get_untracked()
            && let Some(id) = scope_of.get_untracked()
        {
            untracked(|| watch::latest(app, &id));
        }
    };
    {
        let place = place.clone();
        let reach = reach.clone();
        effect(move || {
            let Some(id) = scope_of.get() else {
                return;
            };
            if place.borrow().scope.as_ref() == Some(&id) {
                return;
            }
            let left = place.borrow_mut().leave();
            if let Some((old, reading)) = left {
                untracked(|| save(app, old, reading));
            }
            let resume = shown.with_untracked(|now| now.as_ref().map(|now| now.resume.clone()));
            reach.borrow_mut().reset();
            arrived.set(0);
            let mut known = place.borrow_mut();
            known.scope = Some(id.clone());
            known.top = None;
            known.offset = None;
            match resume {
                Some(Resume::Row { key, offset, rows }) => {
                    known.at_end = false;
                    known.left_at = Some(rows);
                    known.top = Some((key.clone(), offset as f32));
                    drop(known);
                    arrived.set(count.get_untracked().saturating_sub(rows));
                    at_end.set(false);
                    anchors::restore(id.as_str(), key, offset as f32, scroller);
                }
                _ => {
                    known.at_end = true;
                    known.left_at = None;
                    drop(known);
                    anchors::cancel(id.as_str());
                    at_end.set(true);
                    after_answer(move || scroller.scroll_to_end(false));
                }
            }
        });
    }
    kept(place.clone(), move |scope, reading| {
        save(app, scope, reading)
    });
    {
        let place = place.clone();
        effect(move || {
            let now = count.get();
            let top = scope_of.get().zip(first.get());
            let tail = shown.with(|view| view.as_ref().and_then(|view| view.tail.clone()));
            let shown = place
                .borrow_mut()
                .shown(now, top, tail, |key| untracked(|| below(key)));
            match shown {
                Shown::End => after_answer(move || scroller.scroll_to_end(false)),
                Shown::Up { held, arrived: n } => {
                    if let Some((scope, key, offset)) = held {
                        anchors::hold(scope.as_str(), key, offset, scroller);
                    }
                    arrived.set(n);
                }
            }
        });
    }
    {
        let place = place.clone();
        let reach = reach.clone();
        effect(move || {
            let asked = earlier.with(|now| {
                let end = place.borrow().at_end;
                reach.borrow_mut().changed(now.as_ref(), end)
            });
            if let Some(older) = asked {
                untracked(|| run(app, older));
            }
        });
    }
    let asked = computed(jump);
    let answered = stored(asked.get_untracked());
    {
        let place = place.clone();
        effect(move || {
            let now = asked.get();
            if answered.replace(now) != now {
                to_latest();
                {
                    let mut known = place.borrow_mut();
                    known.at_end = true;
                    known.left_at = None;
                }
                at_end.set(true);
                after_answer(move || scroller.scroll_to_end(true));
            }
        });
    }
    let scrolled = place.clone();
    let surface = surface
        .element_ref(&scroller)
        .on_scroll(move |event: &ScrollEvent| {
            let offset = event.offset.y.0;
            let below = event.content_size.height.0 - offset - event.size.height.0;
            let end = below <= END_SLACK;
            let (flipped, scope, moved) = {
                let mut known = scrolled.borrow_mut();
                let moved = known
                    .offset
                    .replace(offset)
                    .is_some_and(|was| (was - offset).abs() > 0.5);
                if let Some(at) = known
                    .scope
                    .as_ref()
                    .map(|id| anchors::at(id.as_str(), offset))
                {
                    known.top = at;
                }
                let flipped = end != known.at_end;
                if flipped {
                    if !end {
                        known.left_at = Some(count.get_untracked());
                    }
                    known.at_end = end;
                }
                (flipped, known.scope.clone(), moved)
            };
            if moved && let Some(scope) = &scope {
                anchors::moved(scope.as_str());
            }
            if flipped {
                if end && let Some(scope) = scope {
                    anchors::cancel(scope.as_str());
                }
                arrived.set(0);
                at_end.set(end);
            }
            let older = earlier.with_untracked(|now| {
                reach
                    .borrow_mut()
                    .scrolled(now.as_ref(), offset, event.size.height.0, end)
            });
            if let Some(older) = older {
                run(app, older);
            }
        });
    let control = dynamic(move || {
        let capped = latest.get();
        if at_end.get() && !capped {
            return None;
        }
        let label = words(form, arrived.get(), capped);
        let words = text(label.clone()).text_color(color(Token::Ink));
        let words = if form == Form::Terminal {
            words.font_weight(FontWeight::BOLD)
        } else {
            words.font_family(FONT_TEXT).text_size(px(12.0))
        };
        let pressed = place.clone();
        let control = view()
            .test_id("btn-jump-latest")
            .role(AccessibilityRole::Button)
            .label(label)
            .cursor_pointer()
            .on_press(move || {
                to_latest();
                {
                    let mut known = pressed.borrow_mut();
                    known.at_end = true;
                    known.left_at = None;
                }
                at_end.set(true);
                scroller.scroll_to_end(true);
            })
            .child(words);
        let control = if form == Form::Terminal {
            control.px(term::cells(1.0)).bg(color(Token::Surface2))
        } else {
            control
                .px(px(12.0))
                .py(px(6.0))
                .rounded(px(14.0))
                .border_1()
                .border_color(color(Token::Rule2))
                .bg(color(Token::Surface))
                .hover(|s| s.bg(color(Token::Hover)))
                .focus_visible(|s| {
                    s.outline(px(2.0))
                        .outline_color(color(Token::Focus))
                        .outline_offset(px(1.0))
                })
        };
        let inset = if form == Form::Terminal {
            term::cells(1.0)
        } else {
            px(16.0)
        };
        Some(
            view()
                .absolute()
                .right(inset)
                .bottom(if form == Form::Terminal {
                    px(0.0)
                } else {
                    px(12.0)
                })
                .child(control)
                .into_element(),
        )
    });
    view()
        .relative()
        .flex_1()
        .min_h(px(0.0))
        .child(surface)
        .child(control)
        .into_element()
}
