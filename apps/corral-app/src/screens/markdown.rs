//! Words as the terminal UI renders them, laid out by NSH's Markdown `TextView` in the app's
//! type: headings, paragraphs, lists, block quotes, inline bold, italic, code and links, tables,
//! and code blocks with a muted language label and no fence characters. Chat tables and
//! narrow tables become records (`header  cell` rows), so their contents can wrap.
//! Shared by every transcript that shows written Markdown: a watched session's and the
//! assistant's.

use nsh_base::text_view::{
    Block, CodeBlock, Document, Inline, MarkdownOptions, Parser, Table, TextViewPart as Part,
    TextViewParts, text_view,
};
use nsh_core::prelude::*;

use crate::model::Form;
use crate::state::AppState;
use crate::ui::term;
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, text_size};

/// Below this page width the existing non-chat table layout uses records.
const TABLE_CELLS: f32 = 54.0;

/// The app's type for every text view drawn under the calling view: body text in ink at the
/// form's size, semibold headings a step or two larger, inline code in mono on a quiet fill,
/// links in the focus colour, quotes behind a rule. A terminal keeps its own type.
pub(crate) fn provide_styles(app: AppState) {
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
            Block::Details {
                summary,
                children,
                open,
            } => vec![Block::Details {
                summary,
                children: records(children),
                open,
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

/// Every image of `document`, in its blocks and its footnotes, replaced by its alt text.
fn without_images(document: &mut Document) {
    blocks_without_images(&mut document.blocks);
    for note in &mut document.footnotes {
        blocks_without_images(&mut note.children);
    }
}

/// Every image among `blocks`, at any depth, replaced by its alt text.
fn blocks_without_images(blocks: &mut [Block]) {
    for block in blocks {
        match block {
            Block::Paragraph { content } | Block::Heading { content, .. } => {
                inlines_without_images(content);
            }
            Block::Blockquote { children, .. } => blocks_without_images(children),
            Block::List { items, .. } => {
                for item in items {
                    blocks_without_images(&mut item.children);
                }
            }
            Block::Table(table) => {
                for cell in table.head.iter_mut().chain(table.rows.iter_mut().flatten()) {
                    inlines_without_images(&mut cell.content);
                }
            }
            Block::Details {
                summary, children, ..
            } => {
                inlines_without_images(summary);
                blocks_without_images(children);
            }
            _ => {}
        }
    }
}

fn inlines_without_images(inlines: &mut [Inline]) {
    for inline in inlines {
        if let Inline::Image(image) = inline {
            let alt = std::mem::take(&mut image.alt);
            *inline = Inline::text(alt);
            continue;
        }
        match inline {
            Inline::Strong { content }
            | Inline::Emphasis { content }
            | Inline::Strikethrough { content }
            | Inline::Underline { content }
            | Inline::Superscript { content }
            | Inline::Small { content }
            | Inline::Subscript { content }
            | Inline::Highlight { content, .. }
            | Inline::Link { content, .. } => inlines_without_images(content),
            _ => {}
        }
    }
}

// Match NSH's standalone-image paragraph rule, including linked groups of images.
fn only_images(content: &[Inline]) -> bool {
    fn walk(content: &[Inline], found: &mut bool) -> bool {
        content.iter().all(|inline| match inline {
            Inline::Image(_) => {
                *found = true;
                true
            }
            Inline::Link { content, .. } => walk(content, found),
            _ => false,
        })
    }
    let mut found = false;
    walk(content, &mut found) && found
}

fn record_blocks(table: &Table) -> Vec<Block> {
    if table.rows.is_empty() {
        return table
            .head
            .iter()
            .map(|cell| Block::paragraph(cell.content.clone()))
            .collect();
    }
    table
        .rows
        .iter()
        .flat_map(|row| {
            let mut content = Vec::new();
            let mut blocks = Vec::new();
            for (at, cell) in row.iter().enumerate() {
                if !content.is_empty()
                    && !table
                        .head
                        .get(at)
                        .is_some_and(|head| only_images(&head.content))
                {
                    content.push(Inline::Break);
                }
                if let Some(head) = table.head.get(at) {
                    if only_images(&head.content) {
                        if !content.is_empty() {
                            blocks.push(Block::paragraph(std::mem::take(&mut content)));
                        }
                        blocks.push(Block::paragraph(head.content.clone()));
                    } else {
                        content.push(Inline::Strong {
                            content: head.content.clone(),
                        });
                        content.push(Inline::Text { text: "  ".into() });
                    }
                }
                // Standalone image paragraphs retain NSH's natural sizing.
                if only_images(&cell.content) {
                    if !content.is_empty() {
                        blocks.push(Block::paragraph(std::mem::take(&mut content)));
                    }
                    blocks.push(Block::paragraph(cell.content.clone()));
                } else {
                    content.extend(cell.content.iter().cloned());
                }
            }
            if !content.is_empty() {
                blocks.push(Block::paragraph(content));
            }
            blocks
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
                view().pl(term::cells(2.0)).child(
                    text(code.code.clone())
                        .select_text()
                        .text_color(color(Token::Ink)),
                ),
            )
            .into_element();
    }
    let label = if app.form == Form::Phone { 12.5 } else { 11.0 };
    view()
        .rounded(px(6.0))
        .border_1()
        .border_color(color(Token::Rule))
        .bg(color(Token::BgSunk))
        .overflow_hidden()
        .children(language.map(|language| {
            view().px(px(10.0)).pt(px(6.0)).child(
                text(language)
                    .text_color(color(Token::Ink3))
                    .font_family(FONT_MONO)
                    .text_size(px(label)),
            )
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
pub(crate) fn markdown(app: AppState, markdown: &str, streaming: bool) -> AnyElement {
    laid_out(app, markdown, streaming, true, false)
}

/// Chat messages use records at every width: their available width is not the page width.
pub(crate) fn chat_markdown(app: AppState, markdown: &str, streaming: bool) -> AnyElement {
    laid_out(app, markdown, streaming, true, true)
}

/// `markdown` laid out, every image as its alt text: for stored words someone else wrote (a
/// reviewer's note), which must not make the device fetch a URL just by being shown.
pub(crate) fn markdown_without_images(app: AppState, markdown: &str) -> AnyElement {
    laid_out(app, markdown, false, false, false)
}

fn laid_out(
    app: AppState,
    markdown: &str,
    streaming: bool,
    images: bool,
    chat: bool,
) -> AnyElement {
    let options = MarkdownOptions {
        math: false,
        html: false,
        alerts: false,
        streaming,
        ..MarkdownOptions::default()
    };
    let mut document = Parser::new().options(options).markdown(markdown);
    if !images {
        without_images(&mut document);
    }
    if chat
        || app.form == Form::Phone
        || (app.form == Form::Terminal && term::page_columns(app) < TABLE_CELLS)
    {
        document.blocks = records(std::mem::take(&mut document.blocks));
        for note in &mut document.footnotes {
            note.children = records(std::mem::take(&mut note.children));
        }
    }
    let rendered = text_view()
        .document(document)
        .selectable(app.form == Form::Terminal)
        .render_code_block(move |code: &CodeBlock| code_block(app, code))
        .into_element();
    if app.form == Form::Terminal {
        crate::ui::composer::terminal_selectable(app, view().selectable(true))
            .child(rendered)
            .into_element()
    } else {
        rendered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_image_cells_remain_standalone() {
        let document = Parser::new()
            .markdown("| Picture | Caption |\n| --- | --- |\n| ![shot](image.png) | example |\n");
        let blocks = records(document.blocks);
        assert!(blocks.iter().any(|block| matches!(block, Block::Paragraph { content } if matches!(content.as_slice(), [Inline::Image(_)]))));
        assert!(serde_json::to_string(&blocks).unwrap().contains("example"));
    }

    /// A stored note must not fetch a URL when shown: an image anywhere, a footnote's
    /// included, is drawn as its alt text.
    #[test]
    fn a_note_without_images_keeps_no_image_url_in_its_body_or_footnotes() {
        let mut document = Parser::new().markdown(
            "**See ![shot](https://example.invalid/a.png)** and note[^1].\n\n\
             - > ![quoted](https://example.invalid/b.png)\n\n\
             [^1]: ![pixel](https://example.invalid/c.png)\n",
        );
        assert_eq!(document.footnotes.len(), 1, "the footnote is parsed apart");
        without_images(&mut document);
        let json = serde_json::to_string(&document).unwrap();
        assert!(!json.contains("example.invalid"), "{json}");
        assert!(json.contains("pixel") && json.contains("shot"), "{json}");
    }
}
