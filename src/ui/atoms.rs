//! The small pieces every shell shares: the health line, key caps, counts and navigation rows.
//!
//! The shell surfaces (desktop title bar and sidebar, phone health strip) stay dark in both
//! appearances while pages follow the appearance. Colours drawn on the shell therefore take the
//! dark value of a tone in both appearances ([`on_shell`]); the light values are tuned for light
//! pages and fall under AA contrast on the dark shell. Counts and key caps carry their own fill,
//! because the same atom sits on the shell in the sidebar and on a page in the content.
//!
//! The health line never cuts a segment or the source label. When a line runs out of room, reach
//! gives way first, then attention, then the clock, decided from the line's own measured width.

use nsh_core::prelude::*;

use crate::model::{Destination, Form, Segment, Snapshot, Tone};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::term::cells;
use crate::ui::tokens::{FONT_MONO, Token, color, row_height, text_size};

/// Text size of the health line and of shell captions on desktop, as in the mock.
const CAPTION: f32 = 11.5;
/// The phone strip's smaller text, so the four segments share one row on a 390 pt phone.
const STRIP_CAPTION: f32 = 10.5;
/// Height of each row of the phone strip.
const STRIP_ROW: f32 = 18.0;
const FOCUS_RING: f32 = 2.0;
/// Space before reach, attention, clock and source in the desktop bar.
const BAR_GAP: f32 = 12.0;
/// Least space after reach, attention and clock on the phone strip, so also before the source.
const STRIP_GAP: f32 = 10.0;
/// Rounding in measured widths: a line within this many points of its room fits.
const SLACK: f32 = 0.5;

/// The parts after the segments, by index: what [`Extras::parts`] lists and [`Fit`] measures.
const REACH: usize = 0;
const ATTENTION: usize = 1;
const CLOCK: usize = 2;
const SOURCE: usize = 3;
/// The segments together, measured as one in the desktop bar.
const SEGMENTS: usize = 4;
/// Reach, attention and clock give way, in that order; the source label never does.
const GIVE_WAY: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthLayout {
    /// The desktop top bar: one row on the shell colours.
    Bar,
    /// The phone strip under the status bar: the segments, then reach, attention and clock with
    /// the source label at the trailing edge.
    Strip,
}

/// The health line for the current snapshot: segments (tone dot, name, value), then reach, attention,
/// clock, and the source label. test_id "health" on the container, "health-source" on the source label.
pub fn health_line(app: AppState, layout: HealthLayout) -> AnyElement {
    let root = view().test_id("health").role(AccessibilityRole::Summary);
    match layout {
        HealthLayout::Bar => bar_line(app, root),
        HealthLayout::Strip => root
            .gap(px(2.0))
            .child(dynamic(move || strip(&app.snapshot())))
            .into_element(),
    }
}

/// A key cap in the mono font: key_hint("/"), key_hint("Ctrl K").
pub fn key_hint(keys: &'static str) -> AnyElement {
    view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .h(px(16.0))
        .px(px(4.0))
        .rounded(px(3.0))
        .border_1()
        .border_color(color(Token::Rule2))
        .bg(color(Token::BgSunk))
        .child(
            text(keys)
                .font_family(FONT_MONO)
                .text_size(px(10.5))
                .text_color(color(Token::Ink3))
                .number_of_lines(1u32),
        )
        .into_element()
}

/// The destination's count; nothing when it has none. Inbox in the needs-you colour. test_id "count-<slug>".
pub fn count(app: AppState, destination: Destination) -> AnyElement {
    let form = app.form;
    let test_id = format!("count-{}", destination.slug());
    dynamic(move || {
        let snapshot = app.snapshot();
        snapshot
            .count(destination)
            .map(|n| count_chip(form, n, snapshot.needs_you(destination), &test_id))
    })
}

/// A pressable row for lists of destinations and sections: label, optional trailing element, selected look,
/// role button, the given test id. Height `row_height(app.form)`.
pub fn nav_row(
    app: AppState,
    label: &'static str,
    test_id: String,
    selected: impl Fn() -> bool + 'static,
    trailing: Option<AnyElement>,
    on_press: impl Fn() + 'static,
) -> AnyElement {
    row(app, None, label, test_id, selected, trailing, on_press)
}

/// nav_row with a leading icon in the label's colour.
pub fn nav_row_icon(
    app: AppState,
    icon: Icon,
    label: &'static str,
    test_id: String,
    selected: impl Fn() -> bool + 'static,
    trailing: Option<AnyElement>,
    on_press: impl Fn() + 'static,
) -> AnyElement {
    row(
        app,
        Some(icon),
        label,
        test_id,
        selected,
        trailing,
        on_press,
    )
}

/// The body of [`nav_row`] and [`nav_row_icon`]. The terminal shows no images, so a glyph is
/// left out there; its rows lead with a bar on the selected one instead, which reads at every
/// colour depth.
fn row(
    app: AppState,
    glyph: Option<Icon>,
    label: &'static str,
    test_id: String,
    selected: impl Fn() -> bool + 'static,
    trailing: Option<AnyElement>,
    on_press: impl Fn() + 'static,
) -> AnyElement {
    let form = app.form;
    let look = RowLook::of(form);
    let selected = computed(selected);
    let ink = move || {
        if selected.get() {
            look.ink_selected
        } else {
            look.ink
        }
    };
    let leading = match form {
        // A column of its own, so the labels line up whether a row is selected or not.
        Form::Terminal => Some(
            view()
                .w(cells(1.0))
                .flex_shrink_0()
                .accessibility_elements_hidden(true)
                .child(
                    text(move || if selected.get() { "▌" } else { " " })
                        .text_color(color(Token::Ink)),
                )
                .into_element(),
        ),
        Form::Desktop | Form::Phone => glyph
            .zip(look.icon)
            .map(|(glyph, size)| dynamic(move || icon(glyph, size, ink()))),
    };
    view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .gap(px(look.gap))
        .h(px(row_height(form)))
        .px(px(look.pad))
        .rounded(px(look.radius))
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(label)
        .accessibility_state(move || AccessibilityState {
            selected: selected.get(),
            ..AccessibilityState::default()
        })
        .test_id(&test_id)
        .style_when(selected, |s| s.bg(look.selected_bg))
        .hover(|s| s.bg(look.hover))
        .active(|s| s.bg(look.selected_bg))
        .when_some(look.focus, |row, focus| {
            row.focus_visible(|s| {
                s.outline(px(FOCUS_RING))
                    .outline_color(focus)
                    .outline_offset(px(-FOCUS_RING))
            })
        })
        .on_press(move || on_press())
        .children(leading)
        .child(
            text(label)
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_size(px(text_size(form)))
                .text_color(ink)
                // Weight marks the selection for those who cannot tell the fills apart.
                .style_when(selected, |s| s.font_weight(FontWeight::SEMIBOLD)),
        )
        .children(trailing)
        .into_element()
}

/// The colours and metrics of a navigation row on the surface its form lists rows on: the shell
/// sidebar on desktop, the page on phone, a framed sidebar in the terminal, whose labels are
/// full ink: a fainter ink fades there and would read as disabled.
#[derive(Clone, Copy)]
struct RowLook {
    ink: AdaptiveColor,
    ink_selected: AdaptiveColor,
    selected_bg: AdaptiveColor,
    hover: AdaptiveColor,
    /// `None` in the terminal, which bands and bolds the focused control itself.
    focus: Option<AdaptiveColor>,
    /// The leading icon's size; `None` in the terminal, which shows no images.
    icon: Option<f32>,
    pad: f32,
    gap: f32,
    radius: f32,
}

impl RowLook {
    fn of(form: Form) -> RowLook {
        match form {
            Form::Desktop => RowLook {
                ink: color(Token::ShellInk2),
                ink_selected: color(Token::ShellInk),
                selected_bg: color(Token::ShellSel),
                hover: color(Token::ShellSel).opacity(0.6),
                focus: Some(on_shell(Token::Focus)),
                icon: Some(16.0),
                pad: 8.0,
                gap: 10.0,
                radius: 6.0,
            },
            Form::Phone => RowLook {
                ink: color(Token::Ink),
                ink_selected: color(Token::Ink),
                selected_bg: color(Token::Sel),
                hover: color(Token::Hover),
                focus: Some(color(Token::Focus)),
                icon: Some(22.0),
                pad: 16.0,
                gap: 12.0,
                radius: 10.0,
            },
            // NSH draws a selected, hovered or pressed row's band itself: an app fill would be
            // a second one, and on a terminal without bands the standard grey the reversed
            // selection then draws its text in.
            Form::Terminal => RowLook {
                ink: color(Token::Ink),
                ink_selected: color(Token::Ink),
                selected_bg: AdaptiveColor::TRANSPARENT,
                hover: AdaptiveColor::TRANSPARENT,
                focus: None,
                icon: None,
                pad: 0.0,
                gap: 8.0,
                radius: 0.0,
            },
        }
    }
}

/// The token's dark value in both appearances, for marks drawn on the always-dark shell.
fn on_shell(token: Token) -> AdaptiveColor {
    let dark = color(token).dark;
    AdaptiveColor { light: dark, dark }
}

/// A tone's colour on the shell.
fn shell_tone(segment_tone: Tone) -> AdaptiveColor {
    match segment_tone {
        Tone::Ok => on_shell(Token::Ok),
        Tone::Warn => on_shell(Token::Held),
        Tone::Fail => on_shell(Token::Fail),
        Tone::Muted => color(Token::ShellInk2),
    }
}

/// A count in a pill of its own fill. The Inbox count (what needs you) is a solid needs-you pill in
/// bold; the others are quiet. A terminal's counts are figures in a column instead: a column of
/// small fills reads as one band there.
fn count_chip(form: Form, n: u32, needs_you: bool, test_id: &str) -> AnyElement {
    let label = if needs_you {
        format!("{n} need you")
    } else {
        n.to_string()
    };
    if form == Form::Terminal {
        return text(n.to_string())
            .test_id(test_id)
            .label(label)
            .flex_shrink_0()
            .text_color(color(if needs_you { Token::You } else { Token::Ink }))
            .when(needs_you, |t| t.font_weight(FontWeight::BOLD))
            .into_element();
    }
    let (fill, ink, weight) = if needs_you {
        (Token::You, Token::BtnInk, FontWeight::BOLD)
    } else {
        (Token::Surface2, Token::Ink2, FontWeight::MEDIUM)
    };
    let (size, height, pad) = match form {
        Form::Desktop => (11.0, 16.0, 5.0),
        Form::Phone | Form::Terminal => (13.0, 22.0, 7.0),
    };
    view()
        .flex_row()
        .items_center()
        .justify_center()
        .flex_shrink_0()
        .h(px(height))
        .min_w(px(height))
        .px(px(pad))
        .rounded(px(height / 2.0))
        .bg(color(fill))
        .child(
            text(n.to_string())
                .test_id(test_id)
                .label(label)
                .font_family(FONT_MONO)
                .tabular_nums()
                .text_size(px(size))
                .font_weight(weight)
                .text_color(color(ink)),
        )
        .into_element()
}

/// The parts the snapshot shows after its segments; none while disconnected, so nothing else can
/// pass for a healthy reading.
struct Extras {
    reach: Option<&'static str>,
    attention: Option<&'static str>,
    clock: Option<&'static str>,
    source: Option<&'static str>,
}

impl Extras {
    fn of(snapshot: &Snapshot) -> Option<Extras> {
        let health = &snapshot.health;
        snapshot.connected().then_some(Extras {
            reach: health.reach,
            attention: health.attention,
            clock: health.clock,
            source: health.source,
        })
    }

    /// Reach, attention, clock and source, in the order they show.
    fn parts(&self) -> [Option<&'static str>; 4] {
        [self.reach, self.attention, self.clock, self.source]
    }

    /// Which of [`Self::parts`] the snapshot has.
    fn presence(snapshot: &Snapshot) -> [bool; 4] {
        Extras::of(snapshot).map_or([false; 4], |extras| {
            extras.parts().map(|part| part.is_some())
        })
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

    /// How many of reach, attention and clock give way, from the front, so that what stays fits
    /// the room; `needed(from)` is the width the line takes once the first `from` have gone.
    /// Nothing gives way before the line has been measured.
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

/// The desktop bar's width once the first `from` parts have given way: the segments, then each
/// part kept after its gap.
fn bar_needed(present: [bool; 4], widths: [f32; 5], from: usize) -> f32 {
    widths[SEGMENTS]
        + kept(present, widths, from)
            .map(|(_, width)| BAR_GAP + width)
            .sum::<f32>()
}

/// The phone strip's second row width once the first `from` parts have given way: the parts
/// kept on the left, each with the least space after it, and the source label.
fn strip_needed(present: [bool; 4], widths: [f32; 5], from: usize) -> f32 {
    kept(present, widths, from)
        .map(|(part, width)| {
            if part == SOURCE {
                width
            } else {
                width + STRIP_GAP
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

/// The desktop bar. Its intrinsic width is the whole line, so the top bar gives up its own room
/// (the workspace path) before any part of the line goes; its real width is its parent's.
fn bar_line(app: AppState, root: View) -> AnyElement {
    let fit = Fit::new();
    let full = computed(move || {
        let widths = fit.widths.get();
        let present = Extras::presence(&app.snapshot());
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
        .child(dynamic(move || bar(&app.snapshot(), fit)))
        .into_element()
}

fn bar(snapshot: &Snapshot, fit: Fit) -> Vec<AnyElement> {
    let segments = view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .on_layout(move |frame: &Bounds<Points>| fit.measure(SEGMENTS, frame.size.width.0))
        .children(
            snapshot
                .health
                .segments
                .iter()
                .enumerate()
                .map(|(at, segment)| shell_segment(segment, false, at == 0)),
        )
        .into_element();
    let mut parts = vec![segments];
    if let Some(extras) = Extras::of(snapshot) {
        let present = Extras::presence(snapshot);
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
                            shell_part(part, content, CAPTION),
                        )
                        .ml(px(BAR_GAP))
                        .into_element()
                    })
                }),
        );
    }
    parts
}

/// Row 1: the four segments, wrapping whole segments if a phone is too narrow for them. Row 2:
/// reach, attention and clock on the left, the source label at the trailing edge.
fn strip(snapshot: &Snapshot) -> Vec<AnyElement> {
    let segments = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_y(px(2.0))
        .children(
            snapshot
                .health
                .segments
                .iter()
                .enumerate()
                .map(|(at, segment)| shell_segment(segment, true, at == 0)),
        )
        .into_element();
    let mut rows = vec![segments];
    rows.extend(
        Extras::of(snapshot).map(|extras| strip_extras(&extras, Extras::presence(snapshot))),
    );
    rows
}

/// The strip's second row. The source label wins the room: reach gives way first, then
/// attention, then the clock.
fn strip_extras(extras: &Extras, present: [bool; 4]) -> AnyElement {
    let fit = Fit::new();
    let cut = computed(move || {
        let widths = fit.widths.get();
        fit.give_way(|from| strip_needed(present, widths, from))
    });
    let parts = extras.parts();
    let left = parts[..SOURCE]
        .iter()
        .enumerate()
        .filter_map(|(part, content)| {
            content.map(|content| {
                fitted(
                    fit,
                    part,
                    move || cut.get() > part,
                    shell_part(part, content, STRIP_CAPTION),
                )
                .mr(px(STRIP_GAP))
                .into_element()
            })
        });
    view()
        .flex_row()
        .items_center()
        .h(px(STRIP_ROW))
        .min_w(px(0.0))
        .overflow_hidden()
        .on_layout(move |frame: &Bounds<Points>| fit.room.set(Some(frame.size.width.0)))
        .children(left)
        .child(spacer())
        .children(
            parts[SOURCE]
                .map(|source| fitted(fit, SOURCE, || false, shell_source(source)).into_element()),
        )
        .into_element()
}

fn segment_text(segment: &Segment) -> String {
    format!("{} {}", segment.name, segment.value)
}

/// One segment on the shell: tone dot and `<name> <value>`. A failing segment is framed and bold
/// so it reads as a failure without its colour. The compact form is the phone strip's.
fn shell_segment(segment: &Segment, compact: bool, first: bool) -> AnyElement {
    let fail = segment.tone == Tone::Fail;
    let ink = match segment.tone {
        Tone::Ok | Tone::Muted => color(Token::ShellInk),
        Tone::Warn | Tone::Fail => shell_tone(segment.tone),
    };
    let (height, pad, gap, size) = if compact {
        (STRIP_ROW, 5.0, 4.0, STRIP_CAPTION)
    } else {
        (22.0, 10.0, 6.0, CAPTION)
    };
    view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .gap(px(gap))
        .h(px(height))
        .px(px(pad))
        .when(compact && first && !fail, |v| v.pl(px(0.0)))
        .when(!first && !fail, |v| {
            v.border_l_1()
                .border_l_color(color(Token::ShellInk).opacity(0.16))
        })
        .when(fail, |v| {
            v.rounded(px(4.0))
                .border_1()
                .border_color(on_shell(Token::Fail).opacity(0.6))
                .bg(on_shell(Token::FailBg))
        })
        .child(tone_dot(shell_tone(segment.tone)))
        .child(
            text(segment_text(segment))
                .number_of_lines(1u32)
                .text_size(px(size))
                .text_color(ink)
                .when(fail, |t| t.font_weight(FontWeight::SEMIBOLD)),
        )
        .into_element()
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

/// Reach, attention, clock or source on the shell, as [`Extras::parts`] numbers them.
fn shell_part(part: usize, content: &'static str, size: f32) -> AnyElement {
    match part {
        REACH => shell_caption(content, Token::ShellInk, size),
        ATTENTION => shell_caption(content, Token::ShellInk2, size),
        CLOCK => shell_clock(content, size),
        _ => shell_source(content),
    }
}

fn shell_caption(content: &'static str, ink: Token, size: f32) -> AnyElement {
    text(content)
        .flex_shrink_0()
        .number_of_lines(1u32)
        .text_size(px(size))
        .text_color(color(ink))
        .into_element()
}

fn shell_clock(content: &'static str, size: f32) -> AnyElement {
    text(content)
        .flex_shrink_0()
        .number_of_lines(1u32)
        .font_family(FONT_MONO)
        .tabular_nums()
        .text_size(px(size))
        .text_color(color(Token::ShellInk2))
        .into_element()
}

/// The source label as the mock's snapshot tag: an outlined held-tone tag, so fixture data never
/// reads as live.
fn shell_source(content: &'static str) -> AnyElement {
    let held = on_shell(Token::Held);
    view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .h(px(17.0))
        .px(px(6.0))
        .rounded(px(3.0))
        .border_1()
        .border_color(held.opacity(0.5))
        .child(
            text(content)
                .test_id("health-source")
                .number_of_lines(1u32)
                .text_size(px(10.5))
                .letter_spacing(px(0.2))
                .text_color(held),
        )
        .into_element()
}

fn spacer() -> AnyElement {
    view().flex_1().min_w(px(0.0)).into_element()
}
