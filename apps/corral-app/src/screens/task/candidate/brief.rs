//! "Ready to land?" as a chain: the gate, the independent review with its note quoted, the
//! owner's verdict, each a mark on a line down to the next. The task page's Checks and a
//! session's Proof draw it.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::review::{ReadyLink, ReviewQuote};
use crate::screens::markdown::{markdown_without_images, provide_styles};
use crate::state::AppState;
use crate::ui::kit::hint::hinted;
use crate::ui::kit::marks::mark;
use crate::ui::kit::text::mono;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, MEASURE_PROSE, Token, color};

/// A chain link's mark column and the line that joins it to the next (`.ready li`).
const MARK_COLUMN: f32 = 18.0;
const MARK_ROW: f32 = 20.0;

/// `.ready`: each link's mark in its hue on a rule-2 line down to the next, the title (its
/// hint the receipt), the candidate's SHA, a count of older results, the quoted note.
pub(crate) fn chain(app: AppState, links: &[ReadyLink]) -> AnyElement {
    view()
        .role(AccessibilityRole::List)
        .children(
            links
                .iter()
                .enumerate()
                .map(|(at, link)| chain_link(app, link, at + 1 == links.len()))
                .collect::<Vec<_>>(),
        )
        .into_element()
}

fn chain_link(app: AppState, link: &ReadyLink, last: bool) -> AnyElement {
    let form = app.form;
    let title = text(link.title)
        .number_of_lines(1u32)
        .font_weight(if form == Form::Desktop {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::BOLD
        })
        .text_color(color(Token::Ink))
        .when(form != Form::Terminal, |t| {
            t.font_family(FONT_TEXT)
                .text_size(px(if form == Form::Phone { 15.0 } else { 13.5 }))
        });
    let mut line: Vec<AnyElement> = vec![hinted(app, title, link.hint.clone())];
    line.extend(
        link.sha7
            .clone()
            .map(|sha| mono(app, sha).text_color(color(Token::Ink3)).into_element()),
    );
    let quote = link.quote.as_ref().map(|quote| quoted(app, quote));
    if form == Form::Terminal {
        return view()
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .child(mark(app, link.mark, link.hue))
                    .children(line.into_iter().map(|part| {
                        view()
                            .flex_row()
                            .flex_shrink_0()
                            .child(term::space(1.0))
                            .child(part)
                            .into_element()
                    })),
            )
            .children(quote)
            .into_element();
    }
    view()
        .relative()
        .flex_row()
        .gap(px(10.0))
        .when(!last, |item| {
            item.pb(px(16.0)).child(
                view()
                    .absolute()
                    .left(px(MARK_COLUMN / 2.0 - 0.5))
                    .top(px(MARK_ROW))
                    .bottom(px(2.0))
                    .w(px(1.0))
                    .bg(color(Token::Rule2)),
            )
        })
        .child(
            view()
                .w(px(MARK_COLUMN))
                .h(px(MARK_ROW))
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .child(mark(app, link.mark, link.hue)),
        )
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.0))
                        .min_h(px(MARK_ROW))
                        .children(line),
                )
                .children(quote),
        )
        .into_element()
}

/// The reviewer's note in a quiet bordered block, who said it above it (`.ready blockquote`),
/// kept to a readable line. Reviewers write Markdown (headings, lists, code spans), so it is
/// laid out as such; a long path breaks within the block rather than running past it. A
/// terminal sets it two Cells in, who quiet over the words.
fn quoted(app: AppState, quote: &ReviewQuote) -> AnyElement {
    let form = app.form;
    if form == Form::Terminal {
        return view()
            .pl(term::cells(2.0))
            .child(term::quiet(quote.by.clone()))
            .child(text(quote.text.clone()).text_color(color(Token::Ink)))
            .into_element();
    }
    provide_styles(app);
    view()
        .mt(px(8.0))
        .max_w(px(MEASURE_PROSE))
        .min_w(px(0.0))
        .px(px(12.0))
        .py(px(8.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(color(Token::Rule2))
        .bg(color(Token::Surface))
        .gap(px(2.0))
        .child(
            text(quote.by.clone())
                .font_family(FONT_TEXT)
                .text_size(px(if form == Form::Phone { 13.0 } else { 12.0 }))
                .text_color(color(Token::Ink3)),
        )
        .child(markdown_without_images(app, &quote.text))
        .into_element()
}
