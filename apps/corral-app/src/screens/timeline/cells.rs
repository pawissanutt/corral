//! The timeline in a terminal: the lanes as rows of Cells, a glyph per kind of segment so the
//! chart reads without colour, measured to the Cells the pane has; and the burn-up as two rows of
//! block heights. The geometry is the model's desktop layout scaled onto the Cells.

use nsh_core::EllipsizeMode;
use nsh_core::prelude::*;

use super::shown::Shown;
use crate::model::Hue;
use crate::model::timeline::{self, BurnupSeries, Kind, Layout};
use crate::ui::kit::marks::hue_ink;
use crate::ui::term;
use crate::ui::tokens::{Token, color};

/// The model lays out at the mockup's desktop width; the plot is then scaled onto the Cells.
const MODEL_WIDTH: f32 = 1190.0;
/// The burn-up's Cells: the mockup's fourteen days at two Cells a day.
const BURNUP_CELLS: usize = 28;
/// Label column: id and title on a grid this wide or wider, the id alone below it.
const TITLES_FROM: usize = 64;
const LABEL_WIDE: usize = 24;
const LABEL_NARROW: usize = 7;
/// The id's column inside the label: its least, and its most, which keeps the wide label's
/// title at least ten Cells.
const ID_CELLS: usize = 7;
const ID_MAX: usize = 14;
/// Block heights from an eighth to full.
const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
/// What marks an owner action, the supervisor's bar and the last-visit line.
const OWNER_GLYPH: char = '●';
const PM_GLYPH: char = '━';
const GUIDE_GLYPH: char = '┊';

/// The glyph a kind of segment fills its Cells with: a heavy line for run, double for gate, a
/// dashed heavy line for review, squares for land, a faint dash for idle, crosses for a stall,
/// `╱` for waiting, a shade for a stale verdict, a heavier shade for awaiting
/// acceptance and a faint dotted line where corral recorded no times.
pub fn glyph(kind: Kind) -> char {
    match kind {
        Kind::Run => '━',
        Kind::Gate => '═',
        Kind::Review => '╍',
        Kind::Land => '■',
        Kind::Idle => '╌',
        Kind::Stall => '╳',
        Kind::Ask => '╱',
        Kind::Stale => '░',
        Kind::Held => '▒',
        Kind::Unavailable => '┄',
    }
}

/// The colour of a kind's glyphs: its state colour, idle quiet.
pub fn kind_ink(kind: Kind) -> AdaptiveColor {
    ink_color(ink_of(kind))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ink {
    Quiet,
    Hue(Hue),
}

fn ink_of(kind: Kind) -> Ink {
    match kind {
        Kind::Idle | Kind::Unavailable => Ink::Quiet,
        other => Ink::Hue(other.hue()),
    }
}

fn ink_color(ink: Ink) -> AdaptiveColor {
    match ink {
        Ink::Quiet => color(Token::Ink3),
        Ink::Hue(hue) => hue_ink(hue),
    }
}

type Cell = Option<(char, Ink)>;

/// One row's track: the plot's Cells, with the model's x mapped onto them.
struct Track {
    cells: Vec<Cell>,
    origin: f32,
    scale: f32,
}

impl Track {
    fn new(len: usize, layout: &Layout) -> Self {
        Self {
            cells: vec![None; len],
            origin: layout.label_width,
            scale: len as f32 / layout.plot_width,
        }
    }

    fn at(&self, x: f32) -> f32 {
        (x - self.origin) * self.scale
    }

    fn index(&self, cell: f32) -> usize {
        (cell.max(0.0) as usize).min(self.cells.len() - 1)
    }

    /// Fills the Cells from `x` for `width`, at least one.
    fn span(&mut self, x: f32, width: f32, glyph: char, ink: Ink) {
        let start = self.index(self.at(x).floor());
        let end = (self.at(x + width).ceil().max(0.0) as usize).clamp(start + 1, self.cells.len());
        for cell in &mut self.cells[start..end] {
            *cell = Some((glyph, ink));
        }
    }

    fn mark(&mut self, x: f32, glyph: char, ink: Ink) {
        let i = self.index(self.at(x).round());
        self.cells[i] = Some((glyph, ink));
    }

    /// The last-visit line, where nothing else is drawn.
    fn guide(&mut self, x: f32) {
        let i = self.index(self.at(x).round());
        if self.cells[i].is_none() {
            self.cells[i] = Some((GUIDE_GLYPH, Ink::Quiet));
        }
    }

    /// Writes `words` from `start` when they fit in free Cells; otherwise leaves the row as is.
    fn write(&mut self, start: usize, words: &str, ink: Ink) {
        let n = words.chars().count();
        if start + n > self.cells.len() || self.cells[start..start + n].iter().any(Option::is_some)
        {
            return;
        }
        for (cell, c) in self.cells[start..start + n].iter_mut().zip(words.chars()) {
            *cell = Some((c, ink));
        }
    }

    /// `words` from the Cell of `x`, moved left as far as needed to end inside the track.
    fn text(&mut self, x: f32, words: &str, ink: Ink) {
        let n = words.chars().count();
        let start = (self.at(x).floor().max(0.0) as usize).min(self.cells.len().saturating_sub(n));
        self.write(start, words, ink);
    }

    /// `words` ending at the Cell of `x`.
    fn text_end(&mut self, x: f32, words: &str, ink: Ink) {
        let end = (self.at(x).floor().max(0.0) as usize).min(self.cells.len());
        self.write(end.saturating_sub(words.chars().count()), words, ink);
    }
}

/// What a screen reader hears of a row.
enum Voice {
    Label(String),
    Text,
    Hidden,
}

/// `words` cut to leave a blank Cell after them in `width` Cells, with an ellipsis when cut.
fn column(words: &str, width: usize) -> String {
    let room = width.saturating_sub(1);
    let cut: String = if words.chars().count() > room {
        let mut kept: String = words.chars().take(room.saturating_sub(1)).collect();
        kept.push('…');
        kept
    } else {
        words.to_owned()
    };
    format!("{cut:<width$}")
}

fn piece(words: String, ink: Option<Ink>) -> Text {
    text(words).text_color(ink.map_or(color(Token::Ink3), ink_color))
}

/// The label column's Cells: the id's column and the whole label.
#[derive(Clone, Copy)]
struct Label {
    id: usize,
    width: usize,
}

/// A lane's label: the id quiet and the title in ink on a wide grid, the id alone otherwise.
fn lead(id: &str, title: &str, label: Label) -> Vec<Text> {
    if label.width == label.id {
        vec![piece(column(id, label.width), Some(Ink::Quiet))]
    } else {
        vec![
            piece(column(id, label.id), Some(Ink::Quiet)),
            piece(
                column(title, label.width - label.id),
                Some(Ink::Hue(Hue::Neutral)),
            ),
        ]
    }
}

/// One row: the label, then the Cells as runs of one glyph colour.
fn line(lead: Vec<Text>, cells: &[Cell], voice: Voice) -> AnyElement {
    let mut spans = lead;
    let mut run = String::new();
    let mut run_ink = None;
    for &cell in cells {
        let (glyph, ink) = match cell {
            Some((glyph, ink)) => (glyph, Some(ink)),
            None => (' ', None),
        };
        if ink != run_ink {
            if !run.is_empty() {
                spans.push(piece(std::mem::take(&mut run), run_ink));
            }
            run_ink = ink;
        }
        run.push(glyph);
    }
    if !run.is_empty() {
        spans.push(piece(run, run_ink));
    }
    let row = text("")
        .number_of_lines(1u32)
        .text_overflow(EllipsizeMode::Clip)
        .flex_shrink_0()
        .children(spans);
    match voice {
        Voice::Label(spoken) => row.label(SharedString::from(spoken)).into_element(),
        Voice::Text => row.into_element(),
        Voice::Hidden => row.accessibility_elements_hidden(true).into_element(),
    }
}

/// `name title, part, part, ...`: a lane's label for screen readers.
fn spoken<'a>((name, title): (&str, &str), parts: impl Iterator<Item = &'a str>) -> String {
    let parts: Vec<&str> = parts.collect();
    format!("{name} {title}, {}", parts.join(", "))
}

/// The lanes as rows of Cells across the pane: the hour labels, the owner's actions (`●`), the
/// supervisor (`━`, the relaunch in amber with its caption under it), then one row per task
/// with its segments as glyphs, its annotations on a row above, the question strip on a row
/// between, phases without recorded times written after its bars, the last-visit line as `┊`,
/// and the captions of the last visit and now at the foot. Narrow grids drop the titles to keep
/// the track. Each lane row tells a screen reader the task and every segment's label.
pub fn lanes(data: Memo<Shown>) -> AnyElement {
    let width = signal(0.0_f32);
    let plot = computed(move || data.with(|shown| shown.plot.clone()));
    view()
        .w_full()
        .flex_col()
        .on_layout(move |frame: &Bounds<Points>| width.set(frame.size.width.0))
        .child(dynamic(move || {
            let columns = (width.get() / term::cells(1.0).0).floor() as usize;
            let plot = plot.get()?;
            (columns > 0).then(|| grid(timeline::layout(&plot, MODEL_WIDTH, false), columns))
        }))
        .into_element()
}

fn grid(layout: Layout, columns: usize) -> AnyElement {
    // The id column holds the longest id with a blank after it (session lanes such as
    // `T-001/R1` are longer than task ids), within bounds that leave the title and track room.
    let longest = layout
        .lanes
        .iter()
        .map(|lane| lane.id.chars().count() + 1)
        .max()
        .unwrap_or(0);
    let label = if columns >= TITLES_FROM {
        Label {
            id: longest.clamp(ID_CELLS, ID_MAX),
            width: LABEL_WIDE,
        }
    } else {
        let width = longest.clamp(LABEL_NARROW, ID_MAX);
        Label { id: width, width }
    };
    let track = columns.saturating_sub(label.width).max(1);
    let blank = || vec![piece(" ".repeat(label.width), None)];
    let mut rows = Vec::new();

    let mut axis = Track::new(track, &layout);
    let mut free = 0;
    for tick in layout.ticks.iter().filter(|tick| !tick.label.is_empty()) {
        let n = tick.label.chars().count();
        let centre = axis.at(tick.x).round().max(0.0) as usize;
        let start = centre.saturating_sub(n / 2);
        if start >= free && start + n <= track {
            axis.write(start, &tick.label, Ink::Quiet);
            free = start + n + 1;
        }
    }
    rows.push(line(blank(), &axis.cells, Voice::Hidden));

    let mut owner = Track::new(track, &layout);
    for marker in &layout.owner {
        owner.mark(marker.x, OWNER_GLYPH, Ink::Hue(Hue::You));
    }
    if let Some(visit) = &layout.last_visit {
        owner.guide(visit.x);
    }
    rows.push(line(
        lead(layout.owner_label.0, layout.owner_label.1, label),
        &owner.cells,
        Voice::Label(spoken(
            layout.owner_label,
            layout.owner.iter().map(|m| m.accessible_label.as_str()),
        )),
    ));

    let mut pm = Track::new(track, &layout);
    let mut pm_notes = Track::new(track, &layout);
    let mut relaunched = false;
    for interval in &layout.pm {
        let ink = match interval.hue {
            Hue::Neutral => Ink::Quiet,
            hue => Ink::Hue(hue),
        };
        pm.span(interval.x, interval.width, PM_GLYPH, ink);
        if let Some(words) = &interval.label {
            pm_notes.text(interval.x, words, Ink::Quiet);
            relaunched = true;
        }
    }
    if let Some(visit) = &layout.last_visit {
        pm.guide(visit.x);
    }
    rows.push(line(
        lead(layout.pm_label.0, layout.pm_label.1, label),
        &pm.cells,
        Voice::Label(spoken(
            layout.pm_label,
            layout.pm.iter().map(|p| p.accessible_label.as_str()),
        )),
    ));
    if relaunched {
        rows.push(line(blank(), &pm_notes.cells, Voice::Hidden));
    }

    for lane in &layout.lanes {
        let mut notes = Track::new(track, &layout);
        let mut strip = Track::new(track, &layout);
        let mut bars = Track::new(track, &layout);
        let (mut noted, mut stripped) = (false, false);
        for segment in &lane.segments {
            let target = if segment.overlay {
                stripped = true;
                &mut strip
            } else {
                &mut bars
            };
            target.span(
                segment.x,
                segment.width,
                glyph(segment.kind),
                ink_of(segment.kind),
            );
            if let Some(words) = &segment.annotation {
                notes.text(segment.x, words, Ink::Hue(segment.hue));
                noted = true;
            }
        }
        // The lane's long waits belong to the row (TT-27): on the row over its bars, from the
        // track's start.
        if let Some(words) = &lane.wait {
            notes.text(layout.label_width, words, Ink::Hue(Hue::Held));
            noted = true;
        }
        if let Some(words) = &lane.missing {
            let end = lane
                .segments
                .iter()
                .filter(|segment| !segment.overlay)
                .map(|segment| segment.x + segment.width)
                .reduce(f32::max);
            match end {
                Some(end) => bars.text(end, &format!(" {words}"), Ink::Quiet),
                None => bars.text(layout.label_width, words, Ink::Quiet),
            }
        }
        if let Some(visit) = &layout.last_visit {
            bars.guide(visit.x);
        }
        if noted {
            rows.push(line(blank(), &notes.cells, Voice::Text));
        }
        if stripped {
            rows.push(line(blank(), &strip.cells, Voice::Hidden));
        }
        rows.push(line(
            lead(&lane.id, &lane.title, label),
            &bars.cells,
            Voice::Label(spoken(
                (lane.id.as_str(), lane.title.as_str()),
                lane.segments
                    .iter()
                    .map(|s| s.accessible_label.as_str())
                    .chain(lane.wait.as_deref())
                    .chain(lane.missing.as_deref()),
            )),
        ));
    }

    let mut foot = Track::new(track, &layout);
    foot.text_end(layout.now.x, &layout.now.label, Ink::Hue(Hue::Neutral));
    if let Some(visit) = &layout.last_visit {
        foot.text_end(visit.x, &visit.label, Ink::Quiet);
    }
    rows.push(line(blank(), &foot.cells, Voice::Text));

    view()
        .flex_col()
        .role(AccessibilityRole::Region)
        .label(layout.accessible_label.clone())
        .children(rows)
        .into_element()
}

/// The goal burn-up as block heights over the period's Cells, each Cell the counts recorded at
/// its moment: scope in quiet ink, landed in ink, each after its end label, the first and last
/// day under them. Screen readers hear the model's summary.
pub fn burnup(series: &BurnupSeries) -> AnyElement {
    let chart = timeline::burnup(series, 404.0, 150.0);
    let top = chart
        .grid
        .iter()
        .map(|&(value, _)| value)
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let [scope_label, landed_label] = &chart.series_labels;
    let label = scope_label
        .0
        .chars()
        .count()
        .max(landed_label.0.chars().count())
        + 2;
    // The counts at each Cell's moment: the last recorded point at or before it.
    let at = |cell: usize, pick: fn(&(f32, u32, u32)) -> u32| -> u32 {
        let moment = (cell as f32 + 0.5) / BURNUP_CELLS as f32;
        series
            .points
            .iter()
            .take_while(|point| point.0 <= moment)
            .last()
            .map_or(0, pick)
    };
    let bars = |pick: fn(&(f32, u32, u32)) -> u32| -> String {
        (0..BURNUP_CELLS)
            .map(|cell| {
                let step = ((at(cell, pick) as f32 / top) * BLOCKS.len() as f32)
                    .ceil()
                    .clamp(1.0, BLOCKS.len() as f32) as usize;
                BLOCKS[step - 1]
            })
            .collect()
    };
    let [first_day, last_day] = &chart.axis_labels;
    let between =
        BURNUP_CELLS.saturating_sub(first_day.0.chars().count() + last_day.0.chars().count());
    let rows = vec![
        line(
            vec![
                piece(column(&scope_label.0, label), Some(Ink::Quiet)),
                piece(bars(|point| point.1), Some(Ink::Quiet)),
            ],
            &[],
            Voice::Hidden,
        ),
        line(
            vec![
                piece(column(&landed_label.0, label), Some(Ink::Hue(Hue::Neutral))),
                piece(bars(|point| point.2), Some(Ink::Hue(Hue::Neutral))),
            ],
            &[],
            Voice::Hidden,
        ),
        line(
            vec![piece(
                format!(
                    "{}{}{}{}",
                    " ".repeat(label),
                    first_day.0,
                    " ".repeat(between),
                    last_day.0
                ),
                Some(Ink::Quiet),
            )],
            &[],
            Voice::Hidden,
        ),
    ];
    view()
        .flex_col()
        .accessible(true)
        .role(AccessibilityRole::Image)
        .label(chart.accessible_label)
        .children(rows)
        .into_element()
}
