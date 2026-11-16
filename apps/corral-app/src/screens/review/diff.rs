//! The Diff tab (`.diff`) and a Brief file's patch shown in place: each file's head, its hunks
//! with their headers, unified or side-by-side rows with their add/del tints and the
//! highlighted word, the discussions between the rows they follow, and collapsed, loading or
//! unavailable files. The rows are built here rather than with `nsh_base`'s DiffView because
//! the mockup sets a discussion card between two rows, which DiffView does not host. On a
//! served candidate a code line is pressable: it opens a new comment on that line.

use nsh_core::EllipsizeMode;
use nsh_core::prelude::*;

use crate::model::board::LineKind;
use crate::model::review::{
    Action, Button, CommentView, DiffContent, DiffView, FileBody, FileView, HunkView, LineAt,
    NumberedLine, SplitRow, UnifiedRow,
};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::chip::{chip, illus};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::page::panel;
use crate::ui::kit::text::dim;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

use super::compose::{composer, status_line};
use super::{Ctx, act, button_id, command, rich_at, ring};

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
pub(super) fn diff(cx: &Ctx, diff: &DiffView) -> AnyElement {
    let app = cx.app;
    view()
        .children(diff.notice.map(|notice| {
            view()
                .mx(px(4.0))
                .mb(px(8.0))
                .child(dim(app, notice))
                .into_element()
        }))
        .children(diff.empty.clone().map(|line| dim(app, line).into_element()))
        .children(diff.files.iter().map(|file| file_block(cx, file)))
        .into_element()
}

fn file_block(cx: &Ctx, file: &FileView) -> AnyElement {
    let terminal = cx.app.form == Form::Terminal;
    view()
        .when(!terminal, |block| block.mb(px(14.0)))
        .child(file_head(cx, file))
        .children(loose(cx, file))
        .child(file_patch(cx, file))
        .when(terminal, |block| block.child(term::blank()))
        .into_element()
}

/// An expanded file's patch in place under its Brief row, which carries its name, counts and
/// collapse: the illustrative tag and Ask at the trailing edge, the discussions shown away from
/// their lines, then the hunks.
pub(super) fn file_body(cx: &Ctx, file: &FileView) -> AnyElement {
    let app = cx.app;
    let terminal = app.form == Form::Terminal;
    let tools: Vec<AnyElement> = file
        .illustrative
        .map(|tag| illus(app, tag))
        .into_iter()
        .chain(file.ask.iter().map(|ask| command(cx, ask)))
        .collect();
    view()
        .when(!terminal, |block| {
            block.gap(px(8.0)).pt(px(8.0)).pb(px(4.0))
        })
        .when(!tools.is_empty(), |block| {
            block.child(
                view()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .gap(px(8.0))
                    .children(tools),
            )
        })
        .children(loose(cx, file))
        .child(file_patch(cx, file))
        .into_element()
}

/// The discussions whose line is not among the shown ones, above the hunks.
fn loose(cx: &Ctx, file: &FileView) -> Vec<AnyElement> {
    file.loose
        .iter()
        .filter_map(|&at| file.discussions.get(at))
        .map(|talk| discussion(cx, talk, true))
        .collect()
}

/// The file's patch: its hunks and notes, or the line that stands where they would be.
fn file_patch(cx: &Ctx, file: &FileView) -> AnyElement {
    let terminal = cx.app.form == Form::Terminal;
    match &file.body {
        FileBody::Collapsed(button) => collapsed(cx, button),
        FileBody::Loading(line) => quiet_box(cx, line.clone(), Hue::Neutral),
        FileBody::Unavailable(line) => quiet_box(cx, line.clone(), Hue::Fail),
        FileBody::Hunks { hunks, notes, tail } => view()
            .gap(px(if terminal { 0.0 } else { 8.0 }))
            .children(hunks.iter().map(|part| hunk(cx, file, part)))
            .children(
                notes
                    .iter()
                    .chain(tail.iter())
                    .map(|line| dim(cx.app, line.clone()).into_element()),
            )
            .into_element(),
    }
}

/// `.ev-file`: the mono path with its counts, cut in the middle so its root and file name both
/// stay, the illustrative tag, then Ask and Collapse at the trailing edge. A phone has no room
/// for all of them on one line: the path takes the whole first line, the rest the second.
fn file_head(cx: &Ctx, file: &FileView) -> AnyElement {
    let app = cx.app;
    let form = app.form;
    let path = rich_at(app, &file.heading, 11.5, Token::Ink2, true)
        .truncate()
        .text_overflow(EllipsizeMode::Middle)
        .into_element();
    let mut parts = Vec::with_capacity(6);
    if let Some(tag) = file.illustrative {
        if form == Form::Terminal {
            parts.push(term::space(1.0));
        }
        parts.push(illus(app, tag));
    }
    parts.push(view().flex_1().min_w(px(0.0)).into_element());
    parts.extend(file.ask.iter().map(|ask| command(cx, ask)));
    parts.extend(file.collapse.iter().map(|collapse| command(cx, collapse)));
    let row = view().flex_row().items_center();
    match form {
        Form::Terminal => row.child(path).children(parts).into_element(),
        Form::Desktop => row
            .gap(px(8.0))
            .mb(px(6.0))
            .child(path)
            .children(parts)
            .into_element(),
        Form::Phone => view()
            .gap(px(2.0))
            .mb(px(6.0))
            .child(path)
            .child(row.gap(px(8.0)).children(parts))
            .into_element(),
    }
}

/// A line where a patch would be: loading, or why it is not shown.
fn quiet_box(cx: &Ctx, line: String, hue: Hue) -> AnyElement {
    let app = cx.app;
    let words = dim(app, line);
    let words = if hue == Hue::Neutral {
        words
    } else {
        words.text_color(hue_ink(hue))
    };
    if app.form == Form::Terminal {
        return words.into_element();
    }
    view()
        .px(px(12.0))
        .py(px(8.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(color(Token::Rule))
        .bg(color(Token::Surface))
        .child(words)
        .into_element()
}

/// The framed hunk: header on the sunk ground, then the rows. A terminal has no frame: the
/// header quiet, the rows under it.
fn hunk(cx: &Ctx, file: &FileView, part: &HunkView) -> AnyElement {
    let app = cx.app;
    let rows = match &part.content {
        DiffContent::Unified(rows) => unified_rows(cx, file, rows),
        DiffContent::Split(rows) => split_rows(cx, file, rows),
    };
    if app.form == Form::Terminal {
        return view()
            .child(clip(term::quiet(part.header.clone())))
            .children(rows)
            .into_element();
    }
    view()
        .rounded(px(6.0))
        .border_1()
        .border_color(color(Token::Rule))
        .bg(color(Token::Surface))
        .overflow_hidden()
        .child(
            view()
                .px(px(10.0))
                .py(px(3.0))
                .bg(color(Token::BgSunk))
                .border_b(px(1.0))
                .border_color(color(Token::Rule))
                .child(clip(
                    code(app, part.header.clone()).text_color(color(Token::Ink3)),
                )),
        )
        .children(rows)
        .into_element()
}

fn placed(cx: &Ctx, file: &FileView, at: &[usize], out: &mut Vec<AnyElement>) {
    out.extend(
        at.iter()
            .filter_map(|&index| file.discussions.get(index))
            .map(|talk| discussion(cx, talk, false)),
    );
}

/// The line a press on `line` comments on: a deletion by its old number, else its new one.
fn line_at(file: &FileView, line: &NumberedLine, old_side: bool) -> Option<LineAt> {
    let old_side = old_side || line.kind == LineKind::Del;
    let number = if old_side { line.old } else { line.new }?;
    Some(LineAt {
        path: file.path.clone(),
        old_side,
        line: number,
    })
}

/// Makes a code row open a new comment on its line when the file allows it.
fn pressable(cx: &Ctx, file: &FileView, line: &NumberedLine, old_side: bool, row: View) -> View {
    let (Some(candidate), Some(at)) = (
        file.candidate.clone(),
        line_at(file, line, old_side).filter(|_| file.annotatable),
    ) else {
        return row;
    };
    let label = format!(
        "Comment on {} {}",
        if at.old_side { "old line" } else { "line" },
        at.line
    );
    let action = Action::Annotate { candidate, at };
    let test_id = button_id(&action);
    let press = act(cx, action);
    row.cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(label)
        .test_id(&test_id)
        .hover(|s| s.bg(color(Token::Hover)))
        .focus_visible(ring)
        .on_press(move || press())
}

/// Unified rows, each discussion after the row the model attaches it to.
fn unified_rows(cx: &Ctx, file: &FileView, rows: &[UnifiedRow]) -> Vec<AnyElement> {
    let mut out = Vec::with_capacity(rows.len() + 1);
    for row in rows {
        out.push(unified_line(cx, file, &row.line));
        placed(cx, file, &row.discussions, &mut out);
    }
    out
}

/// Side-by-side rows, each discussion after the row the model attaches it to.
fn split_rows(cx: &Ctx, file: &FileView, rows: &[SplitRow]) -> Vec<AnyElement> {
    let mut out = Vec::with_capacity(rows.len() + 1);
    for row in rows {
        out.push(
            view()
                .flex_row()
                .child(half(cx, file, row.left.as_ref(), false))
                .child(half(cx, file, row.right.as_ref(), true))
                .into_element(),
        );
        placed(cx, file, &row.discussions, &mut out);
    }
    out
}

/// A unified row: old and new numbers (the phone keeps only the new, as the mockup hides the
/// first column), the sign and the code on the line's tint.
fn unified_line(cx: &Ctx, file: &FileView, line: &NumberedLine) -> AnyElement {
    let app = cx.app;
    let form = app.form;
    let row = if form == Form::Terminal {
        view().child(terminal_line(line))
    } else {
        view()
            .flex_row()
            .bg(tint(line.kind))
            .when(form == Form::Desktop, |row| {
                row.child(gutter(app.form, line.old))
            })
            .child(gutter(app.form, line.new))
            .child(sign(app.form, line))
            .child(code_cell(app.form, line))
    };
    pressable(cx, file, line, false, row).into_element()
}

/// One side of a split row; a missing side is the sunk ground, not a context line.
fn half(cx: &Ctx, file: &FileView, line: Option<&NumberedLine>, new_side: bool) -> AnyElement {
    let form = cx.app.form;
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
        Some(line) => {
            let cell = cell
                .bg(tint(line.kind))
                .child(gutter(form, if new_side { line.new } else { line.old }))
                .child(sign(form, line))
                .child(code_cell(form, line));
            pressable(cx, file, line, !new_side, cell).into_element()
        }
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
            .child(text(before.to_owned()).text_color(ink))
            .children(word.map(|word| {
                text(word.to_owned())
                    .font_weight(FontWeight::BOLD)
                    .text_color(ink)
            }))
            .child(text(after.to_owned()).text_color(ink)),
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
fn pieces(line: &NumberedLine) -> (&str, Option<&str>, &str) {
    let code = line.text.as_str();
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
    code_text(app.form, content)
}

fn code_text(form: Form, content: impl Into<SharedString>) -> Text {
    text(content.into())
        .number_of_lines(1u32)
        .font_family(FONT_MONO)
        .text_size(px(code_size(form)))
        .line_height(px(LINE))
}

/// A line number, right-aligned in its column, faint.
fn gutter(form: Form, number: Option<u32>) -> AnyElement {
    view()
        .w(px(GUTTER))
        .flex_shrink_0()
        .pr(px(8.0))
        .items_end()
        .accessibility_elements_hidden(true)
        .child(
            code_text(form, number.map(|n| n.to_string()).unwrap_or_default())
                .tabular_nums()
                .text_color(color(Token::Ink4)),
        )
        .into_element()
}

fn sign(form: Form, line: &NumberedLine) -> AnyElement {
    let ink = match line.kind {
        LineKind::Add => color(Token::Ok),
        LineKind::Del => color(Token::Fail),
        LineKind::Context => color(Token::Ink4),
    };
    view()
        .w(px(SIGN))
        .flex_shrink_0()
        .child(code_text(form, line.sign).text_color(ink))
        .into_element()
}

/// The code of a row, wrapping inside its column rather than cut at its edge: a narrow window
/// or a phone shows the whole line over as many rows as it takes, the gutters and the tint
/// running down beside it, each side of a split row wrapping on its own.
fn code_cell(form: Form, line: &NumberedLine) -> AnyElement {
    let (before, word, after) = pieces(line);
    view()
        .flex_1()
        .min_w(px(0.0))
        .pr(px(10.0))
        .child(
            text("")
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .font_family(FONT_MONO)
                .text_size(px(code_size(form)))
                .line_height(px(LINE))
                .text_color(color(Token::Ink))
                .child(before.to_owned())
                .children(word.map(|word| {
                    text(word.to_owned())
                        .bg(word_fill(line.kind))
                        .rounded(px(2.0))
                }))
                .child(after.to_owned()),
        )
        .into_element()
}

/// A discussion (`.cm`): where it sits when shown away from its line, its messages as
/// attributed cards with their state tags, the thread's Resolve or Reopen, the lines of
/// commands sent about it, then the owner's composer.
fn discussion(cx: &Ctx, talk: &CommentView, away: bool) -> AnyElement {
    let app = cx.app;
    let form = app.form;
    let tags: Vec<AnyElement> = talk
        .anchor
        .iter()
        .filter(|_| away)
        .map(|anchor| illus(app, anchor.clone()))
        .chain(
            talk.chips
                .iter()
                .map(|tag| chip(app, tag.label.clone(), tag.hue)),
        )
        .collect();
    let tags = (!tags.is_empty()).then(|| {
        view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(6.0))
            .children(tags)
            .into_element()
    });
    let actions = (!talk.actions.is_empty()).then(|| {
        view()
            .flex_row()
            .flex_wrap()
            .gap(px(6.0))
            .children(talk.actions.iter().map(|button| command(cx, button)))
            .into_element()
    });
    let messages: Vec<AnyElement> = talk
        .messages
        .iter()
        .map(|message| {
            if form == Form::Terminal {
                view()
                    .child(rich_at(app, &message.attribution, 0.0, Token::Ink3, false))
                    .child(text(message.text.clone()).text_color(color(Token::Ink)))
                    .into_element()
            } else {
                card(view())
                    .border_color(color(Token::Rule2))
                    .child(rich_at(app, &message.attribution, 11.5, Token::Ink3, false).mb(px(3.0)))
                    .child(
                        text(message.text.clone())
                            .font_family(FONT_TEXT)
                            .text_size(px(if form == Form::Phone { 14.0 } else { 12.5 }))
                            .line_height(px(if form == Form::Phone { 20.0 } else { 19.0 }))
                            .text_color(color(Token::Ink)),
                    )
                    .into_element()
            }
        })
        .collect();
    let content = view()
        .gap(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
        .children(tags)
        .children(messages)
        .children(actions)
        .children(talk.status.iter().map(|line| status_line(cx, line)))
        .children(talk.composer.as_ref().map(|edit| {
            view()
                .max_w(px(COMMENT_MAX))
                .child(composer(cx, edit))
                .into_element()
        }));
    if form == Form::Terminal {
        return view()
            .pl(term::cells(2.0))
            .child(panel(app, None, content.into_element()))
            .into_element();
    }
    view()
        .pl(px(if away || form == Form::Phone {
            12.0
        } else {
            COMMENT_INDENT
        }))
        .pr(px(14.0))
        .py(px(10.0))
        .bg(color(Token::Bg))
        .border_t(px(1.0))
        .border_b(px(1.0))
        .border_color(color(Token::Rule))
        .when(away, |block| block.mb(px(8.0)).rounded(px(6.0)).border_1())
        .child(content)
        .into_element()
}

fn card(s: View) -> View {
    s.max_w(px(COMMENT_MAX))
        .px(px(12.0))
        .py(px(8.0))
        .rounded(px(8.0))
        .border_1()
        .bg(color(Token::Surface))
}

/// A collapsed file: the whole framed block is the button, a chevron before the model's
/// changed-lines label. A terminal shows the model's button as the kit draws it.
fn collapsed(cx: &Ctx, button: &Button) -> AnyElement {
    let form = cx.app.form;
    if form == Form::Terminal {
        return command(cx, button);
    }
    let press = act(cx, button.action.clone());
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
        .test_id(&button_id(&button.action))
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
