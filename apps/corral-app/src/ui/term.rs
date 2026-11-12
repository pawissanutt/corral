//! The terminal's own pieces: lines that join the frames they meet, framed regions with a title
//! set into their line, key caps, and the top bar.
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

use crate::model::{Segment, Snapshot, Tone};
use crate::state::AppState;
use crate::ui::tokens::{Token, color, tone};

/// One Cell at the default Cell size, where the surface reports none yet.
const COLUMN: f32 = 8.0;
const ROW: f32 = 16.0;
/// A grid at least this wide gives the sidebar its full width and Ask half the content.
const WIDE_COLUMNS: f32 = 110.0;
/// Longer than any line a terminal shows; the view a line sits in clips it.
const LINE_CELLS: usize = 400;
/// The box-drawing glyphs `─` and `│` in UTF-8.
const ACROSS_GLYPH: [u8; 3] = [0xE2, 0x94, 0x80];
const DOWN_GLYPH: [u8; 3] = [0xE2, 0x94, 0x82];

const ACROSS_BYTES: [u8; 3 * LINE_CELLS] = {
    let mut bytes = [0; 3 * LINE_CELLS];
    let mut at = 0;
    while at < bytes.len() {
        bytes[at] = ACROSS_GLYPH[at % 3];
        at += 1;
    }
    bytes
};
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
const ACROSS: &str = match std::str::from_utf8(&ACROSS_BYTES) {
    Ok(line) => line,
    Err(_) => "",
};
const DOWN: &str = match std::str::from_utf8(&DOWN_BYTES) {
    Ok(line) => line,
    Err(_) => "",
};

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

/// Whether the grid is wide enough for the full sidebar and a half-width Ask (tracked).
pub fn wide() -> bool {
    columns() as f32 >= WIDE_COLUMNS
}

/// The sidebar's width in Cells, its lines included (tracked): on a wide grid room for "This
/// device <name>" in its bottom line and the autopilot state on one row; otherwise the longest
/// Settings section on its indent.
pub fn sidebar_columns() -> f32 {
    if wide() { 32.0 } else { 24.0 }
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
    view()
        .h(rows(1.0))
        .flex_shrink_0()
        .min_w(px(0.0))
        .overflow_hidden()
        .accessibility_elements_hidden(true)
        .child(
            text(ACROSS)
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
            text(keys.into())
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

/// The mark a health tone leads its segment with: shaped, so a failure reads without its colour.
fn mark(segment_tone: Tone) -> &'static str {
    match segment_tone {
        Tone::Ok => "●",
        Tone::Warn => "▲",
        Tone::Fail => "✗",
        Tone::Muted => "○",
    }
}

const BRAND: &str = "◉ Corral";
/// Cells between two items of a group, and around the line between two groups.
const GAP: usize = 2;
const SEPARATOR: usize = 3;

/// What the top bar shows at its width. Nothing is ever cut: the path gives way first, then
/// reach, attention and the clock, then the brand; the source label never goes, shortened to
/// its first word at the last, so fixture data never passes for live.
struct Bar<'a> {
    brand: bool,
    path: Option<&'a str>,
    reach: Option<&'a str>,
    attention: Option<&'a str>,
    clock: Option<&'a str>,
    /// What shows of the source label, and the label itself.
    source: Option<(&'a str, &'a str)>,
}

fn width(content: &str) -> usize {
    content.chars().count()
}

fn segment_width(segment: &Segment) -> usize {
    width(mark(segment.tone)) + 1 + width(&segment.name) + 1 + width(&segment.value)
}

/// Items `GAP` apart; zero for none.
fn group(items: impl Iterator<Item = usize>) -> Option<usize> {
    items.fold(None, |sum, item| {
        Some(sum.map_or(item, |sum| sum + GAP + item))
    })
}

impl<'a> Bar<'a> {
    fn of(snapshot: &'a Snapshot) -> Bar<'a> {
        let health = &snapshot.health;
        // Disconnected, only the segments show: nothing else may pass for a healthy reading.
        let connected = snapshot.connected();
        Bar {
            brand: true,
            path: snapshot.workspace.as_ref().map(|ws| ws.path.as_ref()),
            reach: health.reach.as_deref().filter(|_| connected),
            attention: health.attention.as_deref().filter(|_| connected),
            clock: health.clock.as_deref().filter(|_| connected),
            source: health
                .source
                .as_deref()
                .filter(|_| connected)
                .map(|source| (source, source)),
        }
    }

    fn meta(&self) -> impl Iterator<Item = &'a str> {
        [self.reach, self.attention, self.clock]
            .into_iter()
            .flatten()
    }

    /// The Cells the bar takes: the band (a Cell of padding each side, the brand and path and the
    /// line after them, the segments, the least room before reach, attention and the clock),
    /// then the source label with a Cell each side.
    fn width(&self, segments: usize) -> usize {
        let left = self
            .brand
            .then(|| group([width(BRAND)].into_iter().chain(self.path.map(width))))
            .flatten()
            .map_or(0, |left| left + SEPARATOR);
        let meta = group(self.meta().map(width)).unwrap_or(0);
        let source = self.source.map_or(0, |(shown, _)| 2 + width(shown));
        2 + left + segments + SEPARATOR + meta + source
    }

    fn fit(mut self, segments: usize, room: usize) -> Bar<'a> {
        let steps: [fn(&mut Bar<'a>); 6] = [
            |bar| bar.path = None,
            |bar| bar.reach = None,
            |bar| bar.attention = None,
            |bar| bar.clock = None,
            |bar| bar.brand = false,
            |bar| {
                bar.source = bar
                    .source
                    .map(|(_, full)| (full.split_whitespace().next().unwrap_or(full), full))
            },
        ];
        for step in steps {
            if self.width(segments) <= room {
                break;
            }
            step(&mut self);
        }
        self
    }
}

/// The first row: a band of the brand and workspace path, the health segments, then reach,
/// attention and the clock; the source label after it at the right, off the band, so it reads as
/// a tag of its own and keeps its colour on the terminal's own background where a terminal
/// without bands draws the band in the standard grey. The band is a step of the terminal's
/// background where it has them (reversed without colours), tinted in the failure tone while
/// corrald is not connected. test_id "health" on the bar, "health-source" on the source label.
pub fn top_bar(app: AppState) -> AnyElement {
    let room = signal(None::<f32>);
    view()
        .test_id("health")
        .role(AccessibilityRole::Summary)
        .h(rows(1.0))
        .flex_shrink_0()
        .flex_row()
        .items_center()
        .overflow_hidden()
        .on_layout(move |frame: &Bounds<Points>| room.set(Some(frame.size.width.0)))
        .child(dynamic(move || {
            let column = cell_size().map_or(COLUMN, |cell| cell.width.0);
            let room = room.get().map_or_else(columns, |width| {
                (width / column + 0.01).floor().max(0.0) as usize
            });
            bar(&app.snapshot(), room)
        }))
        .into_element()
}

fn bar(snapshot: &Snapshot, room: usize) -> Vec<AnyElement> {
    let segments = &snapshot.health.segments;
    let segments_width = group(segments.iter().map(segment_width)).unwrap_or(0);
    let fitted = Bar::of(snapshot).fit(segments_width, room);
    let mut band = Vec::new();
    if fitted.brand {
        band.push(
            text("◉ ")
                .flex_shrink_0()
                .text_color(color(Token::Ink))
                .child(text("Corral").font_weight(FontWeight::BOLD))
                .into_element(),
        );
        if let Some(path) = fitted.path {
            band.push(space(GAP as f32));
            band.push(quiet(path.to_owned()).into_element());
        }
        band.push(
            view()
                .flex_row()
                .flex_shrink_0()
                .child(space(1.0))
                .child(quiet("│"))
                .child(space(1.0))
                .into_element(),
        );
    }
    for (at, segment) in segments.iter().enumerate() {
        if at > 0 {
            band.push(space(GAP as f32));
        }
        band.push(health_segment(segment));
    }
    band.push(
        view()
            .flex_1()
            .min_w(cells(SEPARATOR as f32))
            .into_element(),
    );
    // Reach and attention are secondary; the clock says when the reading is from, and a faded
    // clock on the standard grey of a terminal without bands would not read.
    let meta = [
        (fitted.reach, false),
        (fitted.attention, false),
        (fitted.clock, true),
    ]
    .into_iter()
    .filter_map(|(part, plain)| part.map(|part| (part, plain)));
    for (at, (part, plain)) in meta.enumerate() {
        if at > 0 {
            band.push(space(GAP as f32));
        }
        band.push(if plain {
            quiet(part.to_owned())
                .text_color(color(Token::Ink))
                .into_element()
        } else {
            quiet(part.to_owned()).into_element()
        });
    }
    let fill = if snapshot.connected() {
        color(Token::Surface)
    } else {
        color(Token::Fail).opacity(0.22)
    };
    let mut parts = vec![
        view()
            .flex_1()
            .min_w(px(0.0))
            .h(rows(1.0))
            .flex_row()
            .items_center()
            .px(cells(1.0))
            .bg(fill)
            .children(band)
            .into_element(),
    ];
    parts.extend(fitted.source.map(|(shown, full)| {
        view()
            .flex_row()
            .flex_shrink_0()
            .px(cells(1.0))
            .child(
                text(shown.to_owned())
                    .test_id("health-source")
                    .label(full.to_owned())
                    .number_of_lines(1u32)
                    .text_color(tone(Tone::Warn)),
            )
            .into_element()
    }));
    parts
}

/// `● name value`: the mark in the tone; a warning or failure also has its words in the tone
/// and bold, so it reads as one without colour.
fn health_segment(segment: &Segment) -> AnyElement {
    let loud = matches!(segment.tone, Tone::Warn | Tone::Fail);
    text(mark(segment.tone))
        .flex_shrink_0()
        .number_of_lines(1u32)
        .text_color(tone(segment.tone))
        .child(
            text(format!(" {} {}", segment.name, segment.value))
                .text_color(if loud {
                    tone(segment.tone)
                } else {
                    color(Token::Ink)
                })
                .when(loud, |t| t.font_weight(FontWeight::BOLD)),
        )
        .into_element()
}
