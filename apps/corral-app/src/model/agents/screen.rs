//! A pane's screen as styled rows, ready for a cell grid: the protocol's runs with their colours
//! resolved (the indexed palette here; the terminal's two default colours stay named, for the
//! client that draws them to choose), each row cut into bands of one background, and each band
//! into pieces. Cascadia Mono advances every character it has by one cell; a character it lacks
//! is set by a fallback font of another advance, and a wide one takes two cells, so each of
//! those is a piece of its own that the client pins to its cells.

use corral_protocol::detail::{TerminalColor, TerminalRun};
use unicode_width::UnicodeWidthChar;

/// Red, green and blue.
pub type Rgb = [u8; 3];

/// One row of the screen, left to right.
pub type Row = Vec<Band>;

/// A colour of the screen: the terminal's default foreground or background, which the client
/// owns, or a resolved colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ink {
    Fore,
    Back,
    Rgb(Rgb),
}

/// How a run's characters are drawn. `bold` is the bold face and keeps the colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Look {
    pub ink: Ink,
    pub dim: bool,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

/// Characters of one look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub look: Look,
}

/// Cells drawn as one text: characters of the terminal face, or (`boxed`) one character the face
/// lacks or that is two cells wide, which the client centres in its cells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub cells: usize,
    pub boxed: bool,
    pub runs: Vec<Run>,
}

/// Adjacent cells on one background, so its fill is one box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Band {
    pub back: Ink,
    pub cells: usize,
    pub pieces: Vec<Piece>,
}

/// The terminal's default colours, chosen by the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Defaults {
    pub fore: Rgb,
    pub back: Rgb,
}

/// The share of its colour a dim run keeps; the rest is its background's.
pub const DIM_KEEPS: f32 = 0.6;

/// Windows Terminal's default scheme, Campbell: indexed colours 0 to 15.
const CAMPBELL: [Rgb; 16] = [
    [0x0C, 0x0C, 0x0C],
    [0xC5, 0x0F, 0x1F],
    [0x13, 0xA1, 0x0E],
    [0xC1, 0x9C, 0x00],
    [0x00, 0x37, 0xDA],
    [0x88, 0x17, 0x98],
    [0x3A, 0x96, 0xDD],
    [0xCC, 0xCC, 0xCC],
    [0x76, 0x76, 0x76],
    [0xE7, 0x48, 0x56],
    [0x16, 0xC6, 0x0C],
    [0xF9, 0xF1, 0xA5],
    [0x3B, 0x78, 0xFF],
    [0xB4, 0x00, 0x9E],
    [0x61, 0xD6, 0xD6],
    [0xF2, 0xF2, 0xF2],
];

/// The xterm colour cube's levels (indexed colours 16 to 231).
const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// The code points every shipped Cascadia Mono face has (the intersection of the four faces'
/// character maps, from U+0020), as the first and last of each range, ascending.
const COVERED: &[u32] = &[
    0x0020, 0x007E, 0x00A0, 0x017F, 0x0186, 0x0186, 0x0189, 0x0189, 0x018E, 0x0192, 0x0194, 0x0194,
    0x0196, 0x0196, 0x019A, 0x019B, 0x019D, 0x019D, 0x01A0, 0x01A1, 0x01A9, 0x01A9, 0x01AF, 0x01B2,
    0x01B7, 0x01B7, 0x01CD, 0x01CE, 0x01DD, 0x01DD, 0x01E4, 0x01E7, 0x01EA, 0x01EB, 0x01F0, 0x01F0,
    0x01FA, 0x01FF, 0x0218, 0x021B, 0x021E, 0x021F, 0x0232, 0x0233, 0x0237, 0x0237, 0x023A, 0x023B,
    0x023D, 0x023E, 0x0245, 0x0245, 0x0251, 0x0251, 0x0254, 0x0254, 0x0256, 0x0256, 0x0259, 0x0259,
    0x025B, 0x025B, 0x0262, 0x0263, 0x0269, 0x026C, 0x0272, 0x0272, 0x0283, 0x0283, 0x028A, 0x028C,
    0x0292, 0x0292, 0x0294, 0x0295, 0x029F, 0x029F, 0x02A7, 0x02A7, 0x02B7, 0x02B8, 0x02BB, 0x02BC,
    0x02C0, 0x02C0, 0x02C6, 0x02C7, 0x02C9, 0x02C9, 0x02D0, 0x02D0, 0x02D8, 0x02DD, 0x0300, 0x0304,
    0x0306, 0x030C, 0x0312, 0x0313, 0x0315, 0x0315, 0x031B, 0x031B, 0x0323, 0x0323, 0x0325, 0x0328,
    0x0331, 0x0332, 0x0340, 0x0342, 0x0374, 0x0375, 0x037E, 0x037E, 0x0384, 0x038A, 0x038C, 0x038C,
    0x038E, 0x03A1, 0x03A3, 0x03CF, 0x03D7, 0x03D7, 0x0400, 0x045F, 0x0490, 0x0493, 0x0496, 0x0497,
    0x049A, 0x049B, 0x04A2, 0x04A3, 0x04AE, 0x04B3, 0x04B6, 0x04B7, 0x04BA, 0x04BB, 0x04C0, 0x04C0,
    0x04CF, 0x04CF, 0x04D8, 0x04D9, 0x04E2, 0x04E3, 0x04E8, 0x04E9, 0x04EE, 0x04EF, 0x0E3F, 0x0E3F,
    0x1D00, 0x1D00, 0x1DBB, 0x1DBB, 0x1DBF, 0x1DBF, 0x1E24, 0x1E25, 0x1E30, 0x1E30, 0x1E32, 0x1E37,
    0x1E3A, 0x1E3B, 0x1E48, 0x1E49, 0x1E50, 0x1E53, 0x1E5A, 0x1E5B, 0x1E62, 0x1E63, 0x1E6E, 0x1E6E,
    0x1E80, 0x1E85, 0x1E9E, 0x1E9E, 0x1EA0, 0x1EF9, 0x2008, 0x2008, 0x2010, 0x2011, 0x2013, 0x2015,
    0x2017, 0x201E, 0x2020, 0x2022, 0x2024, 0x2024, 0x2026, 0x2026, 0x2028, 0x2028, 0x2030, 0x2030,
    0x2032, 0x2033, 0x2039, 0x203A, 0x203C, 0x203C, 0x203E, 0x203E, 0x2044, 0x2044, 0x2070, 0x2070,
    0x2074, 0x2079, 0x207F, 0x2089, 0x20A0, 0x20A1, 0x20A3, 0x20A4, 0x20A6, 0x20AE, 0x20B1, 0x20B2,
    0x20B4, 0x20B5, 0x20B8, 0x20BA, 0x20BC, 0x20BF, 0x2105, 0x2105, 0x2113, 0x2113, 0x2116, 0x2117,
    0x211E, 0x211E, 0x2122, 0x2122, 0x2126, 0x2126, 0x212E, 0x212E, 0x215B, 0x215E, 0x2190, 0x2195,
    0x21A8, 0x21A8, 0x21B2, 0x21B2, 0x21E1, 0x21E1, 0x21E3, 0x21E3, 0x2202, 0x2202, 0x2206, 0x2206,
    0x220F, 0x220F, 0x2211, 0x2212, 0x2215, 0x2215, 0x2219, 0x221A, 0x221E, 0x221F, 0x2229, 0x2229,
    0x222B, 0x222B, 0x2236, 0x2236, 0x2248, 0x2248, 0x2260, 0x2265, 0x2302, 0x2302, 0x2310, 0x2310,
    0x2320, 0x2321, 0x23AF, 0x23AF, 0x23CE, 0x23CE, 0x2400, 0x2429, 0x2500, 0x25FF, 0x2619, 0x2619,
    0x263A, 0x263C, 0x2640, 0x2640, 0x2642, 0x2642, 0x2660, 0x2660, 0x2663, 0x2663, 0x2665, 0x2666,
    0x266A, 0x266B, 0x2713, 0x2713, 0x2767, 0x2767, 0x276C, 0x2771, 0x2800, 0x28FF, 0x2A2F, 0x2A2F,
    0x2B12, 0x2B2F, 0x2BC5, 0x2BC8, 0x2BCA, 0x2BCB, 0x2C62, 0x2C62, 0x2C6D, 0x2C6D, 0xFB01, 0xFB02,
    0xFFFD, 0xFFFD, 0x1CC21, 0x1CC2F, 0x1CD00, 0x1CDE5, 0x1CE1A, 0x1CEAF, 0x1FB00, 0x1FB80,
    0x1FB82, 0x1FB8B, 0x1FB95, 0x1FB97, 0x1FB9A, 0x1FB9B, 0x1FBCE, 0x1FBCF, 0x1FBE4, 0x1FBF9,
];

/// An indexed colour: Campbell, then the xterm cube and grey ramp.
pub fn indexed(index: u8) -> Rgb {
    match index {
        0..=15 => CAMPBELL[usize::from(index)],
        16..=231 => {
            let at = usize::from(index - 16);
            [CUBE[at / 36], CUBE[at / 6 % 6], CUBE[at % 6]]
        }
        _ => [8 + 10 * (index - 232); 3],
    }
}

/// The protocol's styled rows as bands and pieces.
pub fn rows(rows: &[Vec<TerminalRun>]) -> Vec<Row> {
    rows.iter().map(|runs| row(runs)).collect()
}

fn row(runs: &[TerminalRun]) -> Row {
    let mut bands: Row = Vec::new();
    for run in runs.iter().filter(|run| !run.text.is_empty()) {
        let (mut ink, mut back) = (ink(run.fg, Ink::Fore), ink(run.bg, Ink::Back));
        if run.inverse {
            std::mem::swap(&mut ink, &mut back);
        }
        let look = Look {
            ink,
            dim: run.dim,
            bold: run.bold,
            italic: run.italic,
            underline: run.underline,
            strike: run.strike,
        };
        if bands.last().is_none_or(|band| band.back != back) {
            bands.push(Band {
                back,
                cells: 0,
                pieces: Vec::new(),
            });
        }
        if let Some(band) = bands.last_mut() {
            for c in run.text.chars() {
                band.put(c, look);
            }
        }
    }
    bands
}

fn ink(color: Option<TerminalColor>, default: Ink) -> Ink {
    match color {
        None => default,
        Some(TerminalColor::Indexed(index)) => Ink::Rgb(indexed(index)),
        Some(TerminalColor::Rgb(rgb)) => Ink::Rgb(rgb),
    }
}

/// Whether every terminal face has `c`, so it takes the grid's advance.
fn covered(c: char) -> bool {
    let code = u32::from(c);
    let (ranges, _) = COVERED.as_chunks::<2>();
    ranges
        .partition_point(|[first, _]| *first <= code)
        .checked_sub(1)
        .and_then(|at| ranges.get(at))
        .is_some_and(|[_, last]| code <= *last)
}

impl Band {
    /// Adds `c` after the band's cells: on the grid with the face's characters before it, in a
    /// piece of its own when the face lacks it or it is wide, and joined to the character before
    /// it when it takes no cell (a combining mark, a variation selector).
    fn put(&mut self, c: char, look: Look) {
        let cells = c.width().unwrap_or(0);
        let boxed = cells > 1 || (cells == 1 && !covered(c));
        self.cells += cells;
        match self.pieces.last_mut() {
            Some(piece) if cells == 0 => {
                if let Some(run) = piece.runs.last_mut() {
                    run.text.push(c);
                }
            }
            Some(piece) if !boxed && !piece.boxed => {
                piece.cells += cells;
                match piece.runs.last_mut() {
                    Some(run) if run.look == look => run.text.push(c),
                    _ => piece.runs.push(Run {
                        text: c.into(),
                        look,
                    }),
                }
            }
            _ => self.pieces.push(Piece {
                cells,
                boxed,
                runs: vec![Run {
                    text: c.into(),
                    look,
                }],
            }),
        }
    }
}

impl Defaults {
    fn rgb(self, ink: Ink) -> Rgb {
        match ink {
            Ink::Fore => self.fore,
            Ink::Back => self.back,
            Ink::Rgb(rgb) => rgb,
        }
    }

    /// A band's fill; none on the default background, which is the surface's own.
    pub fn fill(self, back: Ink) -> Option<Rgb> {
        (back != Ink::Back).then(|| self.rgb(back))
    }

    /// A run's colour on its band's `back`: a dim run keeps `DIM_KEEPS` of its colour.
    pub fn text(self, look: Look, back: Ink) -> Rgb {
        let fore = self.rgb(look.ink);
        if !look.dim {
            return fore;
        }
        let back = self.rgb(back);
        std::array::from_fn(|at| {
            let mixed = f32::from(fore[at]) * DIM_KEEPS + f32::from(back[at]) * (1.0 - DIM_KEEPS);
            mixed.round() as u8
        })
    }
}

#[cfg(test)]
#[path = "screen_tests.rs"]
mod tests;
