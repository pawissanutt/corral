//! The Diff tab (`.diff`): each file's head, the hunk header, unified or side-by-side rows with
//! their add/del tints and the highlighted word, the discussion between the rows it follows, and
//! collapsed files. The rows are built here rather than with `nsh_base`'s DiffView because the
//! mockup sets an inline comment card between two rows, which DiffView does not host.

use nsh_core::EllipsizeMode;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::board::LineKind;
use crate::model::review::{
    Button, CommentView, DiffContent, DiffView, FileView, NumberedLine, SplitRow, UnifiedRow,
};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::chip::illus;
use crate::ui::kit::page::panel;
use crate::ui::kit::text::dim;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::{act, button_id, command, rich_at, ring};

/// One code row: 11.5 pt at the mockup's line height of 1.6.
const LINE: f32 = 18.0;
/// The old and new number columns (`38px 38px`) and the sign column (`14px`).
const GUTTER: f32 = 38.0;
const SIGN: f32 = 14.0;
/// The comment card's indent past the gutters (`padding-left: 90px`) and its widest.
const COMMENT_INDENT: f32 = 90.0;
const COMMENT_MAX: f32 = 640.0;
/// The highlighted word (`--add-word`, `--del-word`): the state colour over the line's tint.
const WORD_ALPHA: f32 = 0.3;
/// The least touch target on a phone.
const TARGET: f32 = 44.0;
/// Wider than any line of the patch, so a code line is laid out whole before its box clips it.
const UNWRAPPED: f32 = 2400.0;

fn code_size(form: Form) -> f32 {
    if form == Form::Phone { 11.0 } else { 11.5 }
}

/// The phone's notice, then every file in the model's order.
pub(super) fn diff(app: AppState, diff: &DiffView) -> AnyElement {
    view()
        .children(diff.notice.map(|notice| {
            view()
                .mx(px(4.0))
                .mb(px(8.0))
                .child(dim(app, notice))
                .into_element()
        }))
        .children(diff.files.iter().map(|file| file_block(app, file)))
        .into_element()
}

fn file_block(app: AppState, file: &FileView) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let body = match &file.content {
        DiffContent::Collapsed(button) => collapsed(app, button),
        DiffContent::Unified(rows) => hunk(app, file, unified_rows(app, file, rows)),
        DiffContent::Split(rows) => hunk(app, file, split_rows(app, file, rows)),
    };
    view()
        .when(!terminal, |block| block.mb(px(14.0)))
        .child(file_head(app, file))
        .child(body)
        .when(terminal, |block| block.child(term::blank()))
        .into_element()
}

/// `.ev-file`: the mono path with its counts, the illustrative tag, and Ask about this hunk at
/// the trailing edge of the file that has a discussion.
fn file_head(app: AppState, file: &FileView) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let mut parts = vec![
        rich_at(app, &file.heading, 11.5, Token::Ink2, true)
            .truncate()
            .into_element(),
    ];
    if let Some(tag) = file.illustrative {
        if terminal {
            parts.push(term::space(1.0));
        }
        parts.push(illus(app, tag));
    }
    parts.push(view().flex_1().min_w(px(0.0)).into_element());
    if let Some(ask) = &file.ask {
        parts.push(command(app, ask));
    }
    view()
        .flex_row()
        .items_center()
        .when(!terminal, |head| head.gap(px(8.0)).mb(px(6.0)))
        .children(parts)
        .into_element()
}

/// The framed hunk: header on the sunk ground, then the rows. A terminal has no frame: the
/// header quiet, the rows under it.
fn hunk(app: AppState, file: &FileView, rows: Vec<AnyElement>) -> AnyElement {
    if app.form == Form::Terminal {
        return view()
            .children(file.hunk_header.map(|header| clip(term::quiet(header))))
            .children(rows)
            .into_element();
    }
    view()
        .rounded(px(6.0))
        .border_1()
        .border_color(color(Token::Rule))
        .bg(color(Token::Surface))
        .overflow_hidden()
        .children(file.hunk_header.map(|header| {
            view()
                .px(px(10.0))
                .py(px(3.0))
                .bg(color(Token::BgSunk))
                .border_b(px(1.0))
                .border_color(color(Token::Rule))
                .child(clip(code(app, header).text_color(color(Token::Ink3))))
                .into_element()
        }))
        .children(rows)
        .into_element()
}

/// Unified rows, the discussion after the row the model attaches it to.
fn unified_rows(app: AppState, file: &FileView, rows: &[UnifiedRow]) -> Vec<AnyElement> {
    let mut out = Vec::with_capacity(rows.len() + 1);
    for row in rows {
        out.push(unified_line(app, &row.line));
        if row.comment.is_some() {
            out.extend(file.discussion.as_ref().map(|talk| discussion(app, talk)));
        }
    }
    out
}

/// Side-by-side rows, the discussion after the row the model attaches it to.
fn split_rows(app: AppState, file: &FileView, rows: &[SplitRow]) -> Vec<AnyElement> {
    let mut out = Vec::with_capacity(rows.len() + 1);
    for row in rows {
        out.push(
            view()
                .flex_row()
                .child(half(app, row.left.as_ref(), false))
                .child(half(app, row.right.as_ref(), true))
                .into_element(),
        );
        if row.comment.is_some() {
            out.extend(file.discussion.as_ref().map(|talk| discussion(app, talk)));
        }
    }
    out
}

/// A unified row: old and new numbers (the phone keeps only the new, as the mockup hides the
/// first column), the sign and the code on the line's tint.
fn unified_line(app: AppState, line: &NumberedLine) -> AnyElement {
    let form = app.form;
    if form == Form::Terminal {
        return terminal_line(line);
    }
    view()
        .flex_row()
        .bg(tint(line.kind))
        .when(form == Form::Desktop, |row| {
            row.child(gutter(app, line.old))
        })
        .child(gutter(app, line.new))
        .child(sign(app, line))
        .child(code_cell(app, line))
        .into_element()
}

/// One side of a split row; a missing side is the sunk ground, not a context line.
fn half(app: AppState, line: Option<&NumberedLine>, new_side: bool) -> AnyElement {
    let cell = view()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(LINE))
        .flex_row()
        .when(new_side, |cell| {
            cell.border_l(px(1.0)).border_l_color(color(Token::Rule))
        });
    match line {
        None => cell.bg(color(Token::BgSunk)).into_element(),
        Some(line) => cell
            .bg(tint(line.kind))
            .child(gutter(app, if new_side { line.new } else { line.old }))
            .child(sign(app, line))
            .child(code_cell(app, line))
            .into_element(),
    }
}

/// A terminal row in Cells: both numbers four wide, the sign, the code; colour says added or
/// deleted, the sign says it again, the highlighted word is bold.
fn terminal_line(line: &NumberedLine) -> AnyElement {
    let ink = color(match line.kind {
        LineKind::Add => Token::Ok,
        LineKind::Del => Token::Fail,
        LineKind::Context => Token::Ink,
    });
    let number = |n: Option<u32>| n.map_or_else(|| "    ".to_owned(), |n| format!("{n:>4}"));
    let (before, word, after) = pieces(line);
    clip(
        text("")
            .child(
                text(format!("{} {} ", number(line.old), number(line.new)))
                    .text_color(color(Token::Ink3)),
            )
            .child(text(format!("{:<1} ", line.sign)).text_color(ink))
            .child(text(before).text_color(ink))
            .children(word.map(|word| text(word).font_weight(FontWeight::BOLD).text_color(ink)))
            .child(text(after).text_color(ink)),
    )
}

/// One line of code that never wraps: laid out wider than any line of the patch, then clipped
/// by its box at the cell's edge. A one-line clamp alone would wrap the code at a word and cut
/// it there, well before the edge. The box stretches across its column.
fn clip(line: Text) -> AnyElement {
    view()
        .min_w(px(0.0))
        .overflow_hidden()
        .child(
            line.w(px(UNWRAPPED))
                .flex_shrink_0()
                .number_of_lines(1u32)
                .text_overflow(EllipsizeMode::Clip),
        )
        .into_element()
}

/// The code before, of and after the highlighted word, by the model's byte range.
fn pieces(line: &NumberedLine) -> (&'static str, Option<&'static str>, &'static str) {
    let code = line.text;
    line.highlight
        .and_then(|mark| {
            Some((
                code.get(..mark.start)?,
                code.get(mark.start..mark.end)?,
                code.get(mark.end..)?,
            ))
        })
        .map_or((code, None, ""), |(before, word, after)| {
            (before, Some(word), after)
        })
}

fn tint(kind: LineKind) -> AdaptiveColor {
    match kind {
        LineKind::Add => color(Token::OkBg),
        LineKind::Del => color(Token::FailBg),
        LineKind::Context => AdaptiveColor::TRANSPARENT,
    }
}

fn word_fill(kind: LineKind) -> AdaptiveColor {
    match kind {
        LineKind::Add => color(Token::Ok).opacity(WORD_ALPHA),
        LineKind::Del => color(Token::Fail).opacity(WORD_ALPHA),
        LineKind::Context => AdaptiveColor::TRANSPARENT,
    }
}

/// A line of code text: mono at the diff's size and line height, one line.
fn code(app: AppState, content: impl Into<SharedString>) -> Text {
    let size = code_size(app.form);
    text(content.into())
        .number_of_lines(1u32)
        .font_family(FONT_MONO)
        .text_size(px(size))
        .line_height(px(LINE))
}

/// A line number, right-aligned in its column, faint.
fn gutter(app: AppState, number: Option<u32>) -> AnyElement {
    view()
        .w(px(GUTTER))
        .flex_shrink_0()
        .pr(px(8.0))
        .items_end()
        .accessibility_elements_hidden(true)
        .child(
            code(app, number.map(|n| n.to_string()).unwrap_or_default())
                .tabular_nums()
                .text_color(color(Token::Ink4)),
        )
        .into_element()
}

fn sign(app: AppState, line: &NumberedLine) -> AnyElement {
    let ink = match line.kind {
        LineKind::Add => color(Token::Ok),
        LineKind::Del => color(Token::Fail),
        LineKind::Context => color(Token::Ink4),
    };
    view()
        .w(px(SIGN))
        .flex_shrink_0()
        .child(code(app, line.sign).text_color(ink))
        .into_element()
}

fn code_cell(app: AppState, line: &NumberedLine) -> AnyElement {
    let (before, word, after) = pieces(line);
    view()
        .flex_1()
        .min_w(px(0.0))
        .pr(px(10.0))
        .child(clip(
            code(app, "")
                .text_color(color(Token::Ink))
                .child(before)
                .children(word.map(|word| text(word).bg(word_fill(line.kind)).rounded(px(2.0))))
                .child(after),
        ))
        .into_element()
}

/// The inline discussion (`.cm`): the attributed comment card, then the owner's reply box with
/// its keycap. The reply box is the model's inert affordance: pressing it runs its action, which
/// opens no editor.
fn discussion(app: AppState, talk: &CommentView) -> AnyElement {
    let form = app.form;
    let reply = act(app, talk.action);
    let test_id = button_id(talk.action);
    if form == Form::Terminal {
        let content = view()
            .child(rich_at(app, &talk.attribution, 0.0, Token::Ink3, false))
            .child(text(talk.text).text_color(color(Token::Ink)))
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .role(AccessibilityRole::Button)
                    .label(talk.placeholder)
                    .test_id(&test_id)
                    .cursor_pointer()
                    .on_press(move || reply())
                    .child(
                        term::quiet(talk.placeholder)
                            .flex_shrink(1.0)
                            .min_w(px(0.0))
                            .truncate(),
                    )
                    .child(view().flex_1().min_w(px(0.0)))
                    .child(term::key_cap(talk.keycap)),
            )
            .into_element();
        return view()
            .pl(term::cells(2.0))
            .child(panel(app, None, content))
            .into_element();
    }
    let card = |s: View| {
        s.max_w(px(COMMENT_MAX))
            .px(px(12.0))
            .py(px(8.0))
            .rounded(px(8.0))
            .border_1()
            .bg(color(Token::Surface))
    };
    view()
        .pl(px(COMMENT_INDENT))
        .pr(px(14.0))
        .py(px(10.0))
        .bg(color(Token::Bg))
        .border_t(px(1.0))
        .border_b(px(1.0))
        .border_color(color(Token::Rule))
        .child(
            card(view())
                .border_color(color(Token::Rule2))
                .child(rich_at(app, &talk.attribution, 11.5, Token::Ink3, false).mb(px(3.0)))
                .child(
                    text(talk.text)
                        .font_family(FONT_TEXT)
                        .text_size(px(12.5))
                        .line_height(px(19.0))
                        .text_color(color(Token::Ink)),
                ),
        )
        .child(
            card(view())
                .mt(px(6.0))
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .when(form == Form::Phone, |reply| reply.min_h(px(TARGET)))
                .border_color(color(Token::Rule))
                .role(AccessibilityRole::Button)
                .label(talk.placeholder)
                .test_id(&test_id)
                .cursor_pointer()
                .hover(|s| s.bg(color(Token::Hover)))
                .focus_visible(ring)
                .on_press(move || reply())
                .child(
                    text(talk.placeholder)
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .font_family(FONT_TEXT)
                        .text_size(px(12.5))
                        .text_color(color(Token::Ink3)),
                )
                .child(keycap(talk.keycap)),
        )
        .into_element()
}

/// `.kbd`: the key in mono on a hairline cap.
fn keycap(key: &'static str) -> AnyElement {
    view()
        .flex_shrink_0()
        .px(px(4.0))
        .rounded(px(3.0))
        .border_1()
        .border_color(color(Token::Rule2))
        .accessibility_elements_hidden(true)
        .child(
            text(key)
                .font_family(FONT_MONO)
                .text_size(px(10.5))
                .line_height(px(15.0))
                .text_color(color(Token::Ink3)),
        )
        .into_element()
}

/// A collapsed file: the whole framed block is the button, a chevron before the model's
/// changed-lines label. A terminal shows the model's button as the kit draws it.
fn collapsed(app: AppState, button: &Button) -> AnyElement {
    let form = app.form;
    if form == Form::Terminal {
        return command(app, button);
    }
    let press = act(app, button.action);
    view()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .px(px(12.0))
        .py(px(8.0))
        .when(form == Form::Phone, |block| block.min_h(px(TARGET)))
        .rounded(px(6.0))
        .border_1()
        .border_color(color(Token::Rule))
        .bg(color(Token::Surface))
        .role(AccessibilityRole::Button)
        .label(button.label.clone())
        .test_id(&button_id(button.action))
        .cursor_pointer()
        .hover(|s| s.bg(color(Token::Hover)))
        .focus_visible(ring)
        .on_press(move || press())
        .child(icon(Icon::Chevron, 14.0, color(Token::Ink3)))
        .child(
            text(button.label.clone())
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .font_family(FONT_TEXT)
                .text_size(px(12.0))
                .text_color(color(Token::Ink3)),
        )
        .into_element()
}
