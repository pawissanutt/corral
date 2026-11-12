//! What every part of the assistant screens shares: the transcript's small type, the outlined
//! message state, the card frame, the quoted owner words, the avatar, link words, and the model's
//! button descriptors wired to `State::act`.

use nsh_base::button as press_target;
use nsh_core::element::StyleRefinement;
use nsh_core::prelude::*;

use crate::model::assistant::{Action, Button};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::chip::chip;
use crate::ui::kit::marks::{dot, hue_fill, hue_ink};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// The phone's least touch target.
const TARGET: f32 = 44.0;
/// `.mono` is 0.93 of the text around it.
const MONO_SCALE: f32 = 0.93;

/// Runs an assistant action through its reducer. Card buttons never submit the draft, so the
/// text is empty and the target is whatever Ask holds.
pub fn act(app: AppState, action: Action) {
    let target = app.ask_target.get_untracked();
    app.act(app.assistant, move |state, env| {
        state.act(action, "", target, env)
    });
}

/// The transcript's header and note size (`.tm .hd`, `.ideahd`: 11.5 pt), a phone's 13 pt.
pub fn meta_size(form: Form) -> f32 {
    match form {
        Form::Phone => 13.0,
        Form::Desktop | Form::Terminal => 11.5,
    }
}

/// The receipt and note size (`.rcpt`, `.rests`, `.afoot`: 12 pt), a phone's 13.5 pt.
pub fn note_size(form: Form) -> f32 {
    match form {
        Form::Phone => 13.5,
        Form::Desktop | Form::Terminal => 12.0,
    }
}

/// Words at `size` in `ink`, wrapping. A terminal shows them in its own font and size, and full
/// ink for anything that must read as text (fainter neutral inks fade there).
pub fn words(app: AppState, content: impl Into<SharedString>, size: f32, ink: Token) -> Text {
    let form = app.form;
    let ink = if form == Form::Terminal && ink == Token::Ink2 {
        Token::Ink
    } else {
        ink
    };
    let t = text(content.into()).text_color(color(ink));
    if form == Form::Terminal {
        t
    } else {
        t.font_family(FONT_TEXT)
            .text_size(px(size))
            .line_height(px((size * 1.45).round()))
    }
}

/// A run in the mono font at 0.93 of `size`, inside a line of [`words`].
pub fn mono_run(app: AppState, content: impl Into<SharedString>, size: f32) -> Text {
    let t = text(content.into());
    if app.form == Form::Terminal {
        t
    } else {
        let mono = size * MONO_SCALE;
        t.font_family(FONT_MONO)
            .text_size(px(mono))
            .letter_spacing(px(-0.01 * mono))
    }
}

/// A bold run in ink, inside a line of [`words`] (`.hd b`).
pub fn strong_run(content: impl Into<SharedString>) -> Text {
    text(content.into())
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color(Token::Ink))
}

/// A whole mono line: commands, paths, receipts (`.acmd`, `.mono`).
pub fn mono_line(app: AppState, content: impl Into<SharedString>, size: f32) -> Text {
    let t = text(content.into()).text_color(color(Token::Ink));
    if app.form == Form::Terminal {
        t
    } else {
        t.font_family(FONT_MONO)
            .tabular_nums()
            .text_size(px(size))
            .line_height(px((size * 1.45).round()))
    }
}

/// The outlined state of a message (`.mstate`, `.pmstate`): its words in the hue inside a thin
/// line of it at 55 %; [`Hue::Neutral`] is ink-3 inside rule-2. A terminal shows it as a chip.
pub fn tag(app: AppState, label: impl Into<String>, hue: Hue) -> AnyElement {
    let form = app.form;
    if form == Form::Terminal {
        return chip(app, label, hue);
    }
    let label: String = label.into();
    let (ink, edge) = match hue {
        Hue::Neutral => (color(Token::Ink3), color(Token::Rule2)),
        _ => (hue_ink(hue), hue_ink(hue).opacity(0.55)),
    };
    view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .px(px(6.0))
        .rounded(px(4.0))
        .border_1()
        .border_color(edge)
        .child(
            text(label)
                .number_of_lines(1u32)
                .font_family(FONT_TEXT)
                .text_size(px(if form == Form::Phone { 12.0 } else { 11.0 }))
                .text_color(ink),
        )
        .into_element()
}

/// The edge of a card: the plain rule-2 line (`.actcard`, `.filed`), or the needs-you line at
/// 55 % and 1.5 pt (`.askuser`, `.permcard`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Plain,
    Asks,
}

/// A card on the surface, radius 8, its children 6 pt apart; `padded` gives it the 10 by 12 pt
/// inset of `.filed` (an owner-action card pads its own rows). A terminal draws a framed region a
/// Cell in from its line.
pub fn frame(app: AppState, edge: Edge, padded: bool, children: Vec<AnyElement>) -> AnyElement {
    if app.form == Form::Terminal {
        return term::framed(view().px(term::cells(1.0)).children(children)).into_element();
    }
    let (width, line) = match edge {
        Edge::Plain => (1.0, color(Token::Rule2)),
        Edge::Asks => (1.5, color(Token::You).opacity(0.55)),
    };
    view()
        .bg(color(Token::Surface))
        .rounded(px(8.0))
        .border(px(width))
        .border_color(line)
        .overflow_hidden()
        .gap(px(6.0))
        .when(padded, |card| card.px(px(12.0)).py(px(10.0)))
        .children(children)
        .into_element()
}

/// The owner's words, quoted and visibly untouched (`.words`): the caption in spaced capitals,
/// then the words in curly quotes on the sunken ground behind a hairline. Desktop 14 pt, phone
/// 16 pt; a terminal draws the caption quiet and the words in ink, two Cells in.
pub fn quote(app: AppState, caption: &str, said: &str) -> AnyElement {
    let form = app.form;
    let quoted = format!("“{said}”");
    if form == Form::Terminal {
        return view()
            .pl(term::cells(2.0))
            .child(term::quiet(caption.to_uppercase()))
            .child(text(quoted).text_color(color(Token::Ink)))
            .into_element();
    }
    let phone = form == Form::Phone;
    let (label, size) = if phone { (11.5, 16.0) } else { (10.5, 14.0) };
    view()
        .px(px(12.0))
        .py(px(6.0))
        .rounded(px(6.0))
        .bg(color(Token::BgSunk))
        .border_l(px(1.0))
        .border_l_color(color(Token::Rule2))
        .gap(px(2.0))
        .child(
            text(caption.to_owned())
                .uppercase()
                .font_family(FONT_TEXT)
                .text_size(px(label))
                .letter_spacing(px(label * 0.07))
                .text_color(color(Token::Ink4)),
        )
        .child(
            text(quoted)
                .font_family(FONT_TEXT)
                .text_size(px(size))
                .line_height(px((size * 1.45).round()))
                .text_color(color(Token::Ink)),
        )
        .into_element()
}

/// The author square of a message (`.av`): its letter bold in the hue on the hue's fill, 24 pt,
/// radius 6. A terminal shows the hue's actor glyph instead. Hidden from screen readers: the
/// header names the author.
pub fn avatar(app: AppState, letter: &str, hue: Hue) -> AnyElement {
    if app.form == Form::Terminal {
        return dot(app, hue);
    }
    view()
        .size(px(24.0))
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .bg(hue_fill(hue))
        .accessibility_elements_hidden(true)
        .child(
            text(letter.to_owned())
                .font_family(FONT_TEXT)
                .text_size(px(10.5))
                .font_weight(FontWeight::BOLD)
                .text_color(hue_ink(hue)),
        )
        .into_element()
}

/// Underlined words that act (`.ln2`), for a link whose target is an action, not a page.
pub fn link_words(
    app: AppState,
    label: impl Into<SharedString>,
    size: f32,
    test_id: &str,
    on_press: impl Fn() + 'static,
) -> Text {
    words(app, label, size, Token::Ink)
        .underline()
        .text_decoration_color(color(Token::Ink).opacity(0.35))
        .underline_offset(px(2.0))
        .cursor_pointer()
        .role(AccessibilityRole::Link)
        .test_id(test_id)
        .on_press(move || on_press())
}

/// A model button: its label, its weight (`primary` filled, otherwise `kind`), its enabled state,
/// a destructive look when `danger`, and its action run through the reducer.
pub fn model_button(app: AppState, button: &Button, kind: ButtonKind) -> AnyElement {
    let id = test_id(button);
    let action = button.action.clone();
    let press = move || act(app, action.clone());
    if button.danger {
        return danger(
            app,
            button.label,
            kind == ButtonKind::Ghost,
            &id,
            button.enabled,
            press,
        );
    }
    let kind = if button.primary {
        ButtonKind::Primary
    } else {
        kind
    };
    button_with(
        app,
        button.label,
        kind,
        ButtonOptions {
            disabled: !button.enabled,
            ..ButtonOptions::default()
        },
        id,
        press,
    )
}

/// Model buttons in a wrapping row, `gap` points apart (two Cells in a terminal).
pub fn button_row(app: AppState, buttons: &[Button], kind: ButtonKind, gap: f32) -> View {
    let row = view().flex_row().flex_wrap().items_center();
    let row = if app.form == Form::Terminal {
        row
    } else {
        row.gap(px(gap))
    };
    let mut parts = Vec::with_capacity(buttons.len() * 2);
    for (at, button) in buttons.iter().enumerate() {
        if at > 0 && app.form == Form::Terminal {
            parts.push(term::space(2.0));
        }
        parts.push(model_button(app, button, kind));
    }
    row.children(parts)
}

/// `btn-<label>`, with the record it acts on when the action names one, so the Edit of two filed
/// cards stay apart (`btn-edit-T-902`).
pub fn test_id(button: &Button) -> String {
    let subject = match &button.action {
        Action::Edit(Some(id)) | Action::Merge(id) | Action::Confirm(_, Some(id)) => Some(*id),
        _ => None,
    };
    let mut slug = String::with_capacity(button.label.len() + 8);
    for c in button.label.chars() {
        if c.is_alphanumeric() {
            slug.extend(c.to_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    match subject {
        Some(id) => format!("btn-{slug}-{id}"),
        None => format!("btn-{slug}"),
    }
}

/// A destructive button (`.btn.danger` outlined in the failure colour at 50 %, `.btn.quiet
/// .danger-t` its words alone), in the metrics of the kit's buttons. The kit has no destructive
/// kind.
fn danger(
    app: AppState,
    label: &'static str,
    outlined: bool,
    test_id: &str,
    enabled: bool,
    on_press: impl Fn() + 'static,
) -> AnyElement {
    let form = app.form;
    let words = text(label)
        .number_of_lines(1u32)
        .text_color(color(Token::Fail))
        .when(form != Form::Terminal, |t| {
            t.font_family(FONT_TEXT)
                .text_size(px(if form == Form::Phone { 15.0 } else { 12.5 }))
                .font_weight(if form == Form::Phone {
                    FontWeight::BOLD
                } else {
                    FontWeight::SEMIBOLD
                })
        });
    let target = press_target()
        .disabled(!enabled)
        .on_press(move |_: &()| on_press())
        .flex_row()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .cursor_pointer()
        .disabled_style(|s| s.opacity(0.45));
    let target = match form {
        Form::Terminal => target.h(px(16.0)).px(term::cells(1.0)),
        Form::Desktop => target
            .h(px(30.0))
            .px(px(if outlined { 12.0 } else { 8.0 }))
            .rounded(px(6.0))
            .hover(|s| s.bg(color(Token::Hover)))
            .focus_visible(ring),
        Form::Phone => target
            .min_h(px(TARGET))
            .px(px(16.0))
            .rounded(px(10.0))
            .active(|s| s.bg(color(Token::Hover)))
            .focus_visible(ring),
    };
    target
        .when(outlined && form != Form::Terminal, |b| {
            b.border_1().border_color(color(Token::Fail).opacity(0.5))
        })
        .child(view().test_id(test_id).child(words))
        .into_element()
}

/// The keyboard focus ring: 2 pt of the focus colour, 2 pt out.
pub fn ring(s: StyleRefinement) -> StyleRefinement {
    s.outline(px(2.0))
        .outline_color(color(Token::Focus))
        .outline_offset(px(2.0))
}

/// The room left in a row.
pub fn spacer() -> AnyElement {
    view().flex_1().min_w(px(0.0)).into_element()
}
