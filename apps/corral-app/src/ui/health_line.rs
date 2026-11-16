//! The health line on the shell: the desktop top bar's, and the phone's strip under the status
//! bar (its one-row line while the window is short). It stays dark in both appearances, as the
//! shell does ([`on_shell`]).
//!
//! While every component is healthy the line is one chip, the ok dot and the model's word, its
//! hint naming each component and the reach; the moment one is not, each component shows by name
//! in its tone, so a problem is never behind a hover. After it the desktop bar has the reach
//! (unless the chip's hint says it), the day and time, and the sample tag over preview data; the
//! phone keeps reach, attention, the clock and the source label.
//!
//! The line never cuts a segment or the source label. When a line runs out of room, reach gives
//! way first, then attention, then the clock, decided from the line's own measured width; on the
//! phone strip reach (R10) holds longest: attention, then the clock, give way before it. A
//! warning segment is outlined and a failing one filled, so either reads without its colour; the
//! daemon's segment (or the chip) and the reach press through to Settings › Connection, and so
//! does the whole phone strip.

use nsh_core::prelude::*;

use crate::model::health::{self, Condensed, SampleTag};
use crate::model::{Form, Segment, SettingsSection, Snapshot, Tone, compact_segments};
use crate::state::AppState;
use crate::ui::atoms::{TOUCH_TARGET, on_shell, touch_shell};
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::hint::{hint_on, hinted};
use crate::ui::tokens::{FONT_MONO, Token, color};

/// Text size of the health line and of shell captions on desktop, as in the mock.
const CAPTION: f32 = 11.5;
/// The desktop bar's source tag, a step under its caption.
const SOURCE_CAPTION: f32 = 10.5;
/// The phone strip's text, the desktop caption's size; a narrow phone folds healthy runtimes.
const STRIP_CAPTION: f32 = 11.5;
/// Height of each row of the phone strip.
const STRIP_ROW: f32 = 20.0;
/// Below this window width the strip's segments no longer share one row at its text size, so
/// healthy runtimes fold into one segment.
const NARROW_STRIP: f32 = 400.0;
const FOCUS_RING: f32 = 2.0;
/// Space before reach, attention, clock and source in the desktop bar.
const BAR_GAP: f32 = 12.0;
/// Least space after reach, attention and clock on the phone strip, so also before the source.
const STRIP_GAP: f32 = 10.0;
/// Rounding in measured widths: a line within this many points of its room fits.
const SLACK: f32 = 0.5;
/// The rounding of an outlined or filled segment and of the reach button (`.hseg`, `.reach`).
const SEGMENT_RADIUS: f32 = 5.0;
/// Room between two framed segments side by side (`.hseg.bad + .hseg.bad`).
const FRAMED_GAP: f32 = 3.0;

/// The parts after the segments, by index: what [`Extras::parts`] lists and [`Fit`] measures.
const REACH: usize = 0;
const ATTENTION: usize = 1;
const CLOCK: usize = 2;
const SOURCE: usize = 3;
/// The segments together, measured as one in the desktop bar.
const SEGMENTS: usize = 4;
/// Reach, attention and clock give way, in that order; the source label never does.
const GIVE_WAY: usize = 3;
/// On the phone strip attention gives way first, then the clock, then reach as a last resort.
const STRIP_GIVE_WAY: [usize; 3] = [ATTENTION, CLOCK, REACH];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthLayout {
    /// The desktop top bar (and the phone's while its window is short): one row on the shell
    /// colours.
    Bar,
    /// The phone strip under the status bar: the segments, then attention and clock, with reach
    /// and the source label at the trailing edge.
    Strip,
}

/// The health line for the current snapshot. test_id "health" on the container,
/// "health-source" on the source label or the sample tag. In the bar the daemon's segment or the
/// chip ("btn-connection") and the reach ("btn-reach") open Settings › Connection; the phone strip
/// is one button that does.
pub fn health_line(app: AppState, layout: HealthLayout) -> AnyElement {
    match layout {
        HealthLayout::Bar => bar_line(
            app,
            view().test_id("health").role(AccessibilityRole::Summary),
        ),
        HealthLayout::Strip => view()
            .test_id("health")
            .role(AccessibilityRole::Button)
            .cursor_pointer()
            .focus_visible(|s| {
                s.outline(px(FOCUS_RING))
                    .outline_color(on_shell(Token::Focus))
                    .outline_offset(px(-FOCUS_RING))
            })
            .on_press(move || app.go_settings(SettingsSection::Connection))
            .gap(px(2.0))
            .child({
                // Only crossing the width rebuilds the strip, not every resize.
                let narrow = computed(|| window().width.0 < NARROW_STRIP);
                dynamic(move || strip(app, &app.snapshot(), narrow.get()))
            })
            .into_element(),
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

    /// The bar's parts after its lead. Desktop: the reach while the line is not one chip (the
    /// chip's hint says it), the day and time, the sample tag; no attention, which the sidebar's
    /// counts say. A phone's short window keeps the strip's parts.
    fn extras(&self, form: Form) -> Option<Extras<'_>> {
        if form == Form::Phone {
            return Extras::strip(&self.snapshot);
        }
        let health = &self.snapshot.health;
        self.snapshot.connected().then(|| Extras {
            reach: health.reach.as_deref().filter(|_| self.condensed.is_none()),
            e2e: health.e2e,
            attention: None,
            clock: self.stamp.as_deref(),
            source: self.sample.as_ref().map(|sample| sample.word),
            source_hint: self.sample.as_ref().map(|sample| sample.hint.as_ref()),
        })
    }
}

/// The parts the snapshot shows after its segments; none while disconnected, so nothing else can
/// pass for a healthy reading.
struct Extras<'a> {
    reach: Option<&'a str>,
    /// The reach path is end-to-end encrypted: a lock leads it.
    e2e: bool,
    attention: Option<&'a str>,
    clock: Option<&'a str>,
    source: Option<&'a str>,
    /// What the sample tag stands for, on hover or keyboard focus.
    source_hint: Option<&'a str>,
}

impl<'a> Extras<'a> {
    /// The phone's parts: reach, attention, the clock and the source label.
    fn strip(snapshot: &'a Snapshot) -> Option<Extras<'a>> {
        let health = &snapshot.health;
        snapshot.connected().then_some(Extras {
            reach: health.reach.as_deref(),
            e2e: health.e2e,
            attention: health.attention.as_deref(),
            clock: health.clock.as_deref(),
            source: health.source.as_deref(),
            source_hint: None,
        })
    }

    /// Reach, attention, clock and source, in the order they show.
    fn parts(&self) -> [Option<&'a str>; 4] {
        [self.reach, self.attention, self.clock, self.source]
    }

    /// Which of [`Self::parts`] there are.
    fn presence(&self) -> [bool; 4] {
        self.parts().map(|part| part.is_some())
    }
}

/// The room a shell line has and the natural widths, in points, of its parts as they last laid
/// out: reach, attention, clock, source, then the segments together.
#[derive(Clone, Copy)]
struct Fit {
    room: Signal<Option<f32>>,
    widths: Signal<[f32; 5]>,
}

impl Fit {
    fn new() -> Fit {
        Fit {
            room: signal(None),
            widths: signal([0.0; 5]),
        }
    }

    fn measure(self, part: usize, width: f32) {
        let mut widths = self.widths.get_untracked();
        widths[part] = width;
        self.widths.set(widths);
    }

    /// How many of the parts that may give way go, in the line's order (reach, attention, clock
    /// in the bar; [`STRIP_GIVE_WAY`] on the strip), so that what stays fits the room;
    /// `needed(from)` is the width the line takes once the first `from` have gone. Nothing gives
    /// way before the line has been measured.
    fn give_way(self, needed: impl Fn(usize) -> f32) -> usize {
        self.room.get().map_or(0, |room| {
            (0..GIVE_WAY)
                .find(|&from| needed(from) <= room + SLACK)
                .unwrap_or(GIVE_WAY)
        })
    }
}

/// The parts present that stay once the first `from` of reach, attention and clock have given
/// way, with their widths. The source label always stays.
fn kept(present: [bool; 4], widths: [f32; 5], from: usize) -> impl Iterator<Item = (usize, f32)> {
    (from..present.len())
        .filter(move |&part| present[part])
        .map(move |part| (part, widths[part]))
}

/// The bar's width once the first `from` parts have given way: the segments, then each part
/// kept after its gap.
fn bar_needed(present: [bool; 4], widths: [f32; 5], from: usize) -> f32 {
    widths[SEGMENTS]
        + kept(present, widths, from)
            .map(|(_, width)| BAR_GAP + width)
            .sum::<f32>()
}

/// The phone strip's second row width once the first `from` of [`STRIP_GIVE_WAY`] have given
/// way: the parts kept, each with the least space after it, and the source label.
fn strip_needed(present: [bool; 4], widths: [f32; 5], from: usize) -> f32 {
    let gone = &STRIP_GIVE_WAY[..from.min(STRIP_GIVE_WAY.len())];
    [ATTENTION, CLOCK, REACH, SOURCE]
        .into_iter()
        .filter(|part| present[*part] && !gone.contains(part))
        .map(|part| {
            if part == SOURCE {
                widths[part]
            } else {
                widths[part] + STRIP_GAP
            }
        })
        .sum()
}

/// One part of a shell line in a box that reports its natural width. A part that has given way
/// stays laid out, out of the flow and invisible, so its width stays known and it comes back as
/// soon as the line has room for it again.
fn fitted(
    fit: Fit,
    part: usize,
    gone: impl Fn() -> bool + Copy + 'static,
    child: AnyElement,
) -> View {
    view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .on_layout(move |frame: &Bounds<Points>| fit.measure(part, frame.size.width.0))
        .style_when(gone, |s| s.absolute().invisible())
        .accessibility_elements_hidden(gone)
        .child(child)
}

/// The bar. Its intrinsic width is the whole line, so the top bar gives up its own room (the
/// workspace path) before any part of the line goes; its real width is its parent's.
fn bar_line(app: AppState, root: View) -> AnyElement {
    let fit = Fit::new();
    let full = computed(move || {
        let widths = fit.widths.get();
        let reading = Reading::of(app);
        let present = reading
            .extras(app.form)
            .map_or([false; 4], |extras| extras.presence());
        (widths[SEGMENTS] > 0.0).then(|| bar_needed(present, widths, 0))
    });
    root.flex_1()
        .flex_row()
        .items_center()
        .overflow_hidden()
        .style_with(move |s| match full.get() {
            Some(width) => s.w(px(width)).min_w(relative(1.0)).max_w(relative(1.0)),
            None => s.min_w(px(0.0)),
        })
        .on_layout(move |frame: &Bounds<Points>| fit.room.set(Some(frame.size.width.0)))
        .child(dynamic(move || bar(app, &Reading::of(app), fit)))
        .into_element()
}

fn bar(app: AppState, reading: &Reading, fit: Fit) -> Vec<AnyElement> {
    let lead = view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .on_layout(move |frame: &Bounds<Points>| fit.measure(SEGMENTS, frame.size.width.0));
    let lead = match &reading.condensed {
        Some(condensed) => lead.child(
            hint_on(
                app,
                to_connection(
                    app,
                    healthy_face(condensed.word, false),
                    "btn-connection",
                    condensed.word.to_owned(),
                    false,
                ),
                condensed.hint.clone(),
            )
            .into_element(),
        ),
        None => lead.children(reading.snapshot.health.segments.iter().enumerate().map(
            |(at, segment)| {
                let face = shell_segment(segment, false, at == 0);
                if at == 0 {
                    let filled = segment.tone == Tone::Fail;
                    to_connection(app, face, "btn-connection", segment_text(segment), filled)
                        .into_element()
                } else {
                    face.into_element()
                }
            },
        )),
    };
    let mut parts = vec![lead.into_element()];
    if let Some(extras) = reading.extras(app.form) {
        let present = extras.presence();
        let cut = computed(move || {
            let widths = fit.widths.get();
            fit.give_way(|from| bar_needed(present, widths, from))
        });
        parts.push(spacer());
        parts.extend(
            extras
                .parts()
                .into_iter()
                .enumerate()
                .filter_map(|(part, content)| {
                    content.map(|content| {
                        fitted(
                            fit,
                            part,
                            move || cut.get() > part,
                            shell_part(app, part, content, &extras, CAPTION),
                        )
                        .ml(px(BAR_GAP))
                        .into_element()
                    })
                }),
        );
    }
    parts
}

/// Row 1: the segments, wrapping whole segments if a phone is too narrow for them; on a
/// `narrow` one healthy runtimes share one segment; all healthy, the one chip. Row 2: attention
/// and clock on the left, reach and the source label at the trailing edge.
fn strip(app: AppState, snapshot: &Snapshot, narrow: bool) -> Vec<AnyElement> {
    let row = view().flex_row().flex_wrap().items_center().gap_y(px(2.0));
    let row = match health::condensed(snapshot) {
        Some(condensed) => row.child(healthy_face(condensed.word, true)),
        None => {
            let segments = if narrow {
                compact_segments(&snapshot.health)
            } else {
                snapshot.health.segments.clone()
            };
            row.children(
                segments
                    .iter()
                    .enumerate()
                    .map(|(at, segment)| shell_segment(segment, true, at == 0)),
            )
        }
    };
    let mut rows = vec![row.into_element()];
    rows.extend(Extras::strip(snapshot).map(|extras| strip_extras(app, &extras)));
    rows
}

/// The strip's second row. The source label wins the room: attention gives way first, then the
/// clock, then reach.
fn strip_extras(app: AppState, extras: &Extras<'_>) -> AnyElement {
    let present = extras.presence();
    let fit = Fit::new();
    let cut = computed(move || {
        let widths = fit.widths.get();
        fit.give_way(|from| strip_needed(present, widths, from))
    });
    let parts = extras.parts();
    let item = |part: usize| {
        let order = STRIP_GIVE_WAY.iter().position(|&gives| gives == part);
        parts[part].map(|content| {
            fitted(
                fit,
                part,
                move || order.is_some_and(|order| cut.get() > order),
                shell_part(app, part, content, extras, STRIP_CAPTION),
            )
            .mr(px(STRIP_GAP))
            .into_element()
        })
    };
    view()
        .flex_row()
        .items_center()
        .h(px(STRIP_ROW))
        .min_w(px(0.0))
        .overflow_hidden()
        .on_layout(move |frame: &Bounds<Points>| fit.room.set(Some(frame.size.width.0)))
        .children(item(ATTENTION))
        .children(item(CLOCK))
        .child(spacer())
        .children(item(REACH))
        .children(parts[SOURCE].map(|source| {
            fitted(fit, SOURCE, || false, shell_source(source, STRIP_CAPTION)).into_element()
        }))
        .into_element()
}

fn segment_text(segment: &Segment) -> String {
    format!("{} {}", segment.name, segment.value)
}

/// A segment's height, side padding, gap and text size: the bar's, or the phone strip's
/// `compact` ones.
fn metrics(compact: bool) -> (f32, f32, f32, f32) {
    if compact {
        (STRIP_ROW, 5.0, 4.0, STRIP_CAPTION)
    } else {
        (22.0, 10.0, 6.0, CAPTION)
    }
}

/// The one chip of a healthy line (`.hl`): the ok dot, then the word in the shell ink.
fn healthy_face(word: &'static str, compact: bool) -> View {
    let (height, pad, gap, size) = metrics(compact);
    view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .gap(px(gap))
        .h(px(height))
        .px(px(pad))
        .when(compact, |v| v.pl(px(0.0)))
        .child(tone_dot(on_shell(Token::Ok)))
        .child(
            text(word)
                .number_of_lines(1u32)
                .text_size(px(size))
                .text_color(color(Token::ShellInk)),
        )
}

/// One segment on the shell: tone dot, the name in the quieter shell ink, the value. A warning
/// is outlined in the held tone and worded in it; a failure is filled in the failure tone with
/// its words in the shell's own dark (6.7:1 on that fill; the light shell ink reached 2.5:1),
/// the value bold, so each reads as what it is without its colour. The compact form is the
/// phone strip's.
fn shell_segment(segment: &Segment, compact: bool, first: bool) -> View {
    let tone = segment.tone;
    let framed = matches!(tone, Tone::Warn | Tone::Fail);
    let held = on_shell(Token::Held);
    let (name_ink, value_ink, dot) = match tone {
        Tone::Ok => (
            color(Token::ShellInk2),
            color(Token::ShellInk),
            on_shell(Token::Ok),
        ),
        Tone::Muted => (
            color(Token::ShellInk2),
            color(Token::ShellInk),
            color(Token::ShellInk2),
        ),
        Tone::Warn => (held, held, held),
        Tone::Fail => {
            let ink = on_shell(Token::Shell);
            (ink, ink, ink)
        }
    };
    let (height, pad, gap, size) = metrics(compact);
    view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .gap(px(gap))
        .h(px(height))
        .px(px(pad))
        .when(compact && first && !framed, |v| v.pl(px(0.0)))
        .when(!first && !framed, |v| {
            v.border_l_1()
                .border_l_color(color(Token::ShellInk).opacity(0.16))
        })
        .when(framed && !first, |v| v.ml(px(FRAMED_GAP)))
        .when(framed, |v| v.rounded(px(SEGMENT_RADIUS)))
        .when(tone == Tone::Warn, |v| {
            v.border_1().border_color(held.opacity(0.55))
        })
        .when(tone == Tone::Fail, |v| v.bg(on_shell(Token::Fail)))
        .child(tone_dot(dot))
        .child(
            text(segment.name.as_ref().to_owned())
                .number_of_lines(1u32)
                .text_size(px(size))
                .text_color(name_ink),
        )
        .child(
            text(segment.value.as_ref().to_owned())
                .number_of_lines(1u32)
                .text_size(px(size))
                .text_color(value_ink)
                .when(tone == Tone::Fail, |t| t.font_weight(FontWeight::SEMIBOLD)),
        )
}

/// A shell control that opens Settings › Connection: the daemon's segment or the healthy chip,
/// the reach. A `filled` face (a failing segment) keeps its fill on hover and only fades a
/// little, as the shell's hover fill would put its dark words on a dark ground.
fn to_connection(app: AppState, face: View, test_id: &str, label: String, filled: bool) -> View {
    face.role(AccessibilityRole::Button)
        .when(touch_shell(), |v| v.min_h(px(TOUCH_TARGET)))
        .label(label)
        .test_id(test_id)
        .cursor_pointer()
        .rounded(px(SEGMENT_RADIUS))
        .hover(move |s| {
            if filled {
                s.opacity(0.9)
            } else {
                s.bg(color(Token::ShellSel))
            }
        })
        .focus_visible(|s| {
            s.outline(px(FOCUS_RING))
                .outline_color(on_shell(Token::Focus))
                .outline_offset(px(-FOCUS_RING))
        })
        .on_press(move || app.go_settings(SettingsSection::Connection))
}

fn tone_dot(fill: AdaptiveColor) -> AnyElement {
    view()
        .size(px(7.0))
        .flex_shrink_0()
        .rounded_full()
        .bg(fill)
        .accessibility_elements_hidden(true)
        .into_element()
}

/// Reach, attention, clock or source on the shell, as [`Extras::parts`] numbers them. The
/// desktop's reach is a button to Settings › Connection (the phone strip is one already), a
/// lock before it on an end-to-end path; attention leads with the changed-since mark; the
/// sample tag shows what it stands for on hover or keyboard focus.
fn shell_part(
    app: AppState,
    part: usize,
    content: &str,
    extras: &Extras<'_>,
    size: f32,
) -> AnyElement {
    match part {
        REACH => {
            let reach = view()
                .flex_row()
                .items_center()
                .gap(px(if app.form == Form::Phone { 4.0 } else { 6.0 }))
                .when(extras.e2e, |v| {
                    v.child(icon(Icon::Lock, size, color(Token::ShellInk)))
                })
                .child(shell_caption(content, Token::ShellInk, size));
            if app.form == Form::Desktop {
                to_connection(
                    app,
                    reach.px(px(8.0)).py(px(2.0)),
                    "btn-reach",
                    content.to_owned(),
                    false,
                )
                .into_element()
            } else {
                reach.into_element()
            }
        }
        ATTENTION => view()
            .flex_row()
            .items_center()
            .gap(px(3.0))
            .child(
                text("▲")
                    .text_size(px(size - 2.0))
                    .text_color(color(Token::ShellInk2))
                    .accessibility_elements_hidden(true),
            )
            .child(shell_caption(content, Token::ShellInk2, size))
            .into_element(),
        CLOCK => shell_clock(content, size),
        _ => {
            let tag = shell_source(content, SOURCE_CAPTION);
            match extras.source_hint {
                Some(hint) => hinted(app, tag, hint.to_owned()),
                None => tag,
            }
        }
    }
}

fn shell_caption(content: &str, ink: Token, size: f32) -> AnyElement {
    text(content.to_owned())
        .flex_shrink_0()
        .number_of_lines(1u32)
        .text_size(px(size))
        .text_color(color(ink))
        .into_element()
}

fn shell_clock(content: &str, size: f32) -> AnyElement {
    text(content.to_owned())
        .flex_shrink_0()
        .number_of_lines(1u32)
        .font_family(FONT_MONO)
        .tabular_nums()
        .text_size(px(size))
        .text_color(color(Token::ShellInk2))
        .into_element()
}

/// The source label or sample tag as the mock's tag: an outlined held-tone tag, so sample data
/// never reads as live. Its words are `size`: a step under the bar's caption, the strip's own
/// size.
fn shell_source(content: &str, size: f32) -> AnyElement {
    let held = on_shell(Token::Held);
    view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .h(px(size + 6.5))
        .px(px(6.0))
        .rounded(px(3.0))
        .border_1()
        .border_color(held.opacity(0.5))
        .child(
            text(content.to_owned())
                .test_id("health-source")
                .number_of_lines(1u32)
                .text_size(px(size))
                .letter_spacing(px(0.2))
                .text_color(held),
        )
        .into_element()
}

fn spacer() -> AnyElement {
    view().flex_1().min_w(px(0.0)).into_element()
}
