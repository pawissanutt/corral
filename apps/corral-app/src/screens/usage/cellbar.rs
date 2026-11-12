//! Terminal bars drawn in Cells: the provider window gauge with its 70 and 90 percent zones, the
//! profile share meter with its marker, and the stacked daily cost bar. Each Cell's glyph says
//! what it is without colour (filled or empty, below or inside a zone, which cost), and colour
//! repeats it where the terminal has colour. Neighbouring Cells of one look are one text.

use nsh_core::prelude::*;

use crate::model::usage::{Level, SHARE_MARKER_PERCENT, level};
use crate::ui::tokens::{Token, color};

/// One Cell's look.
type Look = (char, Token);

/// The window used below the first zone, inside the 70 to 90 zone, and from 90 up.
const USED_CLEAR: Look = ('█', Token::Ink);
const USED_WARN: Look = ('▓', Token::Held);
const USED_BAD: Look = ('▓', Token::Fail);
/// Room left below the first zone, in the 70 to 90 zone, and from 90 up.
const ROOM_CLEAR: Look = ('·', Token::Ink3);
const ROOM_WARN: Look = ('░', Token::Held);
const ROOM_BAD: Look = ('▒', Token::Fail);
/// The share meter's empty Cells and its marker.
const SHARE_EMPTY: Look = ('·', Token::Ink3);
const SHARE_MARKER: Look = ('┊', Token::Ink3);

/// The cost bar's segments in legend order: the OpenAI and Anthropic subscriptions and the
/// metered API (`var(--ink-2)`, `var(--ink-4)`, `var(--held)` in the mockup).
pub const COST_LOOKS: [Look; 3] = [('█', Token::Ink), ('▒', Token::Ink3), ('▓', Token::Held)];

/// The provider window gauge: `cells` wide, filled up to `used` (a fraction), each Cell in the
/// zone the model's [`level`] puts its middle in, so the 70 and 90 marks show filled or empty.
pub fn gauge(used: f32, cells: usize) -> AnyElement {
    let looks = (0..cells)
        .map(|at| {
            let middle = (at as f32 + 0.5) / cells as f32;
            match (middle < used, level(middle)) {
                (true, Level::None) => USED_CLEAR,
                (true, Level::Warn) => USED_WARN,
                (true, Level::Bad) => USED_BAD,
                (false, Level::None) => ROOM_CLEAR,
                (false, Level::Warn) => ROOM_WARN,
                (false, Level::Bad) => ROOM_BAD,
            }
        })
        .collect::<Vec<_>>();
    runs(&looks)
}

/// The profile share meter (`.meter`): `share` percent of `cells` filled in the provider's
/// level, the rest dotted, the model's marker Cell at [`SHARE_MARKER_PERCENT`].
pub fn meter(share: u32, provider_level: Level, cells: usize) -> AnyElement {
    let fill = match provider_level {
        Level::None => USED_CLEAR,
        Level::Warn => USED_WARN,
        Level::Bad => USED_BAD,
    };
    let filled = share_cells(share, cells);
    let marker = share_cells(SHARE_MARKER_PERCENT, cells).min(cells.saturating_sub(1));
    let looks = (0..cells)
        .map(|at| {
            if at < filled {
                fill
            } else if at == marker {
                SHARE_MARKER
            } else {
                SHARE_EMPTY
            }
        })
        .collect::<Vec<_>>();
    runs(&looks)
}

/// A day's stacked cost bar: each value is a percent of `cells`, as the mockup's segment
/// widths are percents of the track. Rounded on the running total, so the bar's length is the
/// total's and no segment drifts.
pub fn stacked(values: [u32; 3], cells: usize) -> AnyElement {
    let mut looks = Vec::with_capacity(cells);
    let mut total = 0;
    for (value, look) in values.into_iter().zip(COST_LOOKS) {
        let start = share_cells(total, cells);
        total += value;
        let end = share_cells(total, cells).min(cells);
        looks.extend(std::iter::repeat_n(look, end.saturating_sub(start)));
    }
    runs(&looks)
}

/// One segment's Cell for the cost legend.
pub fn swatch(look: Look) -> AnyElement {
    runs(&[look])
}

fn share_cells(percent: u32, cells: usize) -> usize {
    (percent as f32 * cells as f32 / 100.0).round() as usize
}

/// The Cells as texts, one per run of the same look.
fn runs(looks: &[Look]) -> AnyElement {
    let mut parts = Vec::new();
    let mut at = 0;
    while at < looks.len() {
        let look = looks[at];
        let length = looks[at..]
            .iter()
            .take_while(|&&other| other == look)
            .count();
        let (glyph, ink) = look;
        parts.push(
            text(std::iter::repeat_n(glyph, length).collect::<String>())
                .number_of_lines(1u32)
                .flex_shrink_0()
                .text_color(color(ink))
                .into_element(),
        );
        at += length;
    }
    view()
        .flex_row()
        .flex_shrink_0()
        .accessibility_elements_hidden(true)
        .children(parts)
        .into_element()
}
