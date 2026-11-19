//! The lanes chart and the goal burn-up on canvases, for desktop and phone. Each is measured to
//! its container and laid out by the model at that width, so both follow the window. A canvas has
//! no parts of its own for the pointer or a screen reader, so every segment and action mark also
//! gets a transparent view over its box that carries the model's label (and, on desktop, the
//! hover tooltip the mockup's SVG titles give).
//!
//! Metrics are `css/app.css` "timeline" and the mockup's `timeline()` and `burnup()` drawing code;
//! the four hatches are its `#hatch*` patterns in `js/ui.js`.

use std::f32::consts::{SQRT_2, TAU};

use nsh_core::canvas::CanvasTextAlign;
use nsh_core::prelude::*;

use super::shown::Shown;
use crate::model::Live;
use crate::model::timeline::{self, Burnup, BurnupSeries, Kind, Layout};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::marks::{hue_fill, hue_ink};
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// A segment bar's height.
const BAR: f32 = 12.0;
/// An action dot: the box the mockup centres it in, and its radius.
const DOT: f32 = 10.0;
const DOT_RADIUS: f32 = 4.5;
/// A dropped or reopened instant's mark, at most this far from its centre.
const MARK_RADIUS: f32 = 4.0;
/// A lane button's focus ring.
const RING: f32 = 2.0;
/// Where a desktop lane's title starts, after the mono id.
const TITLE_X: f32 = 52.0;
/// The phone puts the title after the id at the mockup's 7 points a character.
const PHONE_ID_ADVANCE: f32 = 7.0;
/// Estimated advances of the canvas fonts, to keep words inside their room (a canvas cannot
/// measure): 11 pt mono ids, 12 pt titles, 11 pt semibold wait captions, 10.5 pt mono axis.
const ID_ADVANCE: f32 = 6.7;
const TITLE_ADVANCE: f32 = 6.4;
const NOTE_ADVANCE: f32 = 6.3;
const AXIS_ADVANCE: f32 = 6.4;
/// Room kept between a lane's label and the plot, and between two captions.
const LABEL_GAP: f32 = 8.0;
/// Room under the last lane for the now caption.
const FOOT: f32 = 18.0;
/// The legend swatch (`.legend i`).
const SWATCH_WIDTH: f32 = 18.0;
const SWATCH_HEIGHT: f32 = 8.0;
/// The burn-up's height (the mockup's 404×150 desktop, 354×140 phone).
const BURNUP_HEIGHT: f32 = 150.0;
const BURNUP_HEIGHT_PHONE: f32 = 140.0;
/// The mockup mixes the corral colour 55 % with ink-3 for review; ink-3 laid over the corral
/// colour at 45 % is that blend.
const REVIEW_MIX: f32 = 0.45;
/// The idle hatch's lines are ink-4 at 55 %.
const IDLE_ALPHA: f32 = 0.55;

/// The lanes chart: hour ticks and grid, the owner and automatic action lanes, one lane per task
/// or run with its segments drawn by kind (solid run, gate, review, land; hatched idle and
/// blocked; outlined awaiting acceptance), a lane's long waits after its bars, and the now line
/// with its caption. It fills its container's width
/// and lays out again when that or the plot changes; desktop keeps the 190 pt label column, the
/// phone puts labels above the bars and labels every other tick. Screen readers hear the plot's
/// description and then each segment's own label.
pub fn lanes(app: AppState, data: Memo<Shown>) -> AnyElement {
    let form = app.form;
    let phone = form == Form::Phone;
    let width = signal(0.0_f32);
    let plot = computed(move || data.with(|shown| shown.plot.clone()));
    view()
        .w_full()
        .on_layout(move |frame: &Bounds<Points>| width.set(frame.size.width.0))
        .child(dynamic(move || {
            let room = width.get();
            let plot = plot.get()?;
            (room > 0.0).then(|| drawn_lanes(app, timeline::layout(&plot, room, phone)))
        }))
        .into_element()
}

fn drawn_lanes(app: AppState, layout: Layout) -> AnyElement {
    let phone = app.form == Form::Phone;
    let (width, height) = (layout.width, layout.height);
    let spoken = layout.accessible_label.clone();
    let boxes = hit_boxes(app, &layout);
    view()
        .relative()
        .w(px(width))
        .h(px(height))
        .child(
            canvas(move || paint_lanes(&layout, phone))
                .absolute()
                .top(px(0.0))
                .left(px(0.0))
                .w(px(width))
                .h(px(height))
                .role(AccessibilityRole::Image)
                .label(spoken),
        )
        .children(boxes)
        .into_element()
}

/// The legend's sample of a kind (18 by 8 pt, 2 pt corners), drawn as the chart draws its bars.
/// Hidden from screen readers: the label beside it names the kind.
pub fn swatch(kind: Kind) -> AnyElement {
    canvas(move || {
        bar(
            Painter::new(),
            kind,
            Rect::new(0.0, 0.0, SWATCH_WIDTH, SWATCH_HEIGHT),
            2.0,
        )
    })
    .w(px(SWATCH_WIDTH))
    .h(px(SWATCH_HEIGHT))
    .flex_shrink_0()
    .accessibility_elements_hidden(true)
    .into_element()
}

/// The goal burn-up (R23): the round grid, scope dashed in ink-3, landed solid in ink, the two end
/// labels and the first and last day. Its container's width, 150 pt high (140 on the phone);
/// screen readers hear the model's summary.
pub fn burnup(app: AppState, series: BurnupSeries) -> AnyElement {
    let height = if app.form == Form::Phone {
        BURNUP_HEIGHT_PHONE
    } else {
        BURNUP_HEIGHT
    };
    let width = signal(0.0_f32);
    view()
        .w_full()
        .mt(px(6.0))
        .on_layout(move |frame: &Bounds<Points>| width.set(frame.size.width.0))
        .child(dynamic(move || {
            let room = width.get();
            (room > 0.0).then(|| {
                let chart = timeline::burnup(&series, room, height);
                let spoken = chart.accessible_label.clone();
                canvas(move || paint_burnup(&chart))
                    .w(px(room))
                    .h(px(height))
                    .role(AccessibilityRole::Image)
                    .label(spoken)
                    .into_element()
            })
        }))
        .into_element()
}

#[derive(Clone, Copy)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Rect {
    const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    fn bounds(self) -> Bounds<Points> {
        bounds(point(px(self.x), px(self.y)), size(px(self.w), px(self.h)))
    }
}

/// The colour of a theme colour in the appearance shown now. Reading it inside a canvas's
/// drawing redraws the canvas when the appearance changes.
fn now(adaptive: AdaptiveColor) -> Hsla {
    if dark() {
        adaptive.dark
    } else {
        adaptive.light
    }
}

fn ink(token: Token) -> Hsla {
    now(color(token))
}

fn mono_font(px_size: f32) -> String {
    format!("{px_size}px {FONT_MONO}")
}

fn text_font(weight: u16, px_size: f32) -> String {
    format!("{weight} {px_size}px {FONT_TEXT}")
}

/// Top of a bar `height` high in the lane starting at `lane_y`: centred in the lane on desktop,
/// under the label line on the phone.
fn bar_top(layout: &Layout, phone: bool, lane_y: f32, height: f32) -> f32 {
    if phone {
        lane_y + 18.0
    } else {
        lane_y + (layout.lane_height - height) / 2.0
    }
}

fn auto_y(layout: &Layout) -> f32 {
    layout.top + layout.lane_height
}

/// A segment's bar box.
fn segment_rect(top: f32, segment: &timeline::PlacedSegment) -> Rect {
    Rect::new(segment.x, top, segment.width, BAR)
}

/// One bar in the look of its kind: solid for run, gate, review (the mixed corral) and land;
/// hatched for idle (faint lines, no ground) and blocked (lines in the state colour on its quiet
/// ground, leaning the other way); outlined on its quiet ground for awaiting acceptance; a cross
/// for dropped and a ring for reopened, centred on the instant.
fn bar(p: Painter, kind: Kind, area: Rect, radius: f32) -> Painter {
    let hue = kind.hue();
    let (cx, cy) = (area.x + area.w / 2.0, area.y + area.h / 2.0);
    let r = (area.h / 2.0).min(MARK_RADIUS);
    match kind {
        Kind::Run | Kind::Gate | Kind::Land => solid(p, area, radius, now(hue_ink(hue))),
        Kind::Drop => p
            .stroke_color(now(hue_ink(hue)))
            .line_width(1.6)
            .begin_path()
            .move_to(cx - r, cy - r)
            .line_to(cx + r, cy + r)
            .move_to(cx + r, cy - r)
            .line_to(cx - r, cy + r)
            .stroke(),
        Kind::Reopen => p
            .stroke_color(now(hue_ink(hue)))
            .line_width(1.6)
            .begin_path()
            .arc(cx, cy, r, 0.0, TAU)
            .stroke(),
        Kind::Review => solid(p, area, radius, now(hue_ink(hue)))
            .global_alpha(REVIEW_MIX)
            .fill_color(ink(Token::Ink3))
            .fill()
            .global_alpha(1.0),
        Kind::Idle => hatch(
            p,
            area,
            radius,
            Stripes {
                ground: None,
                line: ink(Token::Ink4).opacity(IDLE_ALPHA),
                width: 1.4,
                pitch: 6.0,
                rising: true,
            },
        ),
        Kind::Ask => hatch(
            p,
            area,
            radius,
            Stripes {
                ground: Some(now(hue_fill(hue))),
                line: now(hue_ink(hue)),
                width: 1.4,
                pitch: 6.0,
                rising: false,
            },
        ),
        Kind::Held => solid(p, area, radius, now(hue_fill(hue)))
            .stroke_color(now(hue_ink(hue)))
            .line_width(1.2)
            .begin_path()
            .round_rect(area.bounds(), px(radius))
            .stroke(),
    }
}

fn solid(p: Painter, area: Rect, radius: f32, fill: Hsla) -> Painter {
    p.fill_color(fill)
        .begin_path()
        .round_rect(area.bounds(), px(radius))
        .fill()
}

/// A hatch: diagonal lines `pitch` apart across the box, over an optional ground.
struct Stripes {
    ground: Option<Hsla>,
    line: Hsla,
    width: f32,
    pitch: f32,
    /// `/` (the mockup's 45° patterns) rather than `\` (its -45° waiting pattern).
    rising: bool,
}

fn hatch(p: Painter, area: Rect, radius: f32, stripes: Stripes) -> Painter {
    let Rect { x, y, w, h } = area;
    let mut p = p
        .save()
        .begin_path()
        .round_rect(area.bounds(), px(radius))
        .clip();
    if let Some(ground) = stripes.ground {
        p = p.fill_color(ground).fill_rect(x, y, w, h);
    }
    p = p
        .stroke_color(stripes.line)
        .line_width(stripes.width)
        .begin_path();
    // The stripes keep to one grid over the whole canvas, as an SVG pattern in user space does,
    // so the hatch of neighbouring bars lines up.
    let step = stripes.pitch * SQRT_2;
    let offset = if stripes.rising { y + h } else { -y };
    let mut c = ((x - h + offset) / step).floor() * step - offset;
    while c <= x + w {
        p = if stripes.rising {
            p.move_to(c, y + h).line_to(c + h, y)
        } else {
            p.move_to(c, y).line_to(c + h, y + h)
        };
        c += step;
    }
    p.stroke().restore()
}

/// A lane's mono id in ink-3 and its title in ink: beside each other in the desktop's label
/// column, on one line above the bar on the phone.
/// `words` cut with an ellipsis to fit `room` points at `advance` points a character.
fn clip(words: &str, room: f32, advance: f32) -> String {
    let fits = (room / advance).floor().max(0.0) as usize;
    if words.chars().count() <= fits {
        return words.to_owned();
    }
    let mut kept: String = words.chars().take(fits.saturating_sub(1)).collect();
    kept.push('…');
    kept
}

/// A lane label's id and title, each cut to its room, with where the title starts: after the
/// id (at least the mockup's 52 pt on desktop), inside the label column on desktop and the
/// canvas on the phone, short of the `reserve` points kept at the line's end.
fn label_parts(
    layout: &Layout,
    phone: bool,
    (id, title): (&str, &str),
    reserve: f32,
) -> (String, String, f32) {
    let right = if phone {
        layout.width - reserve
    } else {
        layout.label_width - LABEL_GAP
    };
    let id = clip(id, right, if phone { PHONE_ID_ADVANCE } else { ID_ADVANCE });
    let id_end = id.chars().count() as f32 * if phone { PHONE_ID_ADVANCE } else { ID_ADVANCE };
    let title_x = if phone {
        id_end + LABEL_GAP
    } else {
        TITLE_X.max(id_end + LABEL_GAP)
    };
    (id, clip(title, right - title_x, TITLE_ADVANCE), title_x)
}

/// Whether a lane label is cut to fit, so its full words need a hover hint.
fn label_cut(layout: &Layout, phone: bool, words: (&str, &str), reserve: f32) -> bool {
    let (id, title, _) = label_parts(layout, phone, words, reserve);
    id != words.0 || title != words.1
}

/// The phone's room at the end of a lane's label line for its wait caption: the caption's
/// width, never more than 45 % of the line, so the title keeps the rest.
fn wait_reserve(layout: &Layout, phone: bool, lane: &timeline::PlacedLane) -> f32 {
    match (&lane.wait, phone) {
        (Some(words), true) => {
            (words.chars().count() as f32 * NOTE_ADVANCE + LABEL_GAP).min(layout.width * 0.45)
        }
        _ => 0.0,
    }
}

/// The baseline of a lane's label line.
fn label_baseline(layout: &Layout, phone: bool, lane_y: f32) -> f32 {
    if phone {
        lane_y + 11.0
    } else {
        lane_y + layout.lane_height / 2.0 + 4.0
    }
}

fn lane_label(
    p: Painter,
    layout: &Layout,
    phone: bool,
    lane_y: f32,
    words: (&str, &str),
    reserve: f32,
) -> Painter {
    let (id, title, title_x) = label_parts(layout, phone, words, reserve);
    let baseline = label_baseline(layout, phone, lane_y);
    p.text_align(CanvasTextAlign::Start)
        .font(&mono_font(11.0))
        .fill_color(ink(Token::Ink3))
        .fill_text(&id, 0.0, baseline)
        .font(&text_font(400, 12.0))
        .fill_color(ink(Token::Ink))
        .fill_text(&title, title_x, baseline)
}

/// Where a lane's words after its bars start (desktop), and the room left to them.
fn trail_start(layout: &Layout, lane: &timeline::PlacedLane) -> (f32, f32) {
    let x = lane_end(lane).map_or(layout.label_width, |end| end + 6.0);
    (x, (layout.width - x).max(0.0))
}

fn paint_lanes(layout: &Layout, phone: bool) -> Painter {
    let axis = mono_font(10.5);
    let note = text_font(600, 11.0);
    let quiet = ink(Token::Ink3);
    let foot = layout.height - FOOT;

    let mut p = Painter::new()
        .stroke_color(ink(Token::Rule))
        .line_width(1.0)
        .begin_path();
    for tick in &layout.ticks {
        p = p
            .move_to(tick.x, layout.top - 6.0)
            .line_to(tick.x, layout.height);
    }
    p = p
        .stroke()
        .font(&axis)
        .fill_color(quiet)
        .text_align(CanvasTextAlign::Center);
    for tick in layout.ticks.iter().filter(|tick| !tick.label.is_empty()) {
        // Centred on its tick, but kept on the canvas: the phone's first day sits at x = 0.
        let half = tick.label.chars().count() as f32 * AXIS_ADVANCE / 2.0;
        let x = tick.x.min(layout.width - half).max(half);
        p = p.fill_text(&tick.label, x, 14.0);
    }

    p = lane_label(p, layout, phone, layout.top, layout.owner_label, 0.0);
    p = dots(
        p,
        layout,
        phone,
        layout.top,
        &layout.owner,
        now(hue_ink(Hue::You)),
    );
    let auto_y = auto_y(layout);
    p = lane_label(p, layout, phone, auto_y, layout.auto_label, 0.0);
    p = dots(p, layout, phone, auto_y, &layout.auto, ink(Token::Ink3));

    for lane in &layout.lanes {
        let reserve = wait_reserve(layout, phone, lane);
        p = lane_label(
            p,
            layout,
            phone,
            lane.y,
            (lane.id.as_str(), lane.title.as_str()),
            reserve,
        );
        let top = bar_top(layout, phone, lane.y, BAR);
        // A lane's long waits belong to its row (TT-27): at the end of the phone's label line,
        // after the desktop's last bar.
        if phone && let Some(words) = &lane.wait {
            let words = clip(words, reserve - LABEL_GAP, NOTE_ADVANCE);
            p = p
                .font(&note)
                .fill_color(now(hue_ink(Hue::Held)))
                .text_align(CanvasTextAlign::End)
                .fill_text(
                    &words,
                    layout.width - 2.0,
                    label_baseline(layout, phone, lane.y),
                );
        }
        if !phone && let Some(words) = &lane.wait {
            let (x, room) = trail_start(layout, lane);
            let words = clip(words, room, NOTE_ADVANCE);
            p = p
                .font(&note)
                .fill_color(now(hue_ink(Hue::Held)))
                .text_align(CanvasTextAlign::Start)
                .fill_text(&words, x, top + BAR - 2.0);
        }
        for segment in &lane.segments {
            p = bar(p, segment.kind, segment_rect(top, segment), 2.0);
        }
    }

    let strong = ink(Token::Ink);
    let current = &layout.now;
    p.stroke_color(strong)
        .line_width(1.2)
        .begin_path()
        .move_to(current.x, layout.top - 4.0)
        .line_to(current.x, foot)
        .stroke()
        .font(&axis)
        .fill_color(strong)
        .text_align(CanvasTextAlign::End)
        .fill_text(&current.label, current.x - 2.0, foot + 12.0)
}

/// Where a lane's last drawn bar ends, if it has one.
fn lane_end(lane: &timeline::PlacedLane) -> Option<f32> {
    lane.segments
        .iter()
        .map(|segment| segment.x + segment.width)
        .reduce(f32::max)
}

/// An action lane's dots, centred where a bar would be.
fn dots(
    p: Painter,
    layout: &Layout,
    phone: bool,
    lane_y: f32,
    markers: &[timeline::PlacedMarker],
    fill: Hsla,
) -> Painter {
    let y = bar_top(layout, phone, lane_y, DOT) + DOT / 2.0;
    markers.iter().fold(p.fill_color(fill), |p, marker| {
        p.begin_path().arc(marker.x, y, DOT_RADIUS, 0.0, TAU).fill()
    })
}

/// Opens what a lane opens: its task, or a live run's session terminal.
fn open(app: AppState, live: &Live) {
    let live = live.clone();
    app.act(app.timeline, move |state, _| state.open(live));
}

/// The transparent boxes over the action dots, each lane's row and every segment. A lane's row
/// box (over its label on desktop, the whole lane on the phone) is the lane's button and stop of
/// the keyboard focus; its segments and wait take the pointer's press to the same place without
/// being stops of their own.
fn hit_boxes(app: AppState, layout: &Layout) -> Vec<AnyElement> {
    let form = app.form;
    let phone = form == Form::Phone;
    let mut boxes = Vec::new();
    for (lane_y, markers) in [(layout.top, &layout.owner), (auto_y(layout), &layout.auto)] {
        let dot_top = bar_top(layout, phone, lane_y, DOT);
        for marker in markers {
            boxes.push(hit(
                form,
                Rect::new(marker.x - DOT / 2.0, dot_top, DOT, DOT),
                &marker.accessible_label,
                None,
            ));
        }
    }
    for lane in &layout.lanes {
        let top = bar_top(layout, phone, lane.y, BAR);
        let words = (lane.id.as_str(), lane.title.as_str());
        let reserve = wait_reserve(layout, phone, lane);
        let row = if phone {
            Rect::new(0.0, lane.y, layout.width, layout.lane_height)
        } else {
            Rect::new(
                0.0,
                lane.y,
                layout.label_width - LABEL_GAP,
                layout.lane_height,
            )
        };
        let cut = form == Form::Desktop && label_cut(layout, phone, words, reserve);
        boxes.push(row_button(app, row, lane, cut));
        let press = Some((app, &lane.opens.live));
        for segment in &lane.segments {
            let area = segment_rect(top, segment);
            // An instant is a 2 pt mark; its box is a fingertip wide around it.
            let area = if area.w <= 2.0 {
                Rect::new(area.x + area.w / 2.0 - DOT / 2.0, area.y, DOT, area.h)
            } else {
                area
            };
            boxes.push(hit(form, area, &segment.accessible_label, press));
        }
        // The row's wait in full, wherever it was cut to fit.
        if let Some(wait) = &lane.wait {
            let area = if phone {
                Rect::new(layout.width - reserve, lane.y, reserve, 14.0)
            } else {
                let (x, room) = trail_start(layout, lane);
                Rect::new(x, top, room, BAR)
            };
            boxes.push(hit(form, area, &format!("{} {wait}", lane.id), press));
        }
    }
    boxes
}

/// A lane's button (`lane-<id>`): named by what it opens and the lane, the full label as its
/// hover hint where the canvas cut it, the focus ring while the keyboard is on it.
fn row_button(app: AppState, area: Rect, lane: &timeline::PlacedLane, cut: bool) -> AnyElement {
    let form = app.form;
    let live = lane.opens.live.clone();
    let hint = if cut {
        format!("{} {} · {}", lane.id, lane.title, lane.opens.label)
    } else {
        lane.opens.label.clone()
    };
    let spoken = format!("{}: {} {}", lane.opens.label, lane.id, lane.title);
    view()
        .absolute()
        .left(px(area.x))
        .top(px(area.y))
        .w(px(area.w))
        .h(px(area.h))
        .test_id(&format!("lane-{}", lane.id))
        .cursor_pointer()
        .rounded(px(4.0))
        .role(AccessibilityRole::Button)
        .label(spoken)
        .on_press(move || open(app, &live))
        .focus_visible(|s| {
            s.outline(px(RING))
                .outline_color(color(Token::Focus))
                .outline_offset(px(-RING))
        })
        .when(form == Form::Desktop, |v| v.tooltip(tip(hint.into())))
        .into_element()
}

/// A box over a mark that a screen reader names and the desktop hints; with `press`, the
/// pointer's press opens what its lane opens.
fn hit(form: Form, area: Rect, spoken: &str, press: Option<(AppState, &Live)>) -> AnyElement {
    let spoken = SharedString::from(spoken.to_owned());
    let press = press.map(|(app, live)| (app, live.clone()));
    view()
        .absolute()
        .left(px(area.x))
        .top(px(area.y))
        .w(px(area.w))
        .h(px(area.h))
        .accessible(true)
        .role(AccessibilityRole::Image)
        .label(spoken.clone())
        .when(form == Form::Desktop, |v| v.tooltip(tip(spoken)))
        .when_some(press, |v, (app, live)| {
            v.cursor_pointer()
                .focusable(false)
                .on_press(move || open(app, &live))
        })
        .into_element()
}

/// The hover hint: the label on the raised surface in 11.5 pt ink.
fn tip(words: SharedString) -> AnyElement {
    view()
        .px(px(8.0))
        .py(px(4.0))
        .rounded(px(6.0))
        .bg(color(Token::Surface))
        .border_1()
        .border_color(color(Token::Rule2))
        .child(
            text(words)
                .font_family(FONT_TEXT)
                .text_size(px(11.5))
                .text_color(color(Token::Ink)),
        )
        .into_element()
}

fn polyline(p: Painter, points: &[timeline::Point]) -> Painter {
    points
        .iter()
        .enumerate()
        .fold(p.begin_path(), |p, (i, at)| {
            if i == 0 {
                p.move_to(at.x, at.y)
            } else {
                p.line_to(at.x, at.y)
            }
        })
        .stroke()
}

fn paint_burnup(chart: &Burnup) -> Painter {
    let axis = mono_font(10.5);
    let quiet = ink(Token::Ink3);
    let [first_day, last_day] = &chart.axis_labels;
    let left = first_day.1;
    let right = last_day.1;
    let mut p = Painter::new()
        .stroke_color(ink(Token::Rule))
        .line_width(1.0)
        .font(&mono_font(9.5))
        .fill_color(ink(Token::Ink4))
        .text_align(CanvasTextAlign::Start);
    for &(value, y) in &chart.grid {
        p = p
            .begin_path()
            .move_to(left, y)
            .line_to(right, y)
            .stroke()
            .fill_text(&value.to_string(), 0.0, y + 3.0);
    }
    p = p
        .stroke_color(quiet)
        .line_width(1.4)
        .line_dash(&[px(4.0), px(3.0)]);
    p = polyline(p, &chart.scope);
    p = p
        .line_dash(&[])
        .stroke_color(ink(Token::Ink))
        .line_width(1.8);
    p = polyline(p, &chart.landed);
    p = p
        .font(&axis)
        .fill_color(quiet)
        .text_align(CanvasTextAlign::End);
    for (words, x, y) in &chart.series_labels {
        p = p.fill_text(words, *x, *y);
    }
    p.text_align(CanvasTextAlign::Start)
        .fill_text(&first_day.0, first_day.1, first_day.2)
        .text_align(CanvasTextAlign::End)
        .fill_text(&last_day.0, last_day.1, last_day.2)
}
