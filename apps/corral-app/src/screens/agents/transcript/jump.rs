//! Following the newest rows, and the way back to them. The transcript's scroll keeps to its
//! end while the reader is there; scrolling up pauses that without moving the reading position
//! as rows arrive. While paused, a control floats at the bottom right of the transcript and
//! counts what arrived: `↓ 37 new rows · End` (`↓ Jump to latest` when nothing arrived; the
//! phone's `↓ 37 new · Latest`). Pressing it returns to the end and following resumes.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::state::AppState;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

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

/// `surface` (the transcript's scroll) kept at its newest rows while the reader is at the end,
/// with the jump control over it while they are not. `rows` counts the transcript's rows
/// (`Transcript::rows`), tracked: each change scrolls to the end while following.
pub fn follow(app: AppState, surface: Scroll, rows: impl Fn() -> usize + 'static) -> AnyElement {
    let form = app.form;
    let scroller = element_ref();
    let at_end = signal(true);
    // The row count when the reader left the end.
    let left_at = stored(None::<usize>);
    let count = computed(rows);
    effect(move || {
        let now = count.get();
        if at_end.get_untracked() {
            left_at.replace(None);
            after_answer(move || scroller.scroll_to_end(false));
        } else if left_at.get().is_none() {
            left_at.replace(Some(now));
        }
    });
    let surface = surface
        .element_ref(&scroller)
        .on_scroll(move |event: &ScrollEvent| {
            let below = event.content_size.height.0 - event.offset.y.0 - event.size.height.0;
            let end = below <= END_SLACK;
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
