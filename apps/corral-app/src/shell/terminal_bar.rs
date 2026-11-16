//! The terminal's top bar on the first row: a band of the brand and workspace path, the health,
//! then reach and the day and time; the sample tag after it at the right, off the band, so it
//! reads as a tag of its own and keeps its colour on the terminal's own background where a
//! terminal without bands draws the band in the standard grey. The band is a step of the
//! terminal's background where it has them (reversed without colours), tinted in the failure tone
//! while corrald is not connected.
//!
//! While every component is healthy the health is one `● healthy`, a stop of the keyboard focus
//! whose hint (on the status line) names each component and the reach; the moment one is not,
//! every component shows by name with its mark, as health is never behind a hint while something
//! is wrong.

use nsh_core::prelude::*;

use crate::model::health::{self, Condensed, SampleTag};
use crate::model::{Segment, Snapshot, Tone};
use crate::state::AppState;
use crate::ui::kit::hint::hinted;
use crate::ui::term::{cells, columns, quiet, rows, space};
use crate::ui::tokens::{Token, color, tone};

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

/// What the top bar shows at its width. Nothing is ever cut: whole parts give way, the least
/// telling first. The path and reach go; then the brand; then the values of healthy segments
/// (their `●` says it); then the day, before the time; then the time; then every segment's
/// value, the marks' shapes still telling a warning and a failure apart. The sample tag never
/// goes, so sample data never passes for live.
struct Bar<'a> {
    brand: bool,
    path: Option<&'a str>,
    /// The one chip of a healthy line, in place of the segments.
    condensed: Option<&'a Condensed>,
    reach: Option<&'a str>,
    /// The day and time, or the time alone once the day has given way.
    stamp: Option<&'a str>,
    /// The time alone, what the stamp shortens to.
    time: Option<&'a str>,
    values: Values,
    sample: Option<&'a SampleTag>,
}

/// Which health segments keep their value after their name.
#[derive(Debug, Clone, Copy)]
enum Values {
    All,
    /// Those not healthy.
    Unwell,
    Hidden,
}

impl Values {
    fn shows(self, segment_tone: Tone) -> bool {
        match self {
            Values::All => true,
            Values::Unwell => segment_tone != Tone::Ok,
            Values::Hidden => false,
        }
    }
}

fn width(content: &str) -> usize {
    content.chars().count()
}

fn segment_width(segment: &Segment, value: bool) -> usize {
    width(mark(segment.tone))
        + 1
        + width(&segment.name)
        + if value { 1 + width(&segment.value) } else { 0 }
}

/// Items `GAP` apart; zero for none.
fn group(items: impl Iterator<Item = usize>) -> Option<usize> {
    items.fold(None, |sum, item| {
        Some(sum.map_or(item, |sum| sum + GAP + item))
    })
}

impl<'a> Bar<'a> {
    fn of(reading: &'a Reading) -> Bar<'a> {
        let snapshot = &reading.snapshot;
        let health = &snapshot.health;
        // Disconnected, only the segments show: nothing else may pass for a healthy reading.
        let connected = snapshot.connected();
        Bar {
            brand: true,
            path: snapshot.workspace.as_ref().map(|ws| ws.path.as_ref()),
            condensed: reading.condensed.as_ref(),
            // The healthy chip's hint says the reach.
            reach: health
                .reach
                .as_deref()
                .filter(|_| connected && reading.condensed.is_none()),
            stamp: reading.stamp.as_deref(),
            time: health.clock.as_deref().filter(|_| connected),
            values: Values::All,
            sample: reading.sample.as_ref(),
        }
    }

    fn meta(&self) -> impl Iterator<Item = &'a str> {
        [self.reach, self.stamp].into_iter().flatten()
    }

    /// The Cells the bar takes: the band (a Cell of padding each side, the brand and path and the
    /// line after them, the health, the least room before reach and the stamp), then the sample
    /// tag with a Cell each side.
    fn width(&self, segments: &[Segment]) -> usize {
        let left = self
            .brand
            .then(|| group([width(BRAND)].into_iter().chain(self.path.map(width))))
            .flatten()
            .map_or(0, |left| left + SEPARATOR);
        let lead = match self.condensed {
            Some(condensed) => width(mark(Tone::Ok)) + 1 + width(condensed.word),
            None => group(
                segments
                    .iter()
                    .map(|segment| segment_width(segment, self.values.shows(segment.tone))),
            )
            .unwrap_or(0),
        };
        let meta = group(self.meta().map(width)).unwrap_or(0);
        let sample = self.sample.map_or(0, |sample| 2 + width(sample.word));
        2 + left + lead + SEPARATOR + meta + sample
    }

    fn fit(mut self, segments: &[Segment], room: usize) -> Bar<'a> {
        let steps: [fn(&mut Bar<'a>); 7] = [
            |bar| bar.path = None,
            |bar| bar.reach = None,
            |bar| bar.brand = false,
            |bar| bar.values = Values::Unwell,
            |bar| bar.stamp = bar.stamp.and(bar.time),
            |bar| bar.stamp = None,
            |bar| bar.values = Values::Hidden,
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

/// What the bar draws, read (tracked) from the snapshot and the feed's clock.
struct Reading {
    snapshot: Snapshot,
    condensed: Option<Condensed>,
    stamp: Option<String>,
    sample: Option<SampleTag>,
}

impl Reading {
    fn of(app: AppState) -> Reading {
        let snapshot = app.snapshot();
        let stamp = if snapshot.connected() {
            app.feed.with(health::stamp)
        } else {
            None
        };
        Reading {
            condensed: health::condensed(&snapshot),
            sample: health::sample_tag(&snapshot),
            stamp,
            snapshot,
        }
    }
}

/// The first row. test_id "health" on the bar, "health-source" on the sample tag.
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
            let column = cells(1.0).0;
            let room = room.get().map_or_else(columns, |width| {
                (width / column + 0.01).floor().max(0.0) as usize
            });
            bar(app, &Reading::of(app), room)
        }))
        .into_element()
}

fn bar(app: AppState, reading: &Reading, room: usize) -> Vec<AnyElement> {
    let segments = &reading.snapshot.health.segments;
    let fitted = Bar::of(reading).fit(segments, room);
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
    match fitted.condensed {
        Some(condensed) => band.push(hinted(
            app,
            text(mark(Tone::Ok))
                .flex_shrink_0()
                .number_of_lines(1u32)
                .text_color(tone(Tone::Ok))
                .child(text(format!(" {}", condensed.word)).text_color(color(Token::Ink))),
            condensed.hint.clone(),
        )),
        None => {
            for (at, segment) in segments.iter().enumerate() {
                if at > 0 {
                    band.push(space(GAP as f32));
                }
                band.push(health_segment(segment, fitted.values.shows(segment.tone)));
            }
        }
    }
    band.push(
        view()
            .flex_1()
            .min_w(cells(SEPARATOR as f32))
            .into_element(),
    );
    // Reach is secondary; the stamp says when it is, and a faded one on the standard grey of a
    // terminal without bands would not read.
    let meta = [(fitted.reach, false), (fitted.stamp, true)]
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
    let fill = if reading.snapshot.connected() {
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
    parts.extend(fitted.sample.map(|sample| {
        view()
            .flex_row()
            .flex_shrink_0()
            .px(cells(1.0))
            .child(hinted(
                app,
                text(sample.word)
                    .test_id("health-source")
                    .number_of_lines(1u32)
                    .text_color(tone(Tone::Warn)),
                sample.hint.to_string(),
            ))
            .into_element()
    }));
    parts
}

/// `● name value`: the mark in the tone; a warning or failure also has its words in the tone
/// and bold, so it reads as one without colour. Short of room the value gives way to the mark.
fn health_segment(segment: &Segment, value: bool) -> AnyElement {
    let loud = matches!(segment.tone, Tone::Warn | Tone::Fail);
    let words = if value {
        format!(" {} {}", segment.name, segment.value)
    } else {
        format!(" {}", segment.name)
    };
    text(mark(segment.tone))
        .flex_shrink_0()
        .number_of_lines(1u32)
        .text_color(tone(segment.tone))
        .child(
            text(words)
                .text_color(if loud {
                    tone(segment.tone)
                } else {
                    color(Token::Ink)
                })
                .when(loud, |t| t.font_weight(FontWeight::BOLD)),
        )
        .into_element()
}
