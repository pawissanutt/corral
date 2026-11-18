//! The terminal's own pieces: lines that join the frames they meet, framed regions with a title
//! set into their line, and key caps. The top bar is `shell::terminal_bar`.
//!
//! NSH draws a terminal in the terminal's own colours: the page is always the terminal's
//! background, fills are bands only where the terminal reports its background, and neutral
//! colours fade when they sit near their ground. Structure here therefore comes from box-drawing
//! lines first, which every terminal draws, then bold, then bands. A row of `─` alone or a
//! column of `│` alone in a filled box is a divider to NSH: where its end meets another line it
//! is joined (`├ ┤ ┬ ┴ ┼`), so the frames and the panes inside them read as one ruled grid.
//!
//! Spacing in a terminal collapses (a gap of 16 points is one Cell, padding never more than
//! two), so exact runs of Cells are fixed-width spaces, not gaps.

use nsh_core::EllipsizeMode;
use nsh_core::element::cell_size;
use nsh_core::prelude::*;

use crate::model::Overlay;
use crate::state::AppState;
use crate::ui::tokens::{Token, color};

/// One Cell at the default Cell size, where the surface reports none yet.
const COLUMN: f32 = 8.0;
const ROW: f32 = 16.0;
/// A grid at least this wide gives the sidebar its full width and docks Ask beside the page. The
/// sidebar folds while Ask docks, so the page keeps half of the rest: 49 Cells at 110.
const WIDE_COLUMNS: f32 = 110.0;
/// A grid this wide or narrower folds an automatic sidebar to its rail: beside the labels, a list
/// would have to leave its detail or crush it (80 columns gave the labels 24 of 80 Cells).
const FOLD_COLUMNS: f32 = 100.0;
/// The folded sidebar: its lines, a selection bar, the count, the key.
const RAIL_COLUMNS: f32 = 10.0;
/// The unfolded sidebar: on a wide grid room for "This device <name>" in its bottom line and the
/// autopilot state on one row; otherwise the longest Settings section on its indent.
const SIDEBAR_WIDE_COLUMNS: f32 = 32.0;
const SIDEBAR_COLUMNS: f32 = 24.0;
/// The content frame's two lines.
const FRAME_COLUMNS: f32 = 2.0;
/// The least a detail keeps beside its list; on a narrower page the list stacks above it. Under
/// this a review's numbered diff lines keep under 30 Cells of code; so on the classic 80-column
/// grid (68 Cells of page beside the rail) every list-and-detail page stacks, while at 120 they
/// all sit beside their details.
const DETAIL_COLUMNS: f32 = 44.0;
/// On a wide page a list grows to this share of it, up to `LIST_MAX_COLUMNS`, so its names stop
/// being cut while the detail has more room than it can use.
const LIST_SHARE: f32 = 0.4;
const LIST_MAX_COLUMNS: f32 = 80.0;
/// Longer than any line a terminal shows; the view a line sits in clips it.
const LINE_CELLS: usize = 400;
/// The glyphs `─`, `│` and `░` in UTF-8.
const ACROSS_GLYPH: [u8; 3] = [0xE2, 0x94, 0x80];
const DOWN_GLYPH: [u8; 3] = [0xE2, 0x94, 0x82];
const SHADE_GLYPH: [u8; 3] = [0xE2, 0x96, 0x91];

/// `glyph` (three bytes of UTF-8) over and over.
const fn repeated<const N: usize>(glyph: [u8; 3]) -> [u8; N] {
    let mut bytes = [0; N];
    let mut at = 0;
    while at < N {
        bytes[at] = glyph[at % 3];
        at += 1;
    }
    bytes
}

const fn utf8(bytes: &'static [u8]) -> &'static str {
    match std::str::from_utf8(bytes) {
        Ok(line) => line,
        Err(_) => "",
    }
}

const ACROSS_BYTES: [u8; 3 * LINE_CELLS] = repeated(ACROSS_GLYPH);
const SHADE_BYTES: [u8; 3 * LINE_CELLS] = repeated(SHADE_GLYPH);
/// `│` on each row, one row apart: every row of the text one glyph, as NSH reads a divider.
const DOWN_BYTES: [u8; 4 * LINE_CELLS - 1] = {
    let mut bytes = [b'\n'; 4 * LINE_CELLS - 1];
    let mut at = 0;
    while at < bytes.len() {
        if at % 4 < 3 {
            bytes[at] = DOWN_GLYPH[at % 4];
        }
        at += 1;
    }
    bytes
};
const ACROSS: &str = utf8(&ACROSS_BYTES);
const SHADE: &str = utf8(&SHADE_BYTES);
const DOWN: &str = utf8(&DOWN_BYTES);

/// The width of `n` Cells, in points.
pub fn cells(n: f32) -> Points {
    px(n * cell_size().map_or(COLUMN, |cell| cell.width.0))
}

/// The height of `n` rows, in points.
pub fn rows(n: f32) -> Points {
    px(n * cell_size().map_or(ROW, |cell| cell.height.0))
}

/// The grid's width in whole Cells (tracked: follows the terminal's size).
pub fn columns() -> usize {
    let column = cell_size().map_or(COLUMN, |cell| cell.width.0);
    (window().width.0 / column + 0.01).floor().max(0.0) as usize
}

/// The grid's height in whole rows.
pub fn grid_rows() -> usize {
    let row = cell_size().map_or(ROW, |cell| cell.height.0);
    (window().height.0 / row + 0.01).floor().max(0.0) as usize
}

/// Whether the grid is wide enough for the full sidebar and Ask docked beside the page (tracked).
pub fn wide() -> bool {
    is_wide(columns())
}

/// Whether the grid is narrow enough that the sidebar folds to its rail (tracked).
pub fn narrow() -> bool {
    folds(columns())
}

fn is_wide(columns: usize) -> bool {
    columns as f32 >= WIDE_COLUMNS
}

fn folds(columns: usize) -> bool {
    columns as f32 <= FOLD_COLUMNS
}

/// Whether Ask is open docked beside the page (tracked); on a grid short of [`wide`] it takes the
/// whole content frame and the page steps out while it is open.
pub fn ask_docked(app: AppState) -> bool {
    app.is_open(Overlay::Ask) && wide()
}

/// Whether the automatic sidebar folds to its rail (tracked): on a narrow grid, and while Ask
/// docks, which takes half the content; with the labels' width gone too the page beside Ask
/// would be left a sliver.
pub fn sidebar_folds(app: AppState) -> bool {
    narrow() || ask_docked(app)
}

/// Whether this app's terminal sidebar shows as the rail of keys and counts (tracked): always
/// while it folds, unless its labels were asked for there for the moment (a terminal cannot lay
/// them over the page without leaving the page's cut words beside them).
pub fn sidebar_collapsed(app: AppState) -> bool {
    let folds = sidebar_folds(app);
    app.sidebar.get().collapsed(folds) && !(folds && app.sidebar_peeking())
}

/// The sidebar's width in Cells, its lines included (tracked).
pub fn sidebar_columns(app: AppState) -> f32 {
    sidebar_width(sidebar_collapsed(app), wide())
}

fn sidebar_width(collapsed: bool, wide: bool) -> f32 {
    if collapsed {
        RAIL_COLUMNS
    } else if wide {
        SIDEBAR_WIDE_COLUMNS
    } else {
        SIDEBAR_COLUMNS
    }
}

/// The page's width in Cells (tracked): the grid less the sidebar and the content frame's lines,
/// and only half of that while Ask docks beside it.
pub fn page_columns(app: AppState) -> f32 {
    page_room(columns(), sidebar_columns(app), ask_docked(app))
}

/// Whether a list of `list` Cells (its natural width) sits beside its detail on this page
/// (tracked): only while the detail keeps its least width after it and the line between them.
/// Otherwise the list stacks above the detail; it is never left out.
pub fn beside(app: AppState, list: f32) -> bool {
    holds_beside(page_columns(app), list)
}

/// The width in Cells of a list beside its detail (tracked): its natural `list` width, growing
/// with a wide page.
pub fn list_columns(app: AppState, list: f32) -> f32 {
    list_room(page_columns(app), list)
}

fn page_room(columns: usize, sidebar: f32, docked: bool) -> f32 {
    let content = (columns as f32 - sidebar - FRAME_COLUMNS).max(0.0);
    // Docked Ask takes half the row, its own line included.
    if docked {
        (content / 2.0).floor()
    } else {
        content
    }
}

fn holds_beside(page: f32, list: f32) -> bool {
    page >= list + 1.0 + DETAIL_COLUMNS
}

fn list_room(page: f32, list: f32) -> f32 {
    (page * LIST_SHARE)
        .round()
        .min(LIST_MAX_COLUMNS)
        .min(page - 1.0 - DETAIL_COLUMNS)
        .max(list)
}

/// `n` blank Cells in a row.
pub fn space(n: f32) -> AnyElement {
    view()
        .w(cells(n))
        .h(rows(1.0))
        .flex_shrink_0()
        .into_element()
}

/// One blank row. Inside a frame NSH gives gaps and margins no rows, and a box around one text
/// none of its padding, so a row of space is a box of that height.
pub fn blank() -> AnyElement {
    view().h(rows(1.0)).flex_shrink_0().into_element()
}

/// A divider across its parent: `─` from edge to edge, joined to the lines its ends touch.
pub fn hrule() -> AnyElement {
    glyph_row(ACROSS)
}

/// A placeholder `rows_high` rows deep, drawn in `░` to the width its caller sets: where the
/// desktop pulses a bar, a terminal that draws no bands would otherwise show nothing loading.
pub fn shade(rows_high: usize) -> View {
    view()
        .flex_shrink_0()
        .min_w(px(0.0))
        .accessibility_elements_hidden(true)
        .children((0..rows_high.max(1)).map(|_| glyph_row(SHADE)))
}

/// One row of `glyphs` across its parent, clipped at its edge.
fn glyph_row(glyphs: &'static str) -> AnyElement {
    view()
        .h(rows(1.0))
        .flex_shrink_0()
        .min_w(px(0.0))
        .overflow_hidden()
        .accessibility_elements_hidden(true)
        .child(
            text(glyphs)
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .right(px(0.0))
                .number_of_lines(1u32)
                .text_overflow(EllipsizeMode::Clip)
                .pointer_events(PointerEvents::None)
                .text_color(color(Token::Rule)),
        )
        .into_element()
}

/// A divider down its parent: `│` on every row, joined to the lines its ends touch. NSH reads a
/// column of `│` as a divider only inside a filled box; the fill itself is never drawn.
pub fn vrule() -> AnyElement {
    view()
        .w(cells(1.0))
        .flex_shrink_0()
        .self_stretch()
        .overflow_hidden()
        .bg(color(Token::Rule))
        .accessibility_elements_hidden(true)
        .child(
            text(DOWN)
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .bottom(px(0.0))
                .text_overflow(EllipsizeMode::Clip)
                .pointer_events(PointerEvents::None)
                .text_color(color(Token::Rule)),
        )
        .into_element()
}

/// A framed region: `body` inside a rounded line. The returned view holds the frame; add
/// [`top_title`] or [`bottom_title`] children to set a title into its line.
pub fn framed(body: impl IntoElement) -> View {
    view()
        .relative()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(
            view()
                .flex_1()
                .flex_col()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .border_1()
                .rounded(px(6.0))
                .border_color(color(Token::Rule2))
                .child(body),
        )
}

/// A title in the top line of the [`framed`] region it is a child of: `╭─ title ──`.
pub fn top_title(content: impl IntoElement) -> AnyElement {
    title_slot(content).top(px(0.0)).into_element()
}

/// A title in the bottom line of the [`framed`] region it is a child of: `╰─ title ──`.
pub fn bottom_title(content: impl IntoElement) -> AnyElement {
    title_slot(content).bottom(px(0.0)).into_element()
}

/// Two Cells in from the corner, a blank Cell each side. Its opaque page-coloured fill clears
/// the line under it and is no band of its own.
fn title_slot(content: impl IntoElement) -> View {
    view()
        .absolute()
        .left(cells(2.0))
        .h(rows(1.0))
        .flex_row()
        .items_center()
        .px(cells(1.0))
        .bg(color(Token::Bg))
        .child(content)
}

/// The focus mark (`›`), led by a kit control's Cell while it has the keyboard focus. Without
/// colours NSH draws a focused control and a selected or filled one alike, reversed (the focus
/// only adds an underline); the mark tells them apart at every colour depth.
pub const FOCUS_MARK: &str = "›";

/// Tracked: whether the control `handle` is attached to has the keyboard focus.
pub fn focus_of(handle: ElementRef) -> Signal<bool> {
    let focused = signal(false);
    handle.on_focus_in(move || focused.set(true));
    handle.on_focus_out(move || focused.set(false));
    focused
}

/// One Cell holding [`FOCUS_MARK`] while `focused`, blank otherwise: the leading Cell of a
/// control's band, so marking it moves nothing.
pub fn focus_cell(focused: Signal<bool>) -> AnyElement {
    view()
        .w(cells(1.0))
        .flex_shrink_0()
        .accessibility_elements_hidden(true)
        .child(
            text(move || if focused.get() { FOCUS_MARK } else { " " })
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink)),
        )
        .into_element()
}

/// A key as a cap: the key in bold on a small band (the standard grey on a 16-colour terminal,
/// reversed without colours), so the legend reads as keys and names.
pub fn key_cap(keys: impl Into<SharedString>) -> AnyElement {
    view()
        .flex_row()
        .flex_shrink_0()
        .h(rows(1.0))
        .px(cells(1.0))
        // Lighter than the page in both appearances, so a raised step in both.
        .bg(color(Token::Surface))
        .accessibility_elements_hidden(true)
        .child(
            // The key is what the cap is for: a short line drops names, never a key's letters.
            text(keys.into())
                .flex_shrink_0()
                .number_of_lines(1u32)
                .font_weight(FontWeight::BOLD)
                .text_color(color(Token::Ink)),
        )
        .into_element()
}

/// A key's cap and, a Cell after it, what the key does: one entry of a legend.
pub fn legend(keys: impl Into<SharedString>, name: &'static str) -> AnyElement {
    view()
        .flex_row()
        .flex_shrink_0()
        .child(key_cap(keys))
        .child(space(1.0))
        .child(
            text(name)
                .number_of_lines(1u32)
                .text_color(color(Token::Ink)),
        )
        .into_element()
}

/// Muted text: secondary, as NSH fades it.
pub fn quiet(content: impl Into<SharedString>) -> Text {
    text(content.into())
        .flex_shrink_0()
        .number_of_lines(1u32)
        .text_color(color(Token::Ink3))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The least the page keeps beside docked Ask: a list or a detail still reads at this width.
    const ASK_PAGE_COLUMNS: f32 = 48.0;
    /// The natural list widths of the terminal's list-and-detail pages (Review, Plans, Inbox,
    /// Now), in Cells.
    const LISTS: [f32; 4] = [25.0, 32.0, 33.0, 44.0];
    /// The lists that stack below `beside`'s width (Review, Plans, Inbox); Now lays out its own
    /// panes.
    const STACKING_LISTS: [f32; 3] = [25.0, 32.0, 33.0];

    #[test]
    fn sidebar_folds_on_an_eighty_column_grid() {
        assert!(folds(80));
        assert!(folds(100));
        assert!(!folds(WIDE_COLUMNS as usize));
    }

    /// TT-13: at 80 columns a list used to be left out, its detail alone across the page, so
    /// `j`/`k` moved an unseen selection. Every list now stacks there, and sits beside its
    /// detail on the 120-column grid with the full sidebar.
    #[test]
    fn lists_stack_on_eighty_columns_and_sit_beside_on_one_hundred_twenty() {
        let narrow = page_room(80, sidebar_width(true, is_wide(80)), false);
        let wide = page_room(120, sidebar_width(false, is_wide(120)), false);
        for list in STACKING_LISTS {
            assert!(!holds_beside(narrow, list), "list {list} beside at 80");
            assert!(holds_beside(wide, list), "list {list} stacked at 120");
        }
    }

    #[test]
    fn docked_ask_never_squeezes_a_detail_beside_its_list() {
        for columns in 60..=240 {
            if is_wide(columns) {
                // While Ask docks the sidebar folds to its rail.
                let page = page_room(columns, sidebar_width(true, true), true);
                assert!(page >= ASK_PAGE_COLUMNS, "{columns} columns: page {page}");
            }
            for (sidebar, docked) in [
                (RAIL_COLUMNS, false),
                (RAIL_COLUMNS, true),
                (SIDEBAR_COLUMNS, false),
                (SIDEBAR_WIDE_COLUMNS, false),
                (SIDEBAR_WIDE_COLUMNS, true),
            ] {
                let page = page_room(columns, sidebar, docked);
                for list in LISTS {
                    if holds_beside(page, list) {
                        let width = list_room(page, list);
                        assert!(width >= list, "{columns} columns: list {width} < {list}");
                        assert!(
                            page - width - 1.0 >= DETAIL_COLUMNS,
                            "{columns} columns, sidebar {sidebar}, docked {docked}: detail {}",
                            page - width - 1.0
                        );
                    }
                }
            }
        }
    }
}
