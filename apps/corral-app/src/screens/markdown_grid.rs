//! An artifact's tables on a desktop, laid out to the width they are given rather than to fixed
//! column floors: every column takes a share of the width by how much it holds, a short column
//! keeps its words on one line, a long one wraps, and the table scrolls sideways only when even
//! each column's narrowest is wider than the page. NSH's own grid keeps each column at least
//! half its widest line (up to 30 rem), so a report's table of two prose columns ran past a
//! 700-point column and hid its last column behind a scroll.
//!
//! Each table is lifted out of the document as a custom node ([`lift`]) and drawn by the
//! artifact's text view as [`table`]; its cells are text views of their own, so emphasis, code,
//! links and citations read as the artifact wrote them.

use std::rc::Rc;

use nsh_base::text_view::{Block, ColumnAlign, CustomNode, Document, Inline, Table, text_view};
use nsh_core::{geometry::REM_SIZE, prelude::*};

use crate::state::AppState;
use crate::ui::tokens::{Token, color};

/// The name of the node a table becomes in an artifact's document.
pub(super) const TABLE: &str = "corral-table";
/// The attribute holding the table's place among the lifted tables.
const AT: &str = "at";
/// A cell's padding on each side, and the room it takes of a column, in rem.
const PAD_X: f32 = 12.0;
const PADDING: f32 = 2.0 * PAD_X / 16.0;
/// A column whose widest line is at most this many ems keeps it on one line.
const SHORT: f32 = 14.0;
/// The narrowest a long column's word is kept whole for, and the most it asks of the width.
const WORD_LEAST: f32 = 4.0;
const WORD_MOST: f32 = 10.0;
/// The most share of the spare width one column asks for, in ems of its widest line.
const SHARE_MOST: f32 = 60.0;

/// The tables of `document` (in its blocks, quotes, lists, disclosures and footnotes) lifted
/// out, each in its place a [`TABLE`] node naming its index in what is returned.
pub(super) fn lift(document: &mut Document) -> Rc<[Table]> {
    let mut tables = Vec::new();
    lift_blocks(&mut document.blocks, &mut tables);
    for note in &mut document.footnotes {
        lift_blocks(&mut note.children, &mut tables);
    }
    tables.into()
}

fn lift_blocks(blocks: &mut [Block], tables: &mut Vec<Table>) {
    for block in blocks {
        match block {
            Block::Table(table) => {
                let at = tables.len();
                tables.push(std::mem::take(table));
                *block = Block::Custom(CustomNode::new(TABLE).with_attribute(AT, at.to_string()));
            }
            Block::Blockquote { children, .. } | Block::Details { children, .. } => {
                lift_blocks(children, tables);
            }
            Block::List { items, .. } => {
                for item in items {
                    lift_blocks(&mut item.children, tables);
                }
            }
            _ => {}
        }
    }
}

/// The table `node` names among `tables`: a grid as wide as the page, its columns sized by
/// [`tracks`], the header on a quiet fill, every cell ruled; `cell` draws a cell's words.
pub(super) fn table(
    tables: &[Table],
    node: &CustomNode,
    cell: impl Fn(Document) -> AnyElement,
) -> Option<AnyElement> {
    let table = tables.get(node.attributes.get(AT)?.parse::<usize>().ok()?)?;
    let columns = table.columns();
    if columns == 0 {
        return None;
    }
    let (tracks, least) = tracks(table);
    let mut grid = Vec::with_capacity((columns + 1) * (table.rows.len() + 1));
    for (r, cells) in std::iter::once(&table.head).chain(&table.rows).enumerate() {
        let head = r == 0;
        let line = i16::try_from(r + 1).unwrap_or(i16::MAX);
        // Behind its cells, so the header's fill spans the row.
        grid.push(
            view()
                .row_start(line)
                .col_span_full()
                .when(head, |row| row.bg(color(Token::Surface2)))
                .into_element(),
        );
        let mut col = 0;
        let mut placed = Vec::with_capacity(columns);
        for each in cells {
            if col >= columns {
                break;
            }
            let span = each.span().max(1).min(columns - col);
            placed.push((col, span, each.content.clone()));
            col += span;
        }
        placed.extend((col..columns).map(|c| (c, 1, Vec::new())));
        for (c, span, content) in placed {
            let content = if head && !content.is_empty() {
                vec![Inline::Strong { content }]
            } else {
                content
            };
            let words = cell(Document::new(vec![Block::paragraph(content)]));
            let place = view().flex_col().min_w(px(0.0));
            let place = match table.align.get(c).copied().unwrap_or_default() {
                ColumnAlign::Center => place.items_center(),
                ColumnAlign::Right => place.items_end(),
                ColumnAlign::Left | ColumnAlign::None => place,
            };
            grid.push(
                place
                    .row_start(line)
                    .col_start(i16::try_from(c + 1).unwrap_or(i16::MAX))
                    .col_end(i16::try_from(c + 1 + span).unwrap_or(i16::MAX))
                    .py(px(8.0))
                    .px(px(PAD_X))
                    .border_r_1()
                    .border_b_1()
                    .border_color(color(Token::Rule))
                    .child(words)
                    .into_element(),
            );
        }
    }
    let grid = view()
        .grid()
        .grid_template_columns(SharedString::from(tracks))
        .w_full()
        .min_w(px((least * REM_SIZE.0).ceil() + 1.0))
        .border_t_1()
        .border_l_1()
        .border_color(color(Token::Rule))
        .children(grid);
    Some(scroll().horizontal().w_full().child(grid).into_element())
}

/// The grid's column tracks and the sum of their least widths in rem. A column whose widest
/// line is short is never narrower than that line, so it does not wrap; a longer one may narrow
/// to its longest word (within [`WORD_LEAST`] to [`WORD_MOST`]). Past their least, the columns
/// share the width by the length of their widest lines, so prose columns take most of it.
fn tracks(table: &Table) -> (String, f32) {
    let columns = table.columns();
    let mut line = vec![0.0f32; columns];
    let mut word = vec![0.0f32; columns];
    for row in std::iter::once(&table.head).chain(&table.rows) {
        let mut col = 0;
        for cell in row {
            if col >= columns {
                break;
            }
            // A spanning cell's width is shared by its columns; it sets none of them alone.
            if cell.span() <= 1 {
                let text: String = cell.content.iter().map(Inline::plain_text).collect();
                for part in text.lines() {
                    line[col] = line[col].max(ems(part));
                    for each in part.split_whitespace() {
                        word[col] = word[col].max(ems(each));
                    }
                }
            }
            col += cell.span().max(1);
        }
    }
    let mut least_sum = 0.0;
    let tracks = line
        .iter()
        .zip(&word)
        .map(|(&line, &word)| {
            let least = if line <= SHORT {
                line
            } else {
                word.clamp(WORD_LEAST, WORD_MOST)
            } + PADDING;
            least_sum += least;
            let share = line.clamp(1.0, SHARE_MOST);
            format!("minmax({least:.2}rem, {share:.2}fr)")
        })
        .collect::<Vec<_>>()
        .join(" ");
    (tracks, least_sum)
}

/// The ems of `text` as NSH estimates them: wide characters an em, spaces 0.3, the rest 0.6.
fn ems(text: &str) -> f32 {
    text.chars()
        .map(|c| match c as u32 {
            0x20 => 0.3,
            0x1100..=0x115f
            | 0x2e80..=0xa4cf
            | 0xac00..=0xd7a3
            | 0xf900..=0xfaff
            | 0xfe30..=0xfe4f
            | 0xff00..=0xff60
            | 0xffe0..=0xffe6
            | 0x1f300..=0x1faff
            | 0x20000..=0x3fffd => 1.0,
            _ => 0.6,
        })
        .sum()
}

/// A cell's words as an artifact's text view draws them.
pub(super) fn cell(app: AppState, document: Document) -> AnyElement {
    super::shown(app, text_view().document(document), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nsh_base::text_view::Parser;

    /// A report's table of a short label column and two prose columns must fit a narrow page:
    /// NSH's own floors (half the widest line, up to 30 rem) asked more than 60 rem of it.
    #[test]
    fn prose_columns_narrow_to_their_words_and_a_short_column_stays_whole() {
        let long = "a material feasible check should be made now if blocked report what cannot be verified";
        let document = Parser::new().markdown(&format!(
            "| Item | Result | Evidence |\n| --- | --- | --- |\n| 1. Routing | {long} | {long} |\n"
        ));
        let Some(Block::Table(table)) = document.blocks.first() else {
            panic!("a table");
        };
        let (tracks, least) = tracks(table);
        assert!(least < 30.0, "{least}: {tracks}");
        let first = tracks.split(" minmax").next().unwrap_or_default();
        let one_line = ems("1. Routing") + PADDING;
        assert!(
            first.starts_with(&format!("minmax({one_line:.2}rem")),
            "{tracks}"
        );
    }

    #[test]
    fn every_table_is_lifted_and_named_in_order_even_inside_a_list() {
        let mut document = Parser::new()
            .markdown("| a | b |\n| - | - |\n| 1 | 2 |\n\n- item\n\n  | c |\n  | - |\n  | 3 |\n");
        let tables = lift(&mut document);
        assert_eq!(tables.len(), 2);
        let named = |block: &Block| match block {
            Block::Custom(node) if node.name == TABLE => node.attributes.get(AT).cloned(),
            _ => None,
        };
        assert_eq!(named(&document.blocks[0]).as_deref(), Some("0"));
        let Some(Block::List { items, .. }) = document.blocks.get(1) else {
            panic!("a list after the table: {:?}", document.blocks);
        };
        let nested = items[0].children.iter().find_map(named);
        assert_eq!(nested.as_deref(), Some("1"));
    }
}
