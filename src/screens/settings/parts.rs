//! What every Settings section shares: reading the model tracked, running its actions, its
//! buttons and their test ids, the section frame of each form, and the cells of its tables.

use nsh_base::button as press_target;
use nsh_core::element::StyleRefinement;
use nsh_core::prelude::*;

use crate::model::Env;
use crate::model::Form;
use crate::model::settings::{self, Action, Button, Emphasis, Sheet};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::text::h3;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, text_size};

/// The widest a section reads at on the desktop (the mockup's `max-width: 960px`).
const MEASURE: f32 = 960.0;
/// The phone's least touch target.
const TARGET: f32 = 44.0;

/// A view of the settings model, read tracked: what `build` reads redraws the caller.
pub fn read<T>(app: AppState, build: impl FnOnce(&settings::State, Env) -> T) -> T {
    let env = app.env();
    app.settings.with(|state| build(state, env))
}

/// Runs a settings action through the model and applies the effects it returns.
pub fn act(app: AppState, action: Action) {
    app.act(app.settings, move |state, _| state.act(action));
}

/// The test id of a control, from the action it runs (`btn-pair-confirm`, `seg-conn-relay`).
pub fn test_id(action: &Action) -> String {
    let id = match action {
        Action::PairStep(step) if *step < 0 => String::from("btn-pair-back"),
        Action::PairStep(_) => String::from("btn-pair-next"),
        Action::PairConfirm => String::from("btn-pair-confirm"),
        Action::PairReject => String::from("btn-pair-reject"),
        Action::Revoke(name) => format!("btn-revoke-{name}"),
        Action::OpenSheet(Sheet::Pair) => String::from("btn-pair-sheet"),
        Action::OpenSheet(Sheet::Scan) => String::from("btn-scan-sheet"),
        Action::CloseSheet => String::from("btn-sheet-close"),
        Action::ToggleNotification(kind) => format!("btn-notify-{kind:?}"),
        Action::SetConnection(conn) => format!("seg-conn-{}", conn.slug()),
        Action::SetSection(section) => format!("settings-{}", section.slug()),
        Action::TurnOn(name) => format!("btn-turn-on-{name}"),
        Action::DiscardQueue => String::from("btn-discard"),
        Action::RemoveProfile(name) => format!("btn-remove-{name}"),
        Action::AddProfile => String::from("btn-add-profile"),
    };
    id.to_lowercase().replace([' ', '.'], "-")
}

/// A model button with its own test id.
pub fn action_button(app: AppState, model: &Button) -> AnyElement {
    model_button(app, model, ButtonOptions::default(), test_id(&model.action))
}

/// A model button in the weight the model gives it; pressing runs its action.
pub fn model_button(
    app: AppState,
    model: &Button,
    options: ButtonOptions,
    test_id: String,
) -> AnyElement {
    let action = model.action.clone();
    let options = ButtonOptions {
        disabled: !model.enabled,
        ..options
    };
    let on_press = move || act(app, action.clone());
    let kind = match model.emphasis {
        Emphasis::Primary => ButtonKind::Primary,
        Emphasis::Ghost => ButtonKind::Ghost,
        Emphasis::Quiet => ButtonKind::Quiet,
        Emphasis::Danger => return danger(app, model.label, options.disabled, test_id, on_press),
    };
    button_with(app, model.label, kind, options, test_id, on_press)
}

/// The destructive button (`.btn.danger`), which the kit has no kind for: failure-red words
/// in a half-strength red outline. Desktop 26 pt high (the device table's row button); phone
/// the outlined `.pbtn` with red words at the 44 pt target; terminal the red words alone.
fn danger(
    app: AppState,
    label: &'static str,
    disabled: bool,
    test_id: String,
    on_press: impl Fn() + 'static,
) -> AnyElement {
    let form = app.form;
    let words = text(label)
        .number_of_lines(1u32)
        .text_color(color(Token::Fail));
    let words = match form {
        Form::Terminal => words,
        Form::Desktop => words
            .font_family(FONT_TEXT)
            .text_size(px(12.5))
            .font_weight(FontWeight::SEMIBOLD),
        Form::Phone => words
            .font_family(FONT_TEXT)
            .text_size(px(14.0))
            .font_weight(FontWeight::BOLD),
    };
    let target = press_target()
        .disabled(disabled)
        .on_press(move |_: &()| on_press())
        .flex_row()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .cursor_pointer()
        .disabled_style(|s| s.opacity(0.45));
    let target = match form {
        Form::Terminal => target.h(term::rows(1.0)).px(term::cells(1.0)),
        Form::Desktop => target
            .h(px(26.0))
            .px(px(12.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(color(Token::Fail).opacity(0.5))
            .hover(|s| s.bg(color(Token::Hover)))
            .active(|s| s.bg(color(Token::Sel)))
            .focus_visible(ring),
        Form::Phone => target
            .min_h(px(TARGET))
            .px(px(12.0))
            .rounded(px(10.0))
            .border_1()
            .border_color(color(Token::Rule2))
            .active(|s| s.bg(color(Token::Hover)))
            .focus_visible(ring),
    };
    target
        .child(view().test_id(&test_id).child(words))
        .into_element()
}

/// The keyboard focus ring: 2 pt of the focus colour, 2 pt out.
pub fn ring(s: StyleRefinement) -> StyleRefinement {
    s.outline(px(2.0))
        .outline_color(color(Token::Focus))
        .outline_offset(px(2.0))
}

/// Controls in a row: 8 pt apart, two Cells apart in a terminal.
pub fn button_row(app: AppState, buttons: Vec<AnyElement>) -> AnyElement {
    if app.form == Form::Terminal {
        let mut parts = Vec::with_capacity(buttons.len() * 2);
        for (at, button) in buttons.into_iter().enumerate() {
            if at > 0 {
                parts.push(term::space(2.0));
            }
            parts.push(button);
        }
        return view()
            .flex_row()
            .items_center()
            .children(parts)
            .into_element();
    }
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(8.0))
        .children(buttons)
        .into_element()
}

/// Vertical room between two parts of a section: `points` on a screen, a blank row in a
/// terminal, where margins inside a frame take no rows.
pub fn space(app: AppState, points: f32) -> AnyElement {
    if app.form == Form::Terminal {
        return term::blank();
    }
    view().h(px(points)).flex_shrink_0().into_element()
}

/// The section's title (`.set h2`), test_id "page-title": 16 pt bold on the desktop; in a
/// terminal the title set into the frame's line. The phone names the section in its pushed
/// head instead.
fn heading(app: AppState, title: &'static str) -> AnyElement {
    let words = text(title)
        .test_id("page-title")
        .role(AccessibilityRole::Header)
        .number_of_lines(1u32)
        .font_weight(FontWeight::BOLD)
        .text_color(color(Token::Ink));
    if app.form == Form::Terminal {
        return words.into_element();
    }
    words
        .font_family(FONT_TEXT)
        .text_size(px(16.0))
        .letter_spacing(px(-0.08))
        .into_element()
}

/// A section as each form shows it: on the desktop the title over the parts, `gap` points
/// apart, held to the mockup's measure; in a terminal a framed pane with the title in its top
/// line; on the phone the parts alone, padded as the mockup's page bodies (14 18 24).
pub fn frame(app: AppState, title: &'static str, gap: f32, parts: Vec<AnyElement>) -> AnyElement {
    match app.form {
        Form::Desktop => view()
            .max_w(px(MEASURE))
            .child(heading(app, title))
            .child(view().h(px(gap)).flex_shrink_0())
            .children(parts)
            .into_element(),
        Form::Terminal => term::framed(view().px(term::cells(1.0)).children(parts))
            .child(term::top_title(heading(app, title)))
            .into_element(),
        Form::Phone => view()
            .px(px(18.0))
            .pt(px(14.0))
            .pb(px(24.0))
            .children(parts)
            .into_element(),
    }
}

/// A quiet paragraph at the body size (`p.dim`), wrapping.
pub fn intro(app: AppState, words: &'static str) -> AnyElement {
    let form = app.form;
    let words = text(words).text_color(color(Token::Ink3));
    if form == Form::Terminal {
        return words.into_element();
    }
    let size = text_size(form);
    let leading = if form == Form::Phone { 1.5 } else { 1.45 };
    words
        .font_family(FONT_TEXT)
        .text_size(px(size))
        .line_height(px((size * leading).round()))
        .into_element()
}

/// The text size of a table cell (`.qtable`, `.devtable`: 12.5 pt); a step up on the phone,
/// where 12.5 would read small at arm's length.
pub fn cell_size(form: Form) -> f32 {
    match form {
        Form::Desktop => 12.5,
        Form::Phone => 13.5,
        Form::Terminal => text_size(form),
    }
}

/// A table cell's words in `ink`, wrapping; bold when `strong`.
pub fn cell(app: AppState, words: impl Into<SharedString>, ink: Token, strong: bool) -> Text {
    let form = app.form;
    let words = text(words.into())
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .text_color(color(ink))
        .when(strong, |t| t.font_weight(FontWeight::BOLD));
    if form == Form::Terminal {
        return words;
    }
    let size = cell_size(form);
    words
        .font_family(FONT_TEXT)
        .text_size(px(size))
        .line_height(px((size * 1.4).round()))
}

/// A table cell's ids, keys or paths in the mono font (`.mono`, 0.93 of the cell's size), in
/// `ink`, wrapping.
pub fn mono_cell(app: AppState, words: impl Into<SharedString>, ink: Token) -> Text {
    let form = app.form;
    let words = text(words.into())
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .text_color(color(ink));
    if form == Form::Terminal {
        return words;
    }
    let size = cell_size(form) * 0.93;
    words
        .font_family(FONT_MONO)
        .tabular_nums()
        .text_size(px(size))
        .line_height(px((size * 1.5).round()))
        .letter_spacing(px(-0.01 * size))
}

/// A cell's content pushed to the trailing edge (`text-align: right`).
pub fn trailing(content: AnyElement) -> AnyElement {
    view()
        .flex_1()
        .flex_row()
        .justify_end()
        .items_center()
        .child(content)
        .into_element()
}

/// A quiet caption under a part, as `.illus` sits under the mockup's tables (4 to 6 pt).
pub fn under(app: AppState, points: f32, content: AnyElement) -> AnyElement {
    if app.form == Form::Terminal {
        return content;
    }
    view().mt(px(points)).child(content).into_element()
}

/// A section caption (`.h3`) with no room above it, as the mockup's first one in a section
/// (`margin-top: 0`), with its optional note at the trailing edge.
pub fn first_heading(app: AppState, label: &'static str, note: Option<&'static str>) -> AnyElement {
    let lift = match app.form {
        Form::Desktop => 18.0,
        Form::Phone => 14.0,
        Form::Terminal => 0.0,
    };
    view()
        .mt(px(-lift))
        .child(h3(app, label, note.map(str::to_owned)))
        .into_element()
}
