//! A segmented control whose options the model names: the roster's role tabs
//! (`seg-agents-<role>`) and the panel's Summary | Terminal. The look is `kit::button::seg`'s (a
//! sunk track, the pressed option raised on the surface in ink, the rest ink-3); unlike it, each
//! option carries the model's own test id and words, and may carry a count after them in quiet
//! mono figures.

use nsh_base::{toggle, toggle_group};
use nsh_core::element::StyleRefinement;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::state::AppState;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, text_size};

/// The phone's least touch target.
const TARGET: f32 = 44.0;
const RING: f32 = 2.0;

/// The track around `options` (each an [`option`]): one toolbar for screen readers, the arrow
/// keys moving between its options. Desktop: 2 pt in on the sunk ground with a hairline edge.
/// Phone: 3 pt in, keeping its natural width (the caller scrolls the line). Terminal: the
/// options a Cell apart.
pub(super) fn track(app: AppState, options: Vec<AnyElement>) -> AnyElement {
    let group = toggle_group().flex_row().items_center().flex_shrink_0();
    let group = match app.form {
        Form::Terminal => group.gap(px(8.0)),
        Form::Desktop => group
            .gap(px(2.0))
            .p(px(2.0))
            .rounded(px(7.0))
            .bg(color(Token::BgSunk))
            .border_1()
            .border_color(color(Token::Rule)),
        Form::Phone => group
            .gap(px(3.0))
            .p(px(3.0))
            .rounded(px(9.0))
            .bg(color(Token::BgSunk))
            .border_1()
            .border_color(color(Token::Rule)),
    };
    group.children(options).into_element()
}

/// One option: `label`, then `count` in mono figures, pressed while `on` holds. Pressing it runs
/// `pick`; pressing the pressed one keeps it, so the control always names one option. Desktop
/// 24 pt high in 12 pt; phone 32 pt (a 44 pt target) in 13.5 pt; terminal one row, NSH banding
/// the pressed one itself. `test_id` names the view around the words.
pub(super) fn option(
    app: AppState,
    test_id: &str,
    label: String,
    count: Option<String>,
    on: Memo<bool>,
    pick: impl Fn() + 'static,
) -> AnyElement {
    let form = app.form;
    let (height, size, radius, pad) = match form {
        Form::Desktop => (24.0, 12.0, 5.0, 10.0),
        Form::Phone => (32.0, 13.5, 7.0, 12.0),
        Form::Terminal => (16.0, text_size(form), 0.0, 8.0),
    };
    let ink = move || {
        if on.get() || form == Form::Terminal {
            color(Token::Ink)
        } else {
            color(Token::Ink3)
        }
    };
    let words = text(label)
        .number_of_lines(1u32)
        .text_color(ink)
        .style_when(move || on.get(), |s| s.font_weight(FontWeight::BOLD))
        .when(form != Form::Terminal, |t| {
            t.font_family(FONT_TEXT).text_size(px(size))
        });
    let figures = count.map(|count| {
        text(count)
            .number_of_lines(1u32)
            .text_color(color(Token::Ink3))
            .when(form != Form::Terminal, |t| {
                t.font_family(FONT_MONO)
                    .tabular_nums()
                    .text_size(px(size - 0.5))
            })
    });
    let face = view()
        .test_id(test_id)
        .flex_row()
        .items_center()
        .gap(px(if form == Form::Terminal { 8.0 } else { 5.0 }))
        .child(words)
        .children(figures);
    toggle()
        .pressed(move || on.get())
        .on_change(move |_: &bool| pick())
        .flex_shrink_0()
        .h(px(height))
        .px(px(pad))
        .rounded(px(radius))
        .items_center()
        .justify_center()
        .cursor_pointer()
        // The 32 pt option reaches the 44 pt target above and below, never into its neighbours.
        .when(form == Form::Phone, |t| {
            let slop = px((TARGET - height) / 2.0);
            t.hit_slop(EdgeInsets {
                top: slop,
                right: px(0.0),
                bottom: slop,
                left: px(0.0),
            })
        })
        // In a terminal NSH bands a pressed toggle itself; a fill of the app's would be a second.
        .when(form != Form::Terminal, |t| {
            t.border_1()
                .bg(move || {
                    if on.get() {
                        color(Token::Surface)
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .border_color(move || {
                    if on.get() {
                        color(Token::Rule2)
                    } else {
                        AdaptiveColor::TRANSPARENT
                    }
                })
                .hover(|s| s.bg(color(Token::Hover)))
                .focus_visible(ring)
        })
        .child(face)
        .into_element()
}

fn ring(s: StyleRefinement) -> StyleRefinement {
    s.outline(px(RING))
        .outline_color(color(Token::Focus))
        .outline_offset(px(RING))
}
