//! The small pieces every shell shares: the health line, key caps, counts and navigation rows.
//!
//! The shell surfaces (desktop title bar and sidebar, phone health strip) stay dark in both
//! appearances while pages follow the appearance. Colours drawn on the shell therefore take the
//! dark value of a tone in both appearances ([`on_shell`]); the light values are tuned for light
//! pages and fall under AA contrast on the dark shell. Counts and key caps carry their own fill,
//! because the same atom sits on the shell in the sidebar and on a page in the content.
//!
//! The health line never cuts a segment or the source label. When a line runs out of room, reach
//! gives way first, then attention, then the clock, decided from the line's own measured width;
//! on the phone strip reach (R10) holds longest: attention, then the clock, give way before it.
//! A warning segment is outlined and a failing one filled, so either reads without its colour;
//! the daemon's segment and the reach press through to Settings › Connection, and so does the
//! whole phone strip.

use nsh_core::AccessibilityCurrent;
use nsh_core::element::Idiom;
use nsh_core::prelude::*;

use crate::model::{Destination, Form, Segment, SettingsSection, Snapshot, Tone, compact_segments};
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::term::cells;
use crate::ui::tokens::{FONT_MONO, Token, color, row_height, text_size};

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
    /// The desktop top bar: one row on the shell colours.
    Bar,
    /// The phone strip under the status bar: the segments, then attention and clock, with reach
    /// and the source label at the trailing edge.
    Strip,
}

/// The touch target's floor (Apple's and the mock's 44 pt).
pub const TOUCH_TARGET: f32 = 44.0;

/// The desktop shell on a touch tablet (lib.rs gives a large tablet the desktop shell): there is
/// no pointer to aim finely, so its controls take the touch target's height.
pub fn touch_shell() -> bool {
    untracked(idiom) == Idiom::Pad
}

/// A control's height: `height` with a pointer, at least the touch target on a tablet.
pub fn control_height(height: f32) -> f32 {
    if touch_shell() {
        height.max(TOUCH_TARGET)
    } else {
        height
    }
}

/// The health line for the current snapshot: segments (tone dot, name, value), then reach (a
/// lock before it on an end-to-end path), attention, clock, and the source label. test_id
/// "health" on the container, "health-source" on the source label. In the desktop bar the
/// daemon's segment ("btn-connection") and the reach ("btn-reach") open Settings › Connection;
/// the phone strip is one button that does.
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

/// What a destination's row says to assistive technology: its name, then its count as the
/// count reads ("Inbox, 5 need you"), as the figure beside the name is part of what it shows.
pub fn destination_spoken(app: AppState, destination: Destination) -> impl Fn() -> SharedString {
    move || {
        let snapshot = app.snapshot();
        match snapshot.count(destination) {
            Some(n) => format!(
                "{}, {}",
                destination.label(),
                count_words(n, snapshot.needs_you(destination))
            )
            .into(),
            None => destination.label().into(),
        }
    }
}

/// A pressable row for lists of destinations and sections: label, optional trailing element,
/// selected look, role button, the given test id. Height `row_height(app.form)`. The selected
/// row is the current page for assistive technology.
pub fn nav_row(
    app: AppState,
    label: &'static str,
    test_id: String,
    selected: impl Fn() -> bool + 'static,
    trailing: Option<AnyElement>,
    on_press: impl Fn() + 'static,
) -> AnyElement {
    let spoken = move || SharedString::from(label);
    row(
        app, None, label, spoken, test_id, selected, trailing, on_press,
    )
}

/// nav_row with a leading icon in the label's colour, named by `spoken` for assistive
/// technology ([`destination_spoken`] for a destination with a count).
#[allow(clippy::too_many_arguments)]
pub fn nav_row_icon(
    app: AppState,
    icon: Icon,
    label: &'static str,
    spoken: impl Fn() -> SharedString + 'static,
    test_id: String,
    selected: impl Fn() -> bool + 'static,
    trailing: Option<AnyElement>,
    on_press: impl Fn() + 'static,
) -> AnyElement {
    row(
        app,
        Some(icon),
        label,
        spoken,
        test_id,
        selected,
        trailing,
        on_press,
    )
}

/// The body of [`nav_row`] and [`nav_row_icon`]. The terminal shows no images, so a glyph is
/// left out there; its rows lead with a bar on the selected one instead, which reads at every
/// colour depth.
#[allow(clippy::too_many_arguments)]
fn row(
    app: AppState,
    glyph: Option<Icon>,
    label: &'static str,
    spoken: impl Fn() -> SharedString + 'static,
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
        .h(px(control_height(row_height(form))))
        .px(px(look.pad))
        .rounded(px(look.radius))
        .cursor_pointer()
        .role(AccessibilityRole::Button)
        .label(spoken)
        // `current` is what a screen reader announces for the page shown (`aria-current`);
        // `selected` stays for the terminal, which bands the selected row from it.
        .accessibility_state(move || AccessibilityState {
            selected: selected.get(),
            current: selected.get().then_some(AccessibilityCurrent::Page),
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

/// A count as it is spoken: "5 need you" where the count is what needs the owner, else "5".
fn count_words(n: u32, needs_you: bool) -> String {
    if needs_you {
        format!("{n} need you")
    } else {
        n.to_string()
    }
}

/// A count. On the phone a pill of its own fill: the Inbox count (what needs you) a solid
/// needs-you pill in bold, the others quiet. The desktop sidebar's counts are figures on the
/// shell, as the mock's; a terminal's are figures in a column: a column of small fills reads as
/// one band there.
fn count_chip(form: Form, n: u32, needs_you: bool, test_id: &str) -> AnyElement {
    let label = count_words(n, needs_you);
    if form == Form::Terminal {
        return text(n.to_string())
            .test_id(test_id)
            .label(label)
            .flex_shrink_0()
            .text_color(color(if needs_you { Token::You } else { Token::Ink }))
            .when(needs_you, |t| t.font_weight(FontWeight::BOLD))
            .into_element();
    }
    if form == Form::Desktop {
        // The sidebar's `.nav .n`: a figure on the shell, the needs-you one bold in its colour.
        return text(n.to_string())
            .test_id(test_id)
            .label(label)
            .flex_shrink_0()
            .font_family(FONT_MONO)
            .tabular_nums()
            .text_size(px(11.0))
            .text_color(if needs_you {
                on_shell(Token::You)
            } else {
                color(Token::ShellInk2)
            })
            .when(needs_you, |t| t.font_weight(FontWeight::BOLD))
            .into_element();
    }
    let (fill, ink, weight) = if needs_you {
        (Token::You, Token::BtnInk, FontWeight::BOLD)
    } else {
        (Token::Surface2, Token::Ink2, FontWeight::MEDIUM)
    };
    let (size, height, pad) = (13.0, 22.0, 7.0);
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
struct Extras<'a> {
    reach: Option<&'a str>,
    /// The reach path is end-to-end encrypted: a lock leads it.
    e2e: bool,
    attention: Option<&'a str>,
    clock: Option<&'a str>,
    source: Option<&'a str>,
}

impl<'a> Extras<'a> {
    fn of(snapshot: &'a Snapshot) -> Option<Extras<'a>> {
        let health = &snapshot.health;
        snapshot.connected().then_some(Extras {
            reach: health.reach.as_deref(),
            e2e: health.e2e,
            attention: health.attention.as_deref(),
            clock: health.clock.as_deref(),
            source: health.source.as_deref(),
        })
    }

    /// Reach, attention, clock and source, in the order they show.
    fn parts(&self) -> [Option<&'a str>; 4] {
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

/// The desktop bar's width once the first `from` parts have given way: the segments, then each
/// part kept after its gap.
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
        .child(dynamic(move || bar(app, &app.snapshot(), fit)))
        .into_element()
}

fn bar(app: AppState, snapshot: &Snapshot, fit: Fit) -> Vec<AnyElement> {
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
                .map(|(at, segment)| {
                    let face = shell_segment(segment, false, at == 0);
                    if at == 0 {
                        let filled = segment.tone == Tone::Fail;
                        to_connection(app, face, "btn-connection", segment_text(segment), filled)
                    } else {
                        face.into_element()
                    }
                }),
        )
        .into_element();
    let mut parts = vec![segments];
    if let Some(extras) = Extras::of(snapshot) {
        let e2e = extras.e2e;
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
                            shell_part(app, part, content, e2e, CAPTION),
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
/// `narrow` one healthy runtimes share one segment. Row 2: attention and clock on the left,
/// reach and the source label at the trailing edge.
fn strip(app: AppState, snapshot: &Snapshot, narrow: bool) -> Vec<AnyElement> {
    let segments = if narrow {
        compact_segments(&snapshot.health)
    } else {
        snapshot.health.segments.clone()
    };
    let segments = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_y(px(2.0))
        .children(
            segments
                .iter()
                .enumerate()
                .map(|(at, segment)| shell_segment(segment, true, at == 0)),
        )
        .into_element();
    let mut rows = vec![segments];
    rows.extend(
        Extras::of(snapshot).map(|extras| strip_extras(app, &extras, Extras::presence(snapshot))),
    );
    rows
}

/// The strip's second row. The source label wins the room: attention gives way first, then the
/// clock, then reach.
fn strip_extras(app: AppState, extras: &Extras<'_>, present: [bool; 4]) -> AnyElement {
    let fit = Fit::new();
    let cut = computed(move || {
        let widths = fit.widths.get();
        fit.give_way(|from| strip_needed(present, widths, from))
    });
    let parts = extras.parts();
    let e2e = extras.e2e;
    let item = |part: usize| {
        let order = STRIP_GIVE_WAY.iter().position(|&gives| gives == part);
        parts[part].map(|content| {
            fitted(
                fit,
                part,
                move || order.is_some_and(|order| cut.get() > order),
                shell_part(app, part, content, e2e, STRIP_CAPTION),
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

/// A shell control that opens Settings › Connection: the daemon's segment, the reach. A
/// `filled` face (a failing segment) keeps its fill on hover and only fades a little, as the
/// shell's hover fill would put its dark words on a dark ground.
fn to_connection(
    app: AppState,
    face: View,
    test_id: &str,
    label: String,
    filled: bool,
) -> AnyElement {
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

/// Reach, attention, clock or source on the shell, as [`Extras::parts`] numbers them. The
/// desktop's reach is a button to Settings › Connection (the phone strip is one already), a
/// lock before it on an end-to-end path; attention leads with the changed-since mark.
fn shell_part(app: AppState, part: usize, content: &str, e2e: bool, size: f32) -> AnyElement {
    match part {
        REACH => {
            let reach = view()
                .flex_row()
                .items_center()
                .gap(px(if app.form == Form::Phone { 4.0 } else { 6.0 }))
                .when(e2e, |v| {
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
        _ => shell_source(content, SOURCE_CAPTION),
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

/// The source label as the mock's snapshot tag: an outlined held-tone tag, so fixture data never
/// reads as live. Its words are `size`: a step under the bar's caption, the strip's own size.
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
