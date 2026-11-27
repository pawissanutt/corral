//! Which of a chat message's tables are laid out as NSH's grid and which as records.
//!
//! The width that matters is the one the message really has (not the window's, not the form's):
//! the message's wrapper reports it after layout, and each table is judged against it. A table
//! is a grid only when NSH would draw it whole, with no sideways scroll; anything else, and
//! every table before the first layout, is records: one bullet per row, each field on its own
//! line behind its bold header, wrapping in whatever width there is, so no cell is clipped or
//! reachable only by scrolling (T-2056). Only the layout changes: no cell is edited, so
//! copying, links, emphasis and code in the cells read as the message wrote them. A word wider
//! than its grid column is broken by the text engines (Pango's `WordChar`, the web's
//! `break_split`), as it is in any paragraph.
//!
//! The decisions are a memo of one `bool` per table in document order ([`Measure::plan`]); the
//! document is built again only when one of them flips, never for each pixel of a resize, and
//! the static and the streaming message share this one policy.

use nsh_base::text_view::document::ListItem;
use nsh_base::text_view::{Block, Document, Inline, Table};
use nsh_core::{geometry::REM_SIZE, prelude::*};

use super::{record_blocks, record_row};
/// What a list item, a quote and a disclosure take of the message's width, in points.
const LIST_INSET: f32 = 24.0;
const QUOTE_INSET: f32 = 16.0;
const DETAILS_INSET: f32 = 16.0;

/// The width a message really has, as its wrapper last laid out (0 until it has).
#[derive(Clone, Copy)]
pub(super) struct Measure(Signal<f32>);

impl Measure {
    pub(super) fn new() -> Self {
        Self(signal(0.0f32))
    }

    /// `wrapper` as wide as its parent and reporting that width, to the half point.
    pub(super) fn track(self, wrapper: View) -> View {
        let width = self.0;
        wrapper.w_full().on_layout(move |frame: &Bounds<Points>| {
            let now = frame.size.width.0;
            if (width.get_untracked() - now).abs() >= 0.5 {
                width.set(now);
            }
        })
    }

    pub(super) fn plan(self, minimums: &[f32]) -> Vec<bool> {
        let width = self.0.get();
        minimums.iter().map(|minimum| width >= *minimum).collect()
    }
}

/// Minimum message width for each table, including space a surrounding list, quote or
/// disclosure consumes. Calculate once per document, not once per resize.
pub(super) fn minimums(document: &Document) -> Vec<f32> {
    fn collect(blocks: &[Block], inset: f32, widths: &mut Vec<f32>) {
        for block in blocks {
            match block {
                Block::Table(table) => widths.push(grid_width(table) + inset),
                Block::Blockquote { children, .. } => {
                    collect(children, inset + QUOTE_INSET, widths)
                }
                Block::Details { children, .. } => collect(children, inset + DETAILS_INSET, widths),
                Block::List { items, .. } => {
                    for item in items {
                        collect(&item.children, inset + LIST_INSET, widths);
                    }
                }
                _ => {}
            }
        }
    }
    let mut widths = Vec::new();
    collect(&document.blocks, 0.0, &mut widths);
    for note in &document.footnotes {
        // NSH places the numbered label beside the note in 13 pt type, with a 6 pt gap.
        let label = 6.0 + 13.0 * (note.index.to_string().len() + 1) as f32;
        collect(&note.children, label, &mut widths);
    }
    widths
}

/// `document`'s tables as `plan` says: the grid where it holds `true`, records otherwise.
pub(super) fn apply(document: &mut Document, plan: &[bool]) {
    let mut grid = plan.iter().copied();
    document.blocks = walk(std::mem::take(&mut document.blocks), &mut grid);
    for note in &mut document.footnotes {
        note.children = walk(std::mem::take(&mut note.children), &mut grid);
    }
}

fn walk(blocks: Vec<Block>, grid: &mut impl Iterator<Item = bool>) -> Vec<Block> {
    blocks
        .into_iter()
        .flat_map(|block| match block {
            Block::Table(table) => {
                if grid.next().unwrap_or(false) {
                    vec![Block::Table(table)]
                } else {
                    records(&table)
                }
            }
            Block::Blockquote { children, alert } => vec![Block::Blockquote {
                children: walk(children, grid),
                alert,
            }],
            Block::Details {
                summary,
                children,
                open,
            } => vec![Block::Details {
                summary,
                children: walk(children, grid),
                open,
            }],
            Block::List {
                ordered,
                start,
                loose,
                mut items,
            } => {
                for item in &mut items {
                    let children = std::mem::take(&mut item.children);
                    item.children = walk(children, grid);
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

/// `table` as records, grouped: a bullet for each row, holding the row's fields as the
/// shared records give them (the header in bold before the cell, each field on its own line).
fn records(table: &Table) -> Vec<Block> {
    if table.rows.is_empty() {
        return record_blocks(table);
    }
    let items = table
        .rows
        .iter()
        .map(|row| ListItem::new(record_row(&table.head, row)))
        .collect();
    vec![Block::List {
        ordered: false,
        start: Default::default(),
        loose: true,
        items,
    }]
}

/// NSH 0.6.1's column floors (`TextView::table_grid`): half the widest line, between 10 and
/// 30 rem, or the line plus padding when that is less. Below their sum the grid scrolls.
fn grid_width(table: &Table) -> f32 {
    if table.head.is_empty() {
        return f32::INFINITY;
    }
    let rows = || std::iter::once(&table.head).chain(&table.rows);
    // An image's alt text does not measure its attachment; a span has no single column.
    if rows()
        .flatten()
        .any(|cell| cell.span() != 1 || cell.content.iter().any(has_image))
    {
        return f32::INFINITY;
    }
    let columns = table.columns();
    let mut line = vec![0.0f32; columns];
    for row in rows() {
        for (at, cell) in row.iter().enumerate().take(columns) {
            let text: String = cell
                .content
                .iter()
                .map(|inline| inline.plain_text())
                .collect();
            for part in text.lines() {
                line[at] = line[at].max(ems(part));
            }
        }
    }
    // Include the left rule and room for rounding the tracks and the measured width.
    2.0 + line
        .iter()
        .map(|line| ((line + 2.0).min((line / 2.0).clamp(10.0, 30.0)) * REM_SIZE.0).ceil())
        .sum::<f32>()
}

fn has_image(inline: &Inline) -> bool {
    match inline {
        Inline::Image(_) => true,
        Inline::Strong { content }
        | Inline::Emphasis { content }
        | Inline::Strikethrough { content }
        | Inline::Underline { content }
        | Inline::Superscript { content }
        | Inline::Small { content }
        | Inline::Subscript { content }
        | Inline::Highlight { content, .. }
        | Inline::Link { content, .. } => content.iter().any(has_image),
        _ => false,
    }
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
