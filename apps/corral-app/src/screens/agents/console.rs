//! The terminal look of the mockup (`.term`, `.bigterm`, `.termbar`, `.tlab`): mono lines on a
//! surface that stays dark in both appearances, each line coloured by its model class.
//!
//! The theme has no terminal roles, so the surface takes the dark appearance's value of the
//! shell, ink and state roles in both appearances: the same colours the mockup's `--term-*` and
//! line classes resolve to. A real terminal (`Form::Terminal`) draws its own ground, so there the
//! lines keep the app's roles and the terminal maps them to its palette.

use nsh_core::EllipsizeMode;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::agents::{Feed, Line, LineClass, Terminal};
use crate::model::hue::Hue;
use crate::model::text::Span;
use crate::state::AppState;
use crate::ui::kit::marks::hue_ink;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// A role's dark-appearance value in both appearances.
fn night(token: Token) -> AdaptiveColor {
    let both = color(token);
    AdaptiveColor::new(both.dark, both.dark)
}

/// The token a state hue is drawn in on the dark surface.
fn hue_token(hue: Hue) -> Token {
    match hue {
        Hue::Neutral => Token::ShellInk,
        Hue::You => Token::You,
        Hue::Corral => Token::Corral,
        Hue::Worker => Token::Worker,
        Hue::Held => Token::Held,
        Hue::Fail => Token::Fail,
        Hue::Ok => Token::Ok,
    }
}

/// The surface's fill (`--term-bg`).
pub fn ground() -> AdaptiveColor {
    night(Token::Shell)
}

/// A line class's colour and weight on the dark surface (`.p`, `.ok`, `.w8`, `.b`).
fn class_look(class: LineClass) -> (AdaptiveColor, FontWeight) {
    match class {
        LineClass::Prompt => (night(Token::Worker), FontWeight::NORMAL),
        LineClass::Ok => (night(Token::Ok), FontWeight::NORMAL),
        LineClass::Dim => (night(Token::Ink3), FontWeight::NORMAL),
        LineClass::Bold => (night(Token::Ink), FontWeight::BOLD),
        LineClass::Plain => (night(Token::ShellInk), FontWeight::NORMAL),
    }
}

/// A line class's colour and weight in a real terminal.
fn cell_look(class: LineClass) -> (AdaptiveColor, FontWeight) {
    match class {
        LineClass::Prompt => (color(Token::Worker), FontWeight::NORMAL),
        LineClass::Ok => (color(Token::Ok), FontWeight::NORMAL),
        LineClass::Dim => (color(Token::Ink3), FontWeight::NORMAL),
        LineClass::Bold => (color(Token::Ink), FontWeight::BOLD),
        LineClass::Plain => (color(Token::Ink), FontWeight::NORMAL),
    }
}

/// One run of a line: its own hue over the class, dim or bold when the span says so.
fn run_of(form: Form, span: &Span, base: AdaptiveColor) -> Text {
    let ink = if span.hue != Hue::Neutral {
        if form == Form::Terminal {
            hue_ink(span.hue)
        } else {
            night(hue_token(span.hue))
        }
    } else if span.dim {
        if form == Form::Terminal {
            color(Token::Ink3)
        } else {
            night(Token::Ink3)
        }
    } else {
        base
    };
    text(span.text.clone())
        .text_color(ink)
        .when(span.bold, |t| t.font_weight(FontWeight::BOLD))
}

/// How a block of lines is set: size and leading in points, and whether long lines wrap
/// (`pre-wrap` on the phone) or are clipped at the surface's edge (`pre`).
#[derive(Clone, Copy)]
pub struct Set {
    pub size: f32,
    pub leading: f32,
    pub wrap: bool,
}

/// The wall tile's tail (`.tile .term`, 11 pt).
pub const TILE: Set = Set {
    size: 11.0,
    leading: 17.0,
    wrap: false,
};
/// An expanded tool call's output (`.tool .term`, 11 pt).
pub const TOOL: Set = Set {
    size: 11.0,
    leading: 17.0,
    wrap: false,
};
/// The desktop session terminal (`.bigterm`, 12.5 pt, 1.55).
pub const BIG: Set = Set {
    size: 12.5,
    leading: 19.5,
    wrap: true,
};
/// The phone session terminal (`.bigterm` at 10.5 pt, wrapping).
pub const PHONE: Set = Set {
    size: 10.5,
    leading: 16.5,
    wrap: true,
};

/// One line on the dark surface; an empty line keeps its row.
fn line_text(line: &Line, set: Set) -> Text {
    let (ink, weight) = class_look(line.class);
    let t = text("")
        .min_w(px(0.0))
        .min_h(px(set.leading))
        .font_family(FONT_MONO)
        .text_size(px(set.size))
        .line_height(px(set.leading))
        .font_weight(weight)
        .text_color(ink)
        .children(
            line.text
                .iter()
                .map(|span| run_of(Form::Desktop, span, ink)),
        );
    if set.wrap {
        t
    } else {
        t.number_of_lines(1u32).text_overflow(EllipsizeMode::Clip)
    }
}

/// The blinking block of the mockup, drawn still: the caret where the agent's input goes.
fn cursor(set: Set) -> AnyElement {
    view()
        .flex_shrink_0()
        .w(px(set.size * 0.64))
        .h(px(set.size * 1.2))
        .bg(night(Token::ShellInk))
        .accessibility_elements_hidden(true)
        .into_element()
}

/// Lines on the dark surface, with the cursor after a closing prompt line or under the rest.
pub fn lines(lines: &[Line], set: Set, with_cursor: bool) -> AnyElement {
    let count = lines.len();
    let mut rows: Vec<AnyElement> = Vec::with_capacity(count + 1);
    for (at, line) in lines.iter().enumerate() {
        let last = at + 1 == count;
        if last && with_cursor && line.class == LineClass::Prompt {
            rows.push(
                view()
                    .flex_row()
                    .items_center()
                    .gap(px(set.size * 0.6))
                    .child(line_text(line, set))
                    .child(cursor(set))
                    .into_element(),
            );
        } else {
            rows.push(line_text(line, set).into_element());
        }
    }
    if with_cursor
        && lines
            .last()
            .is_none_or(|line| line.class != LineClass::Prompt)
    {
        rows.push(
            view()
                .h(px(set.leading))
                .justify_center()
                .child(cursor(set))
                .into_element(),
        );
    }
    view().min_w(px(0.0)).children(rows).into_element()
}

/// The `live` or `snapshot` tag of a tail (`.tlab`): 10 pt words in the ok or held colour in a
/// half-strength ring of the same colour. A snapshot never passes for a stream.
pub fn tag(feed: Feed, label: &'static str) -> AnyElement {
    let ink = night(match feed {
        Feed::Live => Token::Ok,
        Feed::Snapshot => Token::Held,
    });
    view()
        .flex_row()
        .self_start()
        .flex_shrink_0()
        .px(px(5.0))
        .rounded(px(3.0))
        .border_1()
        .border_color(ink.opacity(0.5))
        .child(
            text(label)
                .number_of_lines(1u32)
                .font_family(FONT_TEXT)
                .text_size(px(10.0))
                .line_height(px(15.0))
                .letter_spacing(px(0.4))
                .text_color(ink),
        )
        .into_element()
}

/// The desktop terminal's toolbar (`.termbar`): the runtime, pane and size, the stream's
/// state, and at the trailing edge where keys go. A legacy snapshot's state is in the held
/// colour, so it never reads as a live stream.
pub fn toolbar(terminal: &Terminal) -> AnyElement {
    let words = |content: SharedString, ink: AdaptiveColor| {
        text(content)
            .number_of_lines(1u32)
            .flex_shrink(1.0)
            .min_w(px(0.0))
            .font_family(FONT_MONO)
            .text_size(px(11.5))
            .text_color(ink)
    };
    let quiet = night(Token::Ink2);
    let state = match terminal.feed {
        Feed::Live => quiet,
        Feed::Snapshot => night(Token::Held),
    };
    view()
        .test_id("session-termbar")
        .flex_row()
        .flex_none()
        .items_center()
        .gap(px(10.0))
        .px(px(14.0))
        .py(px(6.0))
        .bg(night(Token::BgSunk))
        .child(words(terminal.heading.clone().into(), quiet))
        .child(words(terminal.status.into(), state))
        .child(view().flex_1().min_w(px(0.0)))
        .child(words(terminal.input.into(), quiet))
        .into_element()
}

/// Lines in a real terminal: one row each in the class's role, the agent's spans in their hue.
pub fn cell_lines(app: AppState, lines: &[Line]) -> AnyElement {
    let form = app.form;
    view()
        .min_w(px(0.0))
        .children(lines.iter().map(|line| {
            let (ink, weight) = cell_look(line.class);
            if line.text.iter().all(|span| span.text.is_empty()) {
                return term::blank();
            }
            text("")
                .min_w(px(0.0))
                .number_of_lines(1u32)
                .font_weight(weight)
                .text_color(ink)
                .children(line.text.iter().map(|span| run_of(form, span, ink)))
                .into_element()
        }))
        .into_element()
}
