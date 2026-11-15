//! A QR tile: dark modules on a pale tile in both appearances, as a code needs to scan. Screens
//! paint each row's runs of dark modules as one rectangle; a terminal draws two module rows per
//! text row with half blocks, dark on the pale tile colour.

use nsh_core::element::color as for_appearance;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::state::AppState;
use crate::ui::tokens::{Token, color};

/// The tile's padding around the modules on a screen.
const PAD: f32 = 12.0;
/// The tile's side on a desktop or in a terminal (`.qr`: 210 pt).
pub(super) const SIDE: f32 = 210.0;

/// `rows` (dark modules true, any quiet zone included) in a tile `side` points wide, read to
/// assistive technology as `label`.
pub fn tile(app: AppState, rows: Vec<Vec<bool>>, side: f32, label: String) -> AnyElement {
    if app.form == Form::Terminal {
        return terminal(rows, label);
    }
    let count = rows.len().max(1) as f32;
    let inner = side - 2.0 * PAD;
    let module = inner / count;
    // Runs of dark modules in a row as one rectangle each, on whole points, so no seams show.
    let drawing = canvas(move || {
        let mut painter = Painter::new().fill_style(for_appearance(color(Token::Shell)));
        for (y, row) in rows.iter().enumerate() {
            let top = (y as f32 * module).round();
            let bottom = ((y + 1) as f32 * module).round();
            let mut x = 0;
            while x < row.len() {
                if !row[x] {
                    x += 1;
                    continue;
                }
                let start = x;
                while x < row.len() && row[x] {
                    x += 1;
                }
                let left = (start as f32 * module).round();
                let right = (x as f32 * module).round();
                painter = painter.fill_rect(px(left), px(top), px(right - left), px(bottom - top));
            }
        }
        painter
    })
    .size(px(inner));
    view()
        .size(px(side))
        .flex_shrink_0()
        .p(px(PAD))
        .rounded(px(8.0))
        .bg(color(Token::ShellInk))
        .role(AccessibilityRole::Image)
        .label(label)
        .child(drawing)
        .into_element()
}

/// Two module rows per text row: `▀` top, `▄` bottom, `█` both, in the tile's dark colour on
/// its pale one.
fn terminal(rows: Vec<Vec<bool>>, label: String) -> AnyElement {
    let lines = rows
        .chunks(2)
        .map(|pair| {
            let top = &pair[0];
            let bottom = pair.get(1);
            let line: String = top
                .iter()
                .enumerate()
                .map(|(x, &up)| {
                    let down = bottom.is_some_and(|row| row.get(x).copied().unwrap_or(false));
                    match (up, down) {
                        (true, true) => '█',
                        (true, false) => '▀',
                        (false, true) => '▄',
                        (false, false) => ' ',
                    }
                })
                .collect();
            view()
                .bg(color(Token::ShellInk))
                .child(text(line).text_color(color(Token::Shell)))
                .into_element()
        })
        .collect::<Vec<_>>();
    view()
        .flex_shrink_0()
        .self_start()
        .role(AccessibilityRole::Image)
        .label(label)
        .children(lines)
        .into_element()
}
