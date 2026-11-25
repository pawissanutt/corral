//! What a list page's filter bar is made of, shared by Sessions and Activity › Feed: the search
//! field and toggle options grouped as a toolbar.
//!
//! Desktop: a bordered search field and options on a sunk track, the chosen one raised. Phone:
//! the field over a wrapping row of options with 44 pt targets. Terminal: the field and options
//! on one row; NSH bands a pressed option itself.

use nsh_base::{toggle, toolbar};
use nsh_core::element::color as for_appearance;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::state::AppState;
use crate::ui::atoms::control_height;
use crate::ui::composer::copyable_input;
use crate::ui::icons::{Icon, icon};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, text_size};

/// The search field's width on a desktop toolbar.
const FIELD_WIDTH: f32 = 280.0;
const FIELD_CELLS: f32 = 22.0;
/// The least touch target on a phone.
const TARGET: f32 = 44.0;

/// How large an option is drawn.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Size {
    /// A toolbar's option.
    Bar,
    /// An option in a section's heading.
    Small,
}

/// A search field: a bordered box with a search glyph, as the Tasks filter is.
pub fn search_field(
    app: AppState,
    test_id: &str,
    label: &'static str,
    read: impl Fn() -> String + 'static,
    write: impl Fn(&str) + 'static,
) -> AnyElement {
    let form = app.form;
    let entry = copyable_input(app, None, read)
        .test_id(test_id)
        .label(label)
        .placeholder(label)
        .on_change(move |typed: &str| write(typed))
        .flex_1()
        .min_w(px(0.0))
        .text_color(color(Token::Ink))
        .placeholder_color(move || {
            for_appearance(color(if form == Form::Terminal {
                Token::Ink3
            } else {
                Token::Ink4
            }))
        })
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)))
        .bg(AdaptiveColor::TRANSPARENT);
    match form {
        Form::Terminal => view()
            .w(term::cells(FIELD_CELLS))
            .flex_shrink(1.0)
            .min_w(term::cells(8.0))
            .border_b_1()
            .border_color(color(Token::Rule2))
            .child(entry.h(term::rows(1.0)))
            .into_element(),
        Form::Desktop | Form::Phone => {
            let (height, size) = if form == Form::Phone {
                (40.0, 16.0)
            } else {
                (30.0, text_size(form))
            };
            view()
                .when(form == Form::Desktop, |field| {
                    field.w(px(FIELD_WIDTH)).flex_shrink(1.0).min_w(px(160.0))
                })
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .h(px(height))
                .px(px(10.0))
                .rounded(px(6.0))
                .border_1()
                .border_color(color(Token::Rule2))
                .bg(color(Token::Surface))
                .focus_within(|s| s.border_color(color(Token::Focus)))
                .child(icon(Icon::Search, 14.0, color(Token::Ink3)))
                .child(
                    entry
                        .h(px(height))
                        .font_family(FONT_TEXT)
                        .text_size(px(size)),
                )
                .into_element()
        }
    }
}

/// The options of one control as a toolbar named for screen readers (one tab stop, the arrow
/// keys move between its options): a sunk track on a desktop toolbar.
pub fn track(
    app: AppState,
    size: Size,
    name: impl Into<String>,
    options: Vec<AnyElement>,
) -> AnyElement {
    let name: String = name.into();
    let row = toolbar()
        .accessibility_label(SharedString::from(name))
        .items_center();
    let row = match (app.form, size) {
        (Form::Terminal, _) => row.flex_wrap().gap_x(term::cells(1.0)),
        (Form::Desktop, Size::Bar) => row
            .flex_wrap()
            .gap(px(2.0))
            .p(px(2.0))
            .rounded(px(7.0))
            .bg(color(Token::BgSunk))
            .border_1()
            .border_color(color(Token::Rule)),
        // Wrapped rows keep apart by what each option reaches beyond its face.
        (Form::Phone, Size::Bar) => row.flex_wrap().gap_x(px(6.0)).gap_y(px(TARGET - 32.0)),
        (Form::Desktop | Form::Phone, Size::Small) => row.gap(px(2.0)),
    };
    row.children(options).into_element()
}

/// One option: its words and, for a mode, how many sessions it holds; the chosen one raised
/// on the surface in ink, the others quiet. `on` is read reactively, so a row of options can
/// stay mounted (and keep the keyboard focus) while the choice changes.
pub fn option(
    app: AppState,
    size: Size,
    label: String,
    count: Option<usize>,
    on: impl Fn() -> bool + Copy + 'static,
    test_id: String,
    pick: impl Fn() + 'static,
) -> AnyElement {
    let form = app.form;
    let (height, words_size, pad, radius) = match (form, size) {
        // A touch tablet draws the desktop form; its options keep the 44 pt target.
        (Form::Desktop, Size::Bar) => (control_height(24.0), 12.0, 10.0, 5.0),
        (Form::Desktop, Size::Small) => (control_height(20.0), 11.0, 7.0, 4.0),
        (Form::Phone, Size::Bar) => (32.0, 13.5, 12.0, 16.0),
        (Form::Phone, Size::Small) => (32.0, 12.5, 8.0, 6.0),
        (Form::Terminal, _) => (16.0, text_size(form), 1.0, 0.0),
    };
    let words = text(label)
        .number_of_lines(1u32)
        .text_color(move || {
            color(if on() || form == Form::Terminal {
                Token::Ink
            } else {
                Token::Ink3
            })
        })
        .style_when(on, |s| s.font_weight(FontWeight::SEMIBOLD))
        .when(form != Form::Terminal, |t| {
            t.font_family(FONT_TEXT).text_size(px(words_size))
        });
    let count = count.map(|count| {
        let figure = text(count.to_string())
            .number_of_lines(1u32)
            .text_color(move || color(if on() { Token::Ink3 } else { Token::Ink4 }));
        match form {
            Form::Terminal => figure.into_element(),
            Form::Desktop | Form::Phone => figure
                .font_family(FONT_MONO)
                .tabular_nums()
                .text_size(px(words_size - 1.0))
                .into_element(),
        }
    });
    let face = view()
        .test_id(&test_id)
        .flex_row()
        .items_center()
        .gap(px(if form == Form::Terminal { 8.0 } else { 6.0 }))
        .child(words)
        .children(count);
    let handle = element_ref();
    let target = toggle()
        .element_ref(&handle)
        .pressed(on)
        .on_change(move |_: &bool| pick())
        .flex_row()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .cursor_pointer();
    match form {
        // NSH bands a pressed toggle itself; the first Cell is the focus mark's.
        Form::Terminal => target
            .h(term::rows(1.0))
            .pr(term::cells(1.0))
            .child(term::focus_cell(term::focus_of(handle)))
            .child(face)
            .into_element(),
        Form::Desktop | Form::Phone => target
            .h(px(height))
            // A phone option reaches the 44 pt target above and below, never into its neighbours.
            .when(form == Form::Phone, |t| {
                let slop = px((TARGET - height) / 2.0);
                t.hit_slop(EdgeInsets {
                    top: slop,
                    right: px(0.0),
                    bottom: slop,
                    left: px(0.0),
                })
            })
            .px(px(pad))
            .rounded(px(radius))
            .border_1()
            .bg(move || {
                if on() {
                    color(Token::Surface)
                } else if form == Form::Phone && size == Size::Bar {
                    color(Token::BgSunk)
                } else {
                    AdaptiveColor::TRANSPARENT
                }
            })
            .border_color(move || {
                if on() {
                    color(Token::Rule2)
                } else if form == Form::Phone && size == Size::Bar {
                    color(Token::Rule)
                } else {
                    AdaptiveColor::TRANSPARENT
                }
            })
            .hover(|s| s.bg(color(Token::Hover)))
            .focus_visible(|s| {
                s.outline(px(2.0))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(1.0))
            })
            .child(face)
            .into_element(),
    }
}
