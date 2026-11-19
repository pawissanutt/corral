//! Following the newest rows, and the way back to them. The transcript's scroll keeps to its
//! end while the reader is there; scrolling up pauses that without moving the reading position
//! as rows arrive. While paused, a control floats at the bottom right of the transcript and
//! counts what arrived: `↓ 37 new rows · End` (`↓ Jump to latest` when nothing arrived; the
//! phone's `↓ 37 new · Latest`). Pressing it returns to the end and following resumes. Where
//! the reader is (the end, or a row and how far into it) is kept per session scope when the
//! view closes, and every view of that scope opens there again.

use std::cell::RefCell;
use std::rc::Rc;

use nsh_core::prelude::*;

use crate::model::session::transcript::{Following, Reading, Resume};
use crate::model::{Form, Id};
use crate::state::AppState;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::anchors;

/// How close to the end still counts as at the end.
const END_SLACK: f32 = 24.0;

/// What the control says, by how many rows arrived since the reader left the end.
fn words(form: Form, arrived: usize) -> String {
    match (form, arrived) {
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
}

impl Place {
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
/// following. `jump` is the page's jump request (End, Ctrl+End, the catalog's Jump to latest),
/// tracked: each change does what pressing the control does. Where the reader is, is saved when
/// the view shows another scope and when it goes.
pub fn follow(
    app: AppState,
    surface: Scroll,
    following: impl Fn() -> Option<Following> + 'static,
    jump: impl Fn() -> u32 + 'static,
) -> AnyElement {
    let form = app.form;
    let scroller = element_ref();
    // Draws the jump control; `Place::at_end` says the same for the cleanup.
    let at_end = signal(true);
    let place = Rc::new(RefCell::new(Place {
        at_end: true,
        ..Place::default()
    }));
    let shown = computed(following);
    let count = computed(move || shown.with(|now| now.as_ref().map_or(0, |now| now.rows)));
    let scope_of = computed(move || shown.with(|now| now.as_ref().map(|now| now.scope.clone())));
    {
        let place = place.clone();
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
            let mut known = place.borrow_mut();
            known.scope = Some(id.clone());
            known.top = None;
            match resume {
                Some(Resume::Row { key, offset, rows }) => {
                    known.at_end = false;
                    known.left_at = Some(rows);
                    known.top = Some((key.clone(), offset as f32));
                    drop(known);
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
            let mut known = place.borrow_mut();
            known.rows = now;
            if known.at_end {
                known.left_at = None;
                drop(known);
                after_answer(move || scroller.scroll_to_end(false));
            } else if known.left_at.is_none() {
                known.left_at = Some(now);
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
            let flipped = {
                let mut known = scrolled.borrow_mut();
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
                flipped
            };
            if flipped {
                at_end.set(end);
            }
        });
    let control = dynamic(move || {
        if at_end.get() {
            return None;
        }
        let left_at = place.borrow().left_at;
        let arrived = count
            .get()
            .saturating_sub(left_at.unwrap_or_else(|| count.get_untracked()));
        let label = words(form, arrived);
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
