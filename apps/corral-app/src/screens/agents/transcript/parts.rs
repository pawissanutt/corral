//! What every transcript row shares: the pressable head that opens a row (`btn-expand-<id>`),
//! the state glyph (a spinner while it runs), the state of a continued item (its complete
//! content loading, or failed with a retry), and an opened row's sections: recorded text in a
//! sunk well, diffs with the review's row anatomy, Markdown summaries.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::board::{LineKind, Mark};
use crate::model::hue::Hue;
use crate::model::review::NumberedLine;
use crate::model::session::Action;
use crate::model::session::transcript::{More, Section, ToolState};
use crate::screens::markdown::markdown;
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::marks::{hue_ink, mark};
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::super::run;

/// Recorded text and diff code: 11.5 pt at a 17 pt line.
const CODE: f32 = 11.5;
const CODE_LINE: f32 = 17.0;
/// The old and new number columns, and the sign column, of a diff row.
const GUTTER: f32 = 34.0;
const SIGN: f32 = 14.0;

/// `row` made pressable when there is something to open: a button that says what it opens and
/// whether it shows, `btn-expand-<id>`, with the hover and focus of a row.
pub(super) fn pressable(
    app: AppState,
    row: View,
    action: Option<&Action>,
    label: String,
    open: bool,
    id: &str,
) -> View {
    let Some(action) = action.cloned() else {
        return row;
    };
    let row = row
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(format!(
            "{label}, {}",
            if open { "shown" } else { "hidden" }
        ))
        .test_id(&format!("btn-expand-{id}"))
        .on_press(move || run(app, action.clone()));
    if app.form == Form::Terminal {
        return row;
    }
    row.hover(|s| s.bg(color(Token::Hover)))
        .active(|s| s.bg(color(Token::Sel)))
        .focus_visible(|s| {
            s.outline(px(2.0))
                .outline_color(color(Token::Focus))
                .outline_offset(px(-2.0))
        })
}

/// Whether a row shows what it hides: the app's chevron, turned down while open; a terminal's
/// `▸` and `▾`.
pub(super) fn disclosure(app: AppState, open: bool) -> AnyElement {
    if app.form == Form::Terminal {
        return term::quiet(if open { "▾" } else { "▸" }).into_element();
    }
    view()
        .flex_shrink_0()
        .accessibility_elements_hidden(true)
        .rotate(deg(if open { 90.0 } else { 0.0 }))
        .child(icon(Icon::Chevron, 12.0, color(Token::Ink3)))
        .into_element()
}

/// Small words at the secondary size: a label, a count, a size.
pub(super) fn small(app: AppState, words: impl Into<SharedString>, ink: Token) -> Text {
    let words = text(words.into()).text_color(color(ink));
    if app.form == Form::Terminal {
        words
    } else {
        words
            .font_family(FONT_TEXT)
            .text_size(px(if app.form == Form::Phone { 12.5 } else { 11.0 }))
    }
}

/// Small words in a state's colour, muted when it has none.
pub(super) fn tinted(app: AppState, words: impl Into<SharedString>, hue: Hue) -> Text {
    let words = small(app, words, Token::Ink3);
    if hue == Hue::Neutral {
        words
    } else {
        words.text_color(hue_ink(hue))
    }
}

/// A row's state: the running mark (a spinner in the terminal) while it is written or runs,
/// else its glyph in its state's colour, so the shape says the state without the colour.
pub(super) fn glyph(app: AppState, state: ToolState) -> AnyElement {
    let Some(shape) = state.glyph() else {
        return mark(app, Mark::Active, Hue::Neutral);
    };
    let ink = if state == ToolState::Waiting {
        color(Token::Ink3)
    } else {
        hue_ink(state.hue())
    };
    let shown = text(shape)
        .flex_shrink_0()
        .text_color(ink)
        .role(AccessibilityRole::Image)
        .label(state.label());
    if app.form == Form::Terminal {
        return shown.into_element();
    }
    shown
        .font_family(FONT_MONO)
        .text_size(px(12.0))
        .line_height(px(17.0))
        .into_element()
}

/// Where a continued item's complete content stands, under its preview: on its way, or failed
/// with why and a retry (`btn-retry-<id>`). `waiting` says a preview not yet asked for is about
/// to be fetched on its own (words), rather than when the row opens.
pub(super) fn state(app: AppState, more: &More, id: &str, waiting: bool) -> Option<AnyElement> {
    let words = match more {
        More::Whole => return None,
        More::Preview { size } if waiting => format!("Loading the rest · {size}…"),
        More::Preview { .. } => return None,
        More::Loading { size } => format!("Loading the rest · {size}…"),
        More::Failed {
            size,
            reason,
            retry,
        } => {
            let retry = retry.clone();
            return Some(
                view()
                    .flex_row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .py(px(2.0))
                    .child(
                        small(
                            app,
                            format!("✗ Only a preview is shown; the rest ({size}) could not be read: {reason}"),
                            Token::Fail,
                        )
                        .flex_shrink(1.0)
                        .min_w(px(0.0)),
                    )
                    .child(button(
                        app,
                        "Retry",
                        ButtonKind::Quiet,
                        format!("btn-retry-{id}"),
                        move || run(app, retry.clone()),
                    ))
                    .into_element(),
            );
        }
    };
    Some(small(app, words, Token::Ink3).py(px(2.0)).into_element())
}

/// What a folded row hides and the size of what is still to fetch: `41 lines · 12 KB`.
pub(super) fn hidden(count: Option<&str>, more: &More) -> Option<String> {
    let parts: Vec<&str> = count.into_iter().chain(more.size()).collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// Where an opened row's or a nested block's content sits: two Cells in, or under the row's
/// words on the page.
pub(super) fn indented(app: AppState, body: impl IntoElement) -> AnyElement {
    if app.form == Form::Terminal {
        view().pl(term::cells(2.0)).child(body).into_element()
    } else {
        view()
            .pl(px(18.0))
            .pt(px(4.0))
            .pb(px(6.0))
            .child(body)
            .into_element()
    }
}

/// An opened row's sections, in order.
pub(super) fn sections(app: AppState, sections: &[Section]) -> AnyElement {
    let gap = if app.form == Form::Terminal { 0.0 } else { 8.0 };
    view()
        .gap(px(gap))
        .min_w(px(0.0))
        .children(sections.iter().map(|part| section(app, part)))
        .into_element()
}

fn section(app: AppState, part: &Section) -> AnyElement {
    match part {
        Section::Prose(words) => markdown(app, words, false),
        Section::Text { label, text: body } => view()
            .min_w(px(0.0))
            .child(small(app, label.clone(), Token::Ink3).mb(px(3.0)))
            .child(well(app, body))
            .into_element(),
        Section::Diff {
            label,
            hunks,
            notes,
        } => {
            let mut rows = Vec::new();
            for hunk in hunks {
                if !hunk.header.is_empty() {
                    rows.push(header(app, &hunk.header));
                }
                rows.extend(hunk.lines.iter().map(|line| diff_line(app, line)));
            }
            let body = if app.form == Form::Terminal {
                view().pl(term::cells(2.0)).children(rows)
            } else {
                view()
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(color(Token::Rule))
                    .overflow_hidden()
                    .children(rows)
            };
            view()
                .min_w(px(0.0))
                .child(small(app, label.clone(), Token::Ink3).mb(px(3.0)))
                .children(
                    notes
                        .iter()
                        .map(|note| small(app, note.clone(), Token::Ink3)),
                )
                .child(body)
                .into_element()
        }
    }
}

/// Recorded text as written, in the mono font: on the desktop a sunk well that scrolls sideways
/// so columns stay aligned; on the phone it wraps; a terminal indents it two Cells.
pub(super) fn well(app: AppState, body: &str) -> AnyElement {
    match app.form {
        Form::Terminal => view()
            .pl(term::cells(2.0))
            .selectable(true)
            .child(
                text(body.to_owned())
                    .select_text()
                    .text_color(color(Token::Ink)),
            )
            .into_element(),
        Form::Phone => view()
            .rounded(px(4.0))
            .bg(color(Token::BgSunk))
            .px(px(10.0))
            .py(px(8.0))
            .child(code(body.to_owned()))
            .into_element(),
        Form::Desktop => view()
            .rounded(px(4.0))
            .bg(color(Token::BgSunk))
            .overflow_hidden()
            .child(
                scroll().horizontal().child(
                    view()
                        .flex_shrink_0()
                        .px(px(10.0))
                        .py(px(8.0))
                        .child(code(body.to_owned()).flex_shrink_0()),
                ),
            )
            .into_element(),
    }
}

/// Code text: mono at the code size and line.
pub(super) fn code(content: impl Into<SharedString>) -> Text {
    text(content.into())
        .font_family(FONT_MONO)
        .text_size(px(CODE))
        .line_height(px(CODE_LINE))
        .text_color(color(Token::Ink))
}

/// A hunk's header (`@@ -12,4 +12,6 @@`, or `⋯` between the hunks of a display diff).
fn header(app: AppState, words: &str) -> AnyElement {
    if app.form == Form::Terminal {
        return term::quiet(words.to_owned()).into_element();
    }
    view()
        .px(px(8.0))
        .py(px(2.0))
        .bg(color(Token::Surface2))
        .child(code(words.to_owned()).text_color(color(Token::Ink3)))
        .into_element()
}

/// A diff row: the numbers (the phone keeps only the new one), the sign and the code on the
/// line's tint; long lines wrap rather than leave the row. A terminal says added or deleted by
/// colour and by the sign again.
fn diff_line(app: AppState, line: &NumberedLine) -> AnyElement {
    let (tint, ink) = match line.kind {
        LineKind::Add => (color(Token::OkBg), Token::Ok),
        LineKind::Del => (color(Token::FailBg), Token::Fail),
        LineKind::Context => (AdaptiveColor::TRANSPARENT, Token::Ink4),
    };
    let number = |n: Option<u32>| n.map(|n| n.to_string()).unwrap_or_default();
    if app.form == Form::Terminal {
        let code_ink = if line.kind == LineKind::Context {
            Token::Ink
        } else {
            ink
        };
        return text("")
            .child(
                text(format!("{:>4} {:>4} ", number(line.old), number(line.new)))
                    .text_color(color(Token::Ink3)),
            )
            .child(text(format!("{:<1} ", line.sign)).text_color(color(ink)))
            .child(text(line.text.clone()).text_color(color(code_ink)))
            .into_element();
    }
    let gutter = move |n: Option<u32>| {
        view()
            .w(px(GUTTER))
            .flex_shrink_0()
            .pr(px(6.0))
            .items_end()
            .accessibility_elements_hidden(true)
            .child(
                code(number(n))
                    .tabular_nums()
                    .text_color(color(Token::Ink4)),
            )
    };
    view()
        .flex_row()
        .bg(tint)
        .when(app.form == Form::Desktop, |row| row.child(gutter(line.old)))
        .child(gutter(line.new))
        .child(
            view()
                .w(px(SIGN))
                .flex_shrink_0()
                .child(code(line.sign).text_color(color(ink))),
        )
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .pr(px(8.0))
                .child(code(line.text.clone())),
        )
        .into_element()
}
