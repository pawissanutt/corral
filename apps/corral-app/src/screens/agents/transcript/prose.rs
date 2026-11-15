//! Words as the terminal UI renders them, laid out by NSH's Markdown `TextView` in the app's
//! type: headings, paragraphs, lists, block quotes, inline bold, italic, code and links, tables,
//! and code blocks with a muted language label and no fence characters. Narrow widths turn a
//! table into one block per record (`header  cell` rows), as the terminal does below 50 Cells.
//! Thinking reads muted under its lead and folds to one row.

use nsh_base::text_view::{
    Block, CodeBlock, Inline, MarkdownOptions, Parser, Table, TextViewPart as Part, TextViewParts,
    text_view,
};
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::agents::{Prose, Thinking};
use crate::state::AppState;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, text_size};

use super::PARAGRAPH;
use super::parts::{disclosure, pressable, small, state};

/// One blink of the streaming caret, as the assistant thread's.
const BLINK_MS: f32 = 1000.0;
/// Cells of transcript width below which a table becomes record blocks (the terminal UI's 50,
/// plus the message's indent).
const TABLE_CELLS: f32 = 54.0;

/// The app's type for every text view drawn under the calling view: body text in ink at the
/// form's size, semibold headings a step or two larger, inline code in mono on a quiet fill,
/// links in the focus colour, quotes behind a rule. A terminal keeps its own type.
pub(super) fn provide_styles(app: AppState) {
    if app.form == Form::Terminal {
        return;
    }
    let size = text_size(app.form);
    provide_context(
        TextViewParts::new()
            .part(Part::Root, move |s| {
                s.font_family(FONT_TEXT)
                    .text_size(px(size))
                    .line_height(px((size * 1.5).round()))
                    .text_color(color(Token::Ink))
            })
            .part(Part::Heading, |s| {
                s.font_weight(FontWeight::SEMIBOLD)
                    .text_color(color(Token::Ink))
            })
            .part(Part::Heading1, move |s| s.text_size(px(size + 3.0)))
            .part(Part::Heading2, move |s| s.text_size(px(size + 1.5)))
            .part(Part::Heading3, move |s| s.text_size(px(size + 0.5)))
            .part(Part::Strong, |s| s.font_weight(FontWeight::SEMIBOLD))
            .part(Part::Code, move |s| {
                s.font_family(FONT_MONO)
                    .text_size(px(size * 0.93))
                    .bg(color(Token::Surface2))
                    .rounded(px(3.0))
            })
            .part(Part::Link, |s| s.text_color(color(Token::Focus)))
            .part(Part::Blockquote, |s| {
                s.border_l(px(2.0))
                    .border_l_color(color(Token::Rule2))
                    .pl(px(10.0))
                    .text_color(color(Token::Ink2))
            })
            .part(Part::ListMarker, |s| s.text_color(color(Token::Ink3))),
    );
}

/// Whether the transcript is too narrow for a table's grid: the phone, or a terminal whose
/// page (after the sidebar and a docked Ask) is under the terminal UI's 50 Cells (tracked:
/// follows the size).
fn narrow(app: AppState) -> bool {
    match app.form {
        Form::Phone => true,
        Form::Desktop => false,
        Form::Terminal => term::page_columns(app) < TABLE_CELLS,
    }
}

/// Each table among `blocks` (and inside quotes and list items) as one paragraph per record:
/// every field on its own line, the header bold before the cell.
fn records(blocks: Vec<Block>) -> Vec<Block> {
    blocks
        .into_iter()
        .flat_map(|block| match block {
            Block::Table(table) => record_blocks(&table),
            Block::Blockquote { children, alert } => vec![Block::Blockquote {
                children: records(children),
                alert,
            }],
            Block::List {
                ordered,
                start,
                loose,
                mut items,
            } => {
                for item in &mut items {
                    item.children = records(std::mem::take(&mut item.children));
                }
                vec![Block::List {
                    ordered,
                    start,
                    loose,
                    items,
                }]
            }
            other => vec![other],
        })
        .collect()
}

fn record_blocks(table: &Table) -> Vec<Block> {
    table
        .rows
        .iter()
        .map(|row| {
            let mut content = Vec::new();
            for (at, cell) in row.iter().enumerate() {
                if at > 0 {
                    content.push(Inline::Break);
                }
                if let Some(head) = table.head.get(at) {
                    content.push(Inline::Strong {
                        content: head.content.clone(),
                    });
                    content.push(Inline::Text { text: "  ".into() });
                }
                content.extend(cell.content.iter().cloned());
            }
            Block::paragraph(content)
        })
        .collect()
}

/// A fenced code block without its fences: the language muted on its own row, then the code in
/// mono, scrolling sideways on a sunk fill; a terminal indents the code two Cells.
fn code_block(app: AppState, code: &CodeBlock) -> AnyElement {
    let language = code
        .language
        .as_deref()
        .map(str::trim)
        .filter(|language| !language.is_empty())
        .map(str::to_owned);
    if app.form == Form::Terminal {
        return view()
            .children(language.map(term::quiet))
            .child(
                view()
                    .pl(term::cells(2.0))
                    .child(text(code.code.clone()).text_color(color(Token::Ink))),
            )
            .into_element();
    }
    view()
        .rounded(px(6.0))
        .border_1()
        .border_color(color(Token::Rule))
        .bg(color(Token::BgSunk))
        .overflow_hidden()
        .children(language.map(|language| {
            view()
                .px(px(10.0))
                .pt(px(6.0))
                .child(small(app, language, Token::Ink3).font_family(FONT_MONO))
        }))
        .child(
            scroll().horizontal().child(
                view().flex_shrink_0().px(px(10.0)).py(px(8.0)).child(
                    text(code.code.clone())
                        .flex_shrink_0()
                        .font_family(FONT_MONO)
                        .text_size(px(11.5))
                        .line_height(px(17.0))
                        .text_color(color(Token::Ink)),
                ),
            ),
        )
        .into_element()
}

/// `markdown` laid out; while `streaming`, syntax its end leaves open reads as closed. Text
/// and HTML-looking words stay literal, as the terminal shows them: no formulas, no markup.
pub(super) fn markdown(app: AppState, markdown: &str, streaming: bool) -> AnyElement {
    let options = MarkdownOptions {
        math: false,
        html: false,
        alerts: false,
        streaming,
        ..MarkdownOptions::default()
    };
    let mut document = Parser::new().options(options).markdown(markdown);
    if narrow(app) {
        document.blocks = records(std::mem::take(&mut document.blocks));
    }
    text_view()
        .document(document)
        .render_code_block(move |code: &CodeBlock| code_block(app, code))
        .into_element()
}

/// The streaming caret (`.caret`): a 7 by 14 pt block of ink, blinking; `▌` in a terminal.
pub(super) fn caret(app: AppState) -> AnyElement {
    if app.form == Form::Terminal {
        return text("▌")
            .text_color(color(Token::Ink))
            .accessibility_elements_hidden(true)
            .into_element();
    }
    let blink = motion().entering(
        MotionSpec::new(KeyframeTiming::new(BLINK_MS).infinite())
            .values(MotionChannel::Opacity, [1.0, 0.0, 1.0]),
    );
    view()
        .w(px(7.0))
        .h(px(14.0))
        .bg(color(Token::Ink))
        .accessibility_elements_hidden(true)
        .bind_motion(&blink)
        .into_element()
}

/// Words of a message: rendered Markdown at most ~70 characters wide on the desktop, then
/// where the rest of a continued item stands.
pub(super) fn prose(app: AppState, words: &Prose, n: usize) -> AnyElement {
    view()
        .min_w(px(0.0))
        .when(app.form == Form::Desktop, |block| {
            block.max_w(px(PARAGRAPH))
        })
        .when(app.form != Form::Terminal, |block| block.mb(px(6.0)))
        .child(markdown(app, &words.markdown, words.streaming))
        .children(state(app, &words.more, n, true))
        .into_element()
}

/// Thinking: a `thinking` lead, then the reasoning muted so it never reads as the answer;
/// folded, one row says what it hides. Pressing the lead folds or shows it.
pub(super) fn thinking(app: AppState, thought: &Thinking, n: usize) -> AnyElement {
    let form = app.form;
    let lead = view()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .when(form != Form::Terminal, |row| {
            row.py(px(2.0)).rounded(px(4.0))
        })
        .child(
            small(app, "thinking", Token::Ink3).font_weight(if form == Form::Terminal {
                FontWeight::BOLD
            } else {
                FontWeight::SEMIBOLD
            }),
        )
        .child(disclosure(app, thought.open))
        .when(!thought.open, |row| {
            row.child(small(app, thought.hidden.clone(), Token::Ink3))
        })
        .when(thought.streaming && !thought.open, |row| {
            row.child(caret(app))
        });
    let lead = pressable(
        app,
        lead,
        Some(&thought.action),
        "Thinking".into(),
        thought.open,
        n,
    );
    let reasoning = thought.open.then(|| {
        let words = text(thought.text.clone()).text_color(color(Token::Ink3));
        let words = if form == Form::Terminal {
            words
        } else {
            words
                .font_family(FONT_TEXT)
                .text_size(px(if form == Form::Phone { 14.0 } else { 12.5 }))
                .line_height(px(if form == Form::Phone { 20.0 } else { 18.0 }))
        };
        view()
            .min_w(px(0.0))
            .when(form == Form::Desktop, |block| block.max_w(px(PARAGRAPH)))
            .child(words)
            .when(thought.streaming, |block| block.child(caret(app)))
    });
    view()
        .min_w(px(0.0))
        .when(form != Form::Terminal, |block| block.mb(px(6.0)))
        .child(lead)
        .children(reasoning)
        .children(state(app, &thought.more, n, true))
        .into_element()
}
