//! Following the newest rows, and the way back to them. The transcript's scroll keeps to its
//! end while the reader is there; scrolling up pauses that without moving the reading position
//! as rows arrive. While paused, a control floats at the bottom right of the transcript and
//! counts what arrived: `↓ 37 new rows · End` (`↓ Jump to latest` when nothing arrived; the
//! phone's `↓ 37 new · Latest`). Pressing it returns to the end and following resumes. Where
//! the reader is (the end, or a row and how far into it) is kept per session scope when the
//! view closes, and every view of that scope opens there again.

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

/// `surface` (the transcript's scroll) kept at its newest rows while the reader is at the end,
/// with the jump control over it while they are not. `following` is what the shown transcript
/// says (`Transcript::following`: its scope, its row count, where its reader left it), tracked:
/// a new scope opens where its reader left it, each new row scrolls to the end while
/// following. `jump` is the page's jump request (End, Ctrl+End, the catalog's Jump to latest),
/// tracked: each change does what pressing the control does.
pub fn follow(
    app: AppState,
    surface: Scroll,
    following: impl Fn() -> Option<Following> + 'static,
    jump: impl Fn() -> u32 + 'static,
) -> AnyElement {
    let form = app.form;
    let scroller = element_ref();
    let at_end = signal(true);
    // The row count when the reader left the end.
    let left_at = stored(None::<usize>);
    // The scope shown, and the row the reader is at in it.
    let scope = stored(None::<Id>);
    let top = stored(None::<(String, f32)>);
    let shown = computed(following);
    let count = computed(move || shown.with(|now| now.as_ref().map_or(0, |now| now.rows)));
    let rows_seen = stored(0usize);
    // Where the reader is now, as kept when the view closes or shows another scope.
    let reading = move || -> Reading {
        if at_end.try_get().unwrap_or(true) {
            return Reading::End;
        }
        match top.try_with(|top| top.clone()).flatten() {
            Some((key, offset)) => Reading::At {
                key,
                offset: offset.round() as i32,
                rows: left_at
                    .try_with(|left| *left)
                    .flatten()
                    .or_else(|| rows_seen.try_with(|rows| *rows))
                    .unwrap_or(0),
            },
            None => Reading::End,
        }
    };
    let scope_of = computed(move || shown.with(|now| now.as_ref().map(|now| now.scope.clone())));
    effect(move || {
        let Some(id) = scope_of.get() else {
            return;
        };
        if scope.with(|known| known.as_ref() == Some(&id)) {
            return;
        }
        let left = untracked(reading);
        if let Some(old) = scope.replace(Some(id.clone())) {
            untracked(|| save(app, old, left));
        }
        top.replace(None);
        let resume = shown.with_untracked(|now| now.as_ref().map(|now| now.resume.clone()));
        match resume {
            Some(Resume::Row { key, offset, rows }) => {
                at_end.set(false);
                left_at.replace(Some(rows));
                top.replace(Some((key.clone(), offset as f32)));
                anchors::restore(id.as_str(), key, offset as f32, scroller);
            }
            _ => {
                anchors::cancel(id.as_str());
                at_end.set(true);
                left_at.replace(None);
                after_answer(move || scroller.scroll_to_end(false));
            }
        }
    });
    cleanup(move || {
        let left = reading();
        if let Some(Some(id)) = scope.try_with(Option::take) {
            save(app, id, left);
        }
    });
    effect(move || {
        let now = count.get();
        rows_seen.replace(now);
        if at_end.get_untracked() {
            left_at.replace(None);
            after_answer(move || scroller.scroll_to_end(false));
        } else if left_at.get().is_none() {
            left_at.replace(Some(now));
        }
    });
    let asked = computed(jump);
    let answered = stored(asked.get_untracked());
    effect(move || {
        let now = asked.get();
        if answered.replace(now) != now {
            at_end.set(true);
            left_at.replace(None);
            after_answer(move || scroller.scroll_to_end(true));
        }
    });
    let surface = surface
        .element_ref(&scroller)
        .on_scroll(move |event: &ScrollEvent| {
            let offset = event.offset.y.0;
            let below = event.content_size.height.0 - offset - event.size.height.0;
            let end = below <= END_SLACK;
            if let Some(at) =
                scope.with(|known| known.as_ref().map(|id| anchors::at(id.as_str(), offset)))
            {
                top.replace(at);
            }
            if end != at_end.get_untracked() {
                if !end {
                    left_at.replace(Some(count.get_untracked()));
                }
                at_end.set(end);
            }
        });
    let control = dynamic(move || {
        if at_end.get() {
            return None;
        }
        let arrived = count
            .get()
            .saturating_sub(left_at.get().unwrap_or_else(|| count.get_untracked()));
        let label = words(form, arrived);
        let words = text(label.clone()).text_color(color(Token::Ink));
        let words = if form == Form::Terminal {
            words.font_weight(FontWeight::BOLD)
        } else {
            words.font_family(FONT_TEXT).text_size(px(12.0))
        };
        let control = view()
            .test_id("btn-jump-latest")
            .role(AccessibilityRole::Button)
            .label(label)
            .cursor_pointer()
            .on_press(move || {
                at_end.set(true);
                left_at.replace(None);
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
