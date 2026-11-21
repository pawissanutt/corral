//! A Code Mode cell (`code-cell.ts`): its header in the tool row grammar (`code 7 calls ✓5 ⠋2 ·
//! read ×3, bash  cell 12  14s`), the calls the cell made two Cells in, each drawn as a direct
//! call of its tool is, folded runs as one muted row, then what the script printed or returned
//! and its error under muted labels; opened, every call and the script under `source`.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::hue::Hue;
use crate::model::session::transcript::{CellOutput, CellRow, CodeCell, ToolState};
use crate::state::AppState;
use crate::ui::kit::marks::hue_ink;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, Token, color};

use super::parts::{disclosure, glyph, pressable, small, state, tinted, well};
use super::tool::{line, tail};

fn mono(app: AppState, words: impl Into<SharedString>, ink: AdaptiveColor) -> Text {
    let words = text(words.into()).text_color(ink);
    if app.form == Form::Terminal {
        words
    } else {
        words.font_family(FONT_MONO).text_size(px(11.5))
    }
}

/// `✓5`, a spinner and `2`, `✗1`: each count after its state's glyph.
fn tally(app: AppState, counts: &[(ToolState, u64)]) -> Vec<AnyElement> {
    counts
        .iter()
        .map(|(state, count)| {
            view()
                .flex_row()
                .items_center()
                .flex_shrink_0()
                .child(glyph(app, *state))
                .child(tinted(app, count.to_string(), state.hue()))
                .into_element()
        })
        .collect()
}

fn header(app: AppState, cell: &CodeCell) -> View {
    let form = app.form;
    let gap = if form == Form::Terminal { 1.0 } else { 8.0 };
    // A terminal spaces the parts by a Cell; the page by the row's gap.
    let space = move || (form == Form::Terminal).then(|| term::space(1.0));
    let mut row = view()
        .flex_row()
        .items_center()
        .min_w(px(0.0))
        .when(form != Form::Terminal, |row| {
            row.gap(px(gap)).px(px(10.0)).py(px(5.0)).min_h(px(26.0))
        })
        .child(glyph(app, cell.state))
        .children(space())
        .child(
            mono(app, "code", color(Token::Ink))
                .flex_shrink_0()
                .font_weight(if form == Form::Terminal {
                    FontWeight::BOLD
                } else {
                    FontWeight::SEMIBOLD
                }),
        );
    if !cell.calls.is_empty() {
        row = row
            .children(space())
            .child(mono(app, cell.calls.clone(), color(Token::Ink2)).flex_shrink_0());
    }
    for count in tally(app, &cell.tally) {
        row = row.children(space()).child(count);
    }
    if !cell.tools.is_empty() {
        row = row.child(
            mono(app, format!(" · {}", cell.tools), color(Token::Ink3))
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .truncate(),
        );
    }
    if let Some(ended) = &cell.ended {
        row = row.child(
            mono(app, format!(" · {ended}"), hue_ink(Hue::Fail))
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .truncate(),
        );
    }
    row.child(view().flex_1().min_w(px(0.0)))
        .children(cell.label.clone().map(|label| {
            view()
                .flex_row()
                .flex_shrink_0()
                .children(space())
                .child(small(app, label, Token::Ink3))
        }))
        .children(cell.time.clone().map(|time| {
            view()
                .flex_row()
                .flex_shrink_0()
                .children(space())
                .child(small(app, time, Token::Ink3))
        }))
        .when(cell.action.is_some(), |row| {
            row.children(space()).child(disclosure(app, cell.open))
        })
}

fn output(app: AppState, part: &CellOutput) -> AnyElement {
    let ink = if part.hue == Hue::Neutral {
        color(Token::Ink)
    } else {
        hue_ink(part.hue)
    };
    view()
        .min_w(px(0.0))
        .child(small(app, part.label, Token::Ink3))
        .child(
            view()
                .when(app.form == Form::Terminal, |block| {
                    crate::ui::composer::terminal_selectable(app, block.selectable(true))
                        .pl(term::cells(2.0))
                })
                .child(mono(app, part.text.clone(), ink).select_text()),
        )
        .into_element()
}

pub(super) fn cell(app: AppState, cell: &CodeCell, id: &str) -> AnyElement {
    let form = app.form;
    let head = pressable(
        app,
        header(app, cell),
        cell.action.as_ref(),
        format!("code cell, {}", cell.calls),
        cell.open,
        id,
    );
    let writing = (!cell.writing.is_empty()).then(|| {
        view().children(
            cell.writing
                .iter()
                .map(|row| mono(app, row.clone(), color(Token::Ink3)).truncate()),
        )
    });
    let note = cell.note.clone().map(|note| small(app, note, Token::Ink3));
    let children = cell.children.iter().enumerate().map(|(k, row)| {
        let child = format!("{id}-{k}");
        match row {
            CellRow::Call(call) => view()
                .test_id(&format!("item-{child}"))
                .min_w(px(0.0))
                .child(line(app, call, &child))
                .children(tail(app, call))
                .into_element(),
            CellRow::Fold { state, words } => view()
                .flex_row()
                .items_center()
                .gap(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
                .when(form != Form::Terminal, |row| row.px(px(10.0)).py(px(2.0)))
                .child(glyph(app, *state))
                .when(form == Form::Terminal, |row| row.child(term::space(1.0)))
                .child(small(app, words.clone(), Token::Ink3))
                .into_element(),
        }
    });
    let outputs = cell.output.iter().map(|part| output(app, part));
    let source = cell.source.as_ref().map(|source| {
        view()
            .min_w(px(0.0))
            .child(small(app, "source", Token::Ink3))
            .child(well(app, source))
    });
    let body = view()
        .min_w(px(0.0))
        .gap(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
        .children(writing)
        .children(note)
        .children(children)
        .children(outputs)
        .children(source)
        .children(state(app, &cell.more, id, false));
    let body = if form == Form::Terminal {
        body.pl(term::cells(2.0))
    } else {
        body.px(px(10.0)).pb(px(8.0))
    };
    let block = view().min_w(px(0.0)).child(head).child(body);
    if form == Form::Terminal {
        return block.into_element();
    }
    block
        .mt(px(4.0))
        .mb(px(4.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(color(Token::Rule))
        .overflow_hidden()
        .into_element()
}
