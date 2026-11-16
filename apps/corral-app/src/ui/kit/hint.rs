//! The sentence behind a terse fact: a count, a rail, a strip of activity shows little, and its
//! hint says the rest (the mock's `[data-tip]`).
//!
//! Desktop and web: NSH's tooltip, a bubble above the element after the pointer rests on it for
//! 350 ms, at once when the keyboard brings the focus to it, gone when the pointer leaves or
//! presses or on Escape. The bubble is the mock's `#tip`: surface-2 with a rule-2 edge, 12.5 pt
//! ink, wrapping at 300 pt (about 38 characters). Terminal: the same tooltip, which NSH shows on
//! the status line while the focus or the pointer is on the element. Phone: no hover and no
//! bubble; the row shows one explanatory line instead (the caller's), and the hint stays for
//! screen readers where an element can carry it.

use nsh_base::{TooltipElement, tooltip};
use nsh_core::prelude::*;

use crate::model::Form;
use crate::state::AppState;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// The pointer's rest before the bubble shows (the mock's 350 ms).
const DELAY_MS: f64 = 350.0;
/// The bubble's widest: the mock's 300 px, about 38 characters of its text.
const MAX_WIDTH: f32 = 300.0;
const SIZE: f32 = 12.5;
const RING: f32 = 2.0;

/// `child` with `hint` as its tooltip, in a view of its own that is a stop of the keyboard focus
/// (desktop and terminal), so a fact that takes no presses still shows its sentence to the
/// keyboard; screen readers read the hint after the child. The wrapper takes the child's size:
/// a child that must grow inside its row takes [`hint_on`] instead. Phone, or an empty hint:
/// `child` unchanged.
pub fn hinted(app: AppState, child: impl IntoElement, hint: impl Into<SharedString>) -> AnyElement {
    let hint: SharedString = hint.into();
    let form = app.form;
    if hint.is_empty() || form == Form::Phone {
        return child.into_element();
    }
    let stop = view()
        .focusable(true)
        .accessibility_description(hint.clone())
        .when(form == Form::Desktop, |stop| {
            stop.rounded(px(4.0)).focus_visible(|s| {
                s.outline(px(RING))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(1.0))
            })
        })
        .child(child.into_element());
    tip(form, stop, hint).into_element()
}

/// `element` with `hint` as its tooltip, for an element that is already a stop of the keyboard
/// focus (a pressable row, cell or toggle) or sits inside one: the focus shows it only when it
/// lands on `element` itself or inside it. Screen readers read the hint after the element's
/// name on every form, the phone's too. An empty hint leaves `element` as it is.
pub fn hint_on<E: Interactive + Accessible>(
    app: AppState,
    element: E,
    hint: impl Into<SharedString>,
) -> E {
    let hint: SharedString = hint.into();
    if hint.is_empty() {
        return element;
    }
    let described = element.accessibility_description(hint.clone());
    match app.form {
        Form::Phone => described,
        Form::Desktop | Form::Terminal => tip(app.form, described, hint),
    }
}

/// [`hint_on`] for a hint that follows the feed (a project chip's counts): `hint` is read again
/// whenever what it reads changes, so the element keeps its place and its focus. Tooltip only:
/// the caller's label or accessibility label carries the words for screen readers. Phone:
/// `element` unchanged.
pub fn hint_on_live<E: Interactive>(
    app: AppState,
    element: E,
    hint: impl Fn() -> SharedString + 'static,
) -> E {
    match app.form {
        Form::Phone => element,
        Form::Desktop | Form::Terminal => element.tooltip(bubble(app.form, text(hint))),
    }
}

/// `element` with the tooltip only, for an element whose own label already says the hint (a
/// count's label is its sentence). Phone or an empty hint: `element` unchanged.
pub(crate) fn tip<E: Interactive>(form: Form, element: E, hint: SharedString) -> E {
    if hint.is_empty() || form == Form::Phone {
        return element;
    }
    element.tooltip(bubble(form, text(hint)))
}

/// The bubble around `words`. A terminal draws a tooltip as its status line in its own colours,
/// so there it is the words alone.
fn bubble(form: Form, words: Text) -> TooltipElement {
    // The element it describes carries the sentence for screen readers already.
    let words = words.accessibility_elements_hidden(true);
    if form == Form::Terminal {
        return tooltip().delay(DELAY_MS).child(words);
    }
    tooltip()
        .delay(DELAY_MS)
        .max_w(px(MAX_WIDTH))
        .px(px(9.0))
        .py(px(6.0))
        .rounded(px(6.0))
        .bg(color(Token::Surface2))
        .border_1()
        .border_color(color(Token::Rule2))
        .shadow_lg()
        .child(
            words
                .font_family(FONT_TEXT)
                .text_size(px(SIZE))
                .line_height(px((SIZE * 1.4).round()))
                .text_color(color(Token::Ink)),
        )
}
