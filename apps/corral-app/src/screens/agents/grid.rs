//! A pane's screen with its colours and styles (`model::agents::screen`).
//!
//! On the dark surface (desktop, phone) it is a strict grid of Cascadia Mono cells: every band
//! of one background is a box exactly its cells wide and one row tall, so fills join across runs
//! and rows and a frame's right side stays in its column; a character the face lacks, or a wide
//! one, is centred in a box of its own cells, so the fallback font's advance moves nothing after
//! it. Only bands and those characters take boxes; the runs inside a band are one text.
//!
//! In a real terminal (`Form::Terminal`) the host terminal keeps the grid, and NSH draws the
//! runs' colours in its theme.

use nsh_core::prelude::*;

use crate::model::agents::screen::{Band, DIM_KEEPS, Defaults, Ink, Look, Piece, Rgb, Row, Run};
use crate::ui::term;
use crate::ui::tokens::{FONT_SCREEN, Token, color};

use super::console::Set;

/// Cascadia Mono's advance in em: every character it has takes one cell of this width.
const ADVANCE: f32 = 1200.0 / 2048.0;

/// A screen colour as an app colour, the same in both appearances.
fn paint([red, green, blue]: Rgb) -> AdaptiveColor {
    let hex = u32::from_be_bytes([0, red, green, blue]);
    AdaptiveColor::rgb(hex, hex)
}

fn rgb_of(color: Hsla) -> Rgb {
    let [red, green, blue, _] = u32::from(Rgba::from(color)).to_be_bytes();
    [red, green, blue]
}

/// The terminal's default colours on the dark surface: the surface's own ink and fill, as the
/// plain screen (`console::screen_rows`) draws, so the view stays one with the app around it.
fn defaults() -> Defaults {
    Defaults {
        fore: rgb_of(color(Token::ShellInk).dark),
        back: rgb_of(color(Token::Shell).dark),
    }
}

/// A run's face and lines; its colour is the caller's.
fn marked(run: Text, look: Look) -> Text {
    run.when(look.bold, |t| t.font_weight(FontWeight::BOLD))
        .when(look.italic, |t| t.italic())
        .when(look.underline, |t| t.underline())
        .when(look.strike, |t| t.line_through())
}

/// A pane's styled screen on the dark surface: its rows at full width in a sideways scroll, as
/// `console::screen_rows`, with the insets inside the scroll so its bar never covers a row.
pub fn screen(rows: &[Row], set: Set, (sides, ends): (f32, f32)) -> AnyElement {
    let colours = defaults();
    let cell = set.size * ADVANCE;
    let rows = rows.iter().map(|row| {
        view()
            .flex_row()
            .flex_shrink_0()
            .h(px(set.leading))
            .children(row.iter().map(|band| band_box(band, set, cell, colours)))
            .into_element()
    });
    scroll()
        .horizontal()
        .child(
            view()
                .flex_shrink_0()
                .px(px(sides))
                .py(px(ends))
                .children(rows),
        )
        .into_element()
}

/// One band: its fill over exactly its cells. A band of one piece is that piece's box; one of
/// several pins each piece to its cells, so a fallback glyph's advance stays inside its box.
fn band_box(band: &Band, set: Set, cell: f32, colours: Defaults) -> AnyElement {
    let cells = |count: usize| px(count as f32 * cell);
    let fill = view()
        .flex_row()
        .flex_shrink_0()
        .w(cells(band.cells))
        .h(px(set.leading))
        .when_some(colours.fill(band.back), |fill, rgb| fill.bg(paint(rgb)));
    match band.pieces.as_slice() {
        [piece] => fill
            .when(piece.boxed, |fill| fill.justify_center())
            .child(piece_text(piece, band.back, set, colours))
            .into_element(),
        pieces => fill
            .children(pieces.iter().map(|piece| {
                view()
                    .flex_row()
                    .flex_shrink_0()
                    .w(cells(piece.cells))
                    .when(piece.boxed, |pinned| pinned.justify_center())
                    .child(piece_text(piece, band.back, set, colours))
                    .into_element()
            }))
            .into_element(),
    }
}

/// A piece's runs as one line of the terminal face at its own width; the box around it keeps the
/// grid, so the text is never clipped or wrapped to fit.
fn piece_text(piece: &Piece, back: Ink, set: Set, colours: Defaults) -> Text {
    text("")
        .flex_shrink_0()
        .number_of_lines(1u32)
        .font_family(FONT_SCREEN)
        .text_size(px(set.size))
        .line_height(px(set.leading))
        .children(piece.runs.iter().map(|run| {
            let ink = paint(colours.text(run.look, back));
            marked(text(run.text.clone()).text_color(ink), run.look)
        }))
}

/// A pane's styled screen in a real terminal: one row each, at full width in a sideways scroll,
/// as `console::cell_screen`.
pub fn cell_screen(rows: &[Row]) -> AnyElement {
    let rows = rows.iter().map(|row| {
        if row.is_empty() {
            return term::blank();
        }
        text("")
            .flex_shrink_0()
            .number_of_lines(1u32)
            .text_color(color(Token::Ink))
            .children(row.iter().flat_map(|band| {
                band.pieces
                    .iter()
                    .flat_map(|piece| &piece.runs)
                    .map(move |run| cell_run(run, band.back))
            }))
            .into_element()
    });
    scroll()
        .horizontal()
        .child(view().flex_shrink_0().px(term::cells(1.0)).children(rows))
        .into_element()
}

/// A screen colour in a real terminal: the defaults are the terminal's own foreground and page.
fn cell_ink(ink: Ink) -> AdaptiveColor {
    match ink {
        Ink::Fore => color(Token::Ink),
        Ink::Back => color(Token::Bg),
        Ink::Rgb(rgb) => paint(rgb),
    }
}

/// A run in a real terminal; a dim run keeps `DIM_KEEPS` of its colour over the terminal's
/// ground, and a set background is a band of the run's own.
fn cell_run(run: &Run, back: Ink) -> Text {
    let ink = cell_ink(run.look.ink);
    marked(
        text(run.text.clone()).text_color(if run.look.dim {
            ink.opacity(DIM_KEEPS)
        } else {
            ink
        }),
        run.look,
    )
    .when(back != Ink::Back, |t| t.bg(cell_ink(back)))
}
