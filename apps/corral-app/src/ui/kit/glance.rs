//! Facts read at a glance (`model::glance`): a count as an icon and a number, the page head
//! that leads with them, the loop band, a task's rail with its stage named, and a session's
//! activity. Each shows little and says the rest in its hint (`kit::hint`); the model and the
//! caller supply every word and number.
//!
//! Two kinds of placement. A standalone fact (a page header's counts, the activity chart in a
//! panel) is a stop of the keyboard focus on desktop and in a terminal, so its hint reaches the
//! keyboard too. A fact inside a pressable row or cell (`stat_in`, the rail, the activity strip)
//! is no stop of its own: the row is, and the row's label speaks for it.

use std::rc::Rc;

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::board::rail_label;
use crate::model::glance::{Activity, LoopBand, LoopStage, StageRail, Stat};
use crate::model::hue::Hue;
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::hint::{hint_on, tip};
use crate::ui::kit::marks::{hue_ink, mark};
use crate::ui::kit::notice;
use crate::ui::kit::page::{Title, actions_row, phone_head, spacer, title_text};
use crate::ui::term::{self, cells};
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color, text_size};

const RING: f32 = 2.0;
/// The loop band's cell (`.loop a`): 36 pt high, 14 pt in.
const CELL_HEIGHT: f32 = 36.0;
const RADIUS: f32 = 8.0;
/// The chevron between two cells, a rotated square's two edges (`.loop a+a::before`).
const CHEVRON: f32 = 8.0;
/// What a stage with no counts shows (`.loop .nil`).
const NIL: &str = "–";

/// The size a count is drawn at: in a row or cell (`.tok`) or in a page header (`.stats .tok`).
#[derive(Clone, Copy)]
enum Scale {
    Token,
    Header,
}

impl Scale {
    /// The icon's and the figures' size, in points.
    fn metrics(self, form: Form) -> (f32, f32) {
        match (self, form) {
            (Scale::Token, Form::Phone) => (15.0, 14.0),
            (Scale::Header, Form::Phone) => (16.0, 15.0),
            (Scale::Token, _) => (14.0, 12.5),
            (Scale::Header, _) => (15.0, 14.0),
        }
    }
}

/// The colour of a count's icon and figures: the hue's; a neutral count is the mock's quiet
/// `dim` ink-3, full ink in a terminal, where ink-3 would read as disabled.
fn stat_ink(form: Form, hue: Hue) -> AdaptiveColor {
    match (hue, form) {
        (Hue::Neutral, Form::Terminal) => color(Token::Ink),
        (Hue::Neutral, _) => color(Token::Ink3),
        _ => hue_ink(hue),
    }
}

/// The icon and the figures of `stat`, nothing around them.
fn face(form: Form, stat: &Stat, scale: Scale) -> View {
    let ink = stat_ink(form, stat.hue);
    if form == Form::Terminal {
        return view().flex_row().flex_shrink_0().child(
            text("")
                .number_of_lines(1u32)
                .child(text(stat.icon.glyph().to_string()).text_color(ink))
                .child(text(stat.value.clone()).text_color(ink)),
        );
    }
    let (glyph, figures) = scale.metrics(form);
    view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .gap(px(4.0))
        .child(icon(Icon::stat(stat.icon), glyph, ink))
        .child(
            text(stat.value.clone())
                .number_of_lines(1u32)
                .font_family(FONT_MONO)
                .tabular_nums()
                .text_size(px(figures))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ink),
        )
}

/// A standalone count at `scale`: its own focus stop (desktop, terminal) whose label is the
/// hint, with the hint as its tooltip.
fn standalone(app: AppState, stat: &Stat, scale: Scale) -> AnyElement {
    let form = app.form;
    let count = face(form, stat, scale)
        .test_id(&format!("stat-{}", stat.key))
        .accessible(true)
        .role(AccessibilityRole::Text)
        .label(stat.label().to_owned())
        .when(form != Form::Phone, |count| count.focusable(true))
        .when(form == Form::Desktop, |count| {
            count.rounded(px(4.0)).focus_visible(|s| {
                s.outline(px(RING))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(1.0))
            })
        });
    tip(form, count, stat.hint.clone().into()).into_element()
}

/// One count (`.tok`): the icon in the count's hue and the figures alone in tabular mono beside
/// it, the sentence in its hint. Desktop 14 pt icon, 12.5 pt semibold figures, 4 pt apart;
/// phone 15 pt and 14 pt with no tooltip; terminal [`StatIcon::glyph`] then the figures, both in
/// the hue. A stop of the keyboard focus on desktop and in a terminal, so the hint shows for
/// the keyboard too; screen readers hear [`Stat::label`]. test id `stat-<key>`. Inside a
/// pressable row or cell use [`stat_in`].
///
/// [`StatIcon::glyph`]: crate::model::glance::StatIcon::glyph
pub fn stat(app: AppState, stat: &Stat) -> AnyElement {
    standalone(app, stat, Scale::Token)
}

/// A count inside a pressable row or cell, whose own label says what the count says: drawn as
/// [`stat`], its hint on pointer rest (desktop) or on the status line (terminal), no focus stop
/// of its own and hidden from screen readers. test id `stat-<key>`.
pub fn stat_in(app: AppState, stat: &Stat) -> AnyElement {
    quiet(app, stat, format!("stat-{}", stat.key))
}

/// [`stat_in`] under its own test id.
fn quiet(app: AppState, stat: &Stat, test_id: String) -> AnyElement {
    let form = app.form;
    let count = face(form, stat, Scale::Token)
        .test_id(&test_id)
        .accessibility_elements_hidden(true);
    tip(form, count, stat.hint.clone().into()).into_element()
}

/// A page header's counts (`.stats`): [`stat`]s 16 pt apart (a terminal: two Cells), at the
/// header's size (desktop 15 pt icons, 14 pt figures; phone 16 and 15), wrapping onto further
/// lines rather than running out of a narrow header. An empty slice renders an empty element.
pub fn stats(app: AppState, stats: &[Stat]) -> AnyElement {
    if stats.is_empty() {
        return view().into_element();
    }
    stats_row(app, stats, true).into_element()
}

/// The header's counts in a row that wraps, or, in a head's single row, gives way: it shrinks
/// and clips its last counts before the title or the actions go. The 3 pt inset keeps the
/// focus ring inside the clip.
fn stats_row(app: AppState, stats: &[Stat], wraps: bool) -> View {
    let form = app.form;
    view()
        .flex_row()
        .items_center()
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .when(wraps, |row| row.flex_wrap())
        .when(!wraps, |row| row.overflow_hidden())
        .when(!wraps && form != Form::Terminal, |row| row.p(px(3.0)))
        .gap(match form {
            Form::Terminal => cells(2.0),
            Form::Desktop | Form::Phone => px(16.0),
        })
        .children(
            stats
                .iter()
                .map(|stat| standalone(app, stat, Scale::Header))
                .collect::<Vec<_>>(),
        )
}

/// The page head of the main loop's pages (`.phead`): the title, its counts ([`stats`]) right
/// after it, the actions at the trailing edge. Built as `page::head`, with the counts in place
/// of its sub line. Desktop: one 46 pt row under a rule, 18 pt in, title 15 pt bold, the counts
/// 16 pt after it giving way (clipped) before the title is cut. Phone: the large title with the
/// actions at its trailing edge as `page::head` draws it, the counts wrapping on a line of their
/// own under it, then the connection banner. Terminal: one row, the title bold and never cut,
/// the counts two Cells after it giving way, the actions at the end, a joined rule under it.
/// No counts: the head without them. The title carries test id "page-title" and the header
/// role.
pub fn page_head(
    app: AppState,
    title: impl Into<SharedString>,
    stats: &[Stat],
    actions: Vec<AnyElement>,
) -> AnyElement {
    let form = app.form;
    let title: SharedString = title.into();
    let counts = (!stats.is_empty()).then(|| stats_row(app, stats, form == Form::Phone));
    match form {
        Form::Desktop => view()
            .flex_row()
            .flex_none()
            .items_center()
            .gap(px(16.0))
            .h(px(46.0))
            .px(px(18.0))
            .border_b(px(1.0))
            .border_color(color(Token::Rule))
            .child(title_text(form, title, "page-title", Title::Page))
            .children(counts)
            .child(spacer())
            .child(actions_row(form, actions, 8.0))
            .into_element(),
        Form::Phone => view()
            .flex_none()
            .child(phone_head(title, actions))
            .children(counts.map(|counts| {
                view()
                    .flex_row()
                    .px(px(18.0))
                    .pb(px(10.0))
                    .bg(color(Token::Bg))
                    // The line's width, not the wrapping row's own guess at its content (sized
                    // by its first counts), decides where it wraps.
                    .child(counts.flex_1())
            }))
            .child(notice::connection(app))
            .into_element(),
        Form::Terminal => view()
            .flex_none()
            .child(
                view()
                    .flex_row()
                    .items_center()
                    .h(term::rows(1.0))
                    .px(cells(1.0))
                    .child(title_text(form, title, "page-title", Title::Page))
                    .children(counts.map(|counts| {
                        view()
                            .flex_row()
                            .flex_shrink(1.0)
                            .min_w(px(0.0))
                            .child(term::space(2.0))
                            .child(counts)
                    }))
                    .child(spacer())
                    .child(actions_row(form, actions, 2.0)),
            )
            .child(term::hrule())
            .into_element(),
    }
}

/// Plan, run, gate, review, land with what each holds (`.loop`). Desktop: one band with a
/// rule-1 edge, 8 pt corners and the surface fill, its five cells of equal width 36 pt high,
/// each the stage's name in semibold on the left and its counts ([`stat_in`]) on the right, a
/// faint dash when it holds none; a chevron of rule-2 joins one cell to the next. A cell whose
/// stage has a destination is a button calling `on_stage`; every cell is a focus stop with the
/// stage's hint. The band sets no margin: the screen places it. Phone: not drawn (an empty
/// element), as the mock hides it at 640 pt and under. Terminal: one line, `Plan ●1 ◐4 › Run
/// ●1 ◐2 › …`, each stage pressable as on desktop. test id `loop-<key>` on each cell and
/// `loop-<key>-stat-<stat key>` on its counts.
pub fn loop_band(
    app: AppState,
    band: &LoopBand,
    on_stage: impl Fn(&LoopStage) + 'static,
) -> AnyElement {
    let form = app.form;
    if form == Form::Phone || band.stages.is_empty() {
        return view().into_element();
    }
    let on_stage: Rc<dyn Fn(&LoopStage)> = Rc::new(on_stage);
    let last = band.stages.len() - 1;
    let cells = band
        .stages
        .iter()
        .enumerate()
        .map(|(index, stage)| {
            let place = Place {
                first: index == 0,
                last: index == last,
            };
            loop_cell(app, stage, place, on_stage.clone())
        })
        .collect::<Vec<_>>();
    let band_view = view()
        .flex_row()
        .items_center()
        .role(AccessibilityRole::Navigation)
        .label(LoopBand::LABEL);
    match form {
        Form::Terminal => band_view
            .flex_wrap()
            .children(interleave(cells, || {
                text(" › ")
                    .number_of_lines(1u32)
                    .flex_shrink_0()
                    .text_color(color(Token::Ink3))
                    .accessibility_elements_hidden(true)
                    .into_element()
            }))
            .into_element(),
        Form::Desktop | Form::Phone => band_view
            .border_1()
            .border_color(color(Token::Rule))
            .rounded(px(RADIUS))
            .bg(color(Token::Surface))
            .children(cells)
            .into_element(),
    }
}

/// Where a cell sits in the band: the first has no edge or chevron before it, the first and
/// last round the band's corners.
#[derive(Clone, Copy)]
struct Place {
    first: bool,
    last: bool,
}

/// `items` with a `separator` between each two.
fn interleave(items: Vec<AnyElement>, separator: impl Fn() -> AnyElement) -> Vec<AnyElement> {
    let mut out = Vec::with_capacity(items.len() * 2);
    for (index, item) in items.into_iter().enumerate() {
        if index > 0 {
            out.push(separator());
        }
        out.push(item);
    }
    out
}

fn loop_cell(
    app: AppState,
    stage: &LoopStage,
    place: Place,
    on_stage: Rc<dyn Fn(&LoopStage)>,
) -> AnyElement {
    let form = app.form;
    let pressable = stage.to.is_some();
    let owned = stage.clone();
    let cell = view()
        .flex_row()
        .items_center()
        .test_id(&format!("loop-{}", stage.key))
        .accessible(true)
        .label(stage.spoken())
        .when(pressable, |cell| {
            cell.role(AccessibilityRole::Button)
                .cursor_pointer()
                .on_press(move || on_stage(&owned))
        })
        .when(!pressable, |cell| cell.focusable(true));
    let cell = match form {
        Form::Terminal => cell.flex_shrink_0().child(terminal_cell(stage)),
        Form::Desktop | Form::Phone => desktop_cell(app, cell, stage, place, pressable),
    };
    hint_on(app, cell, stage.hint.clone()).into_element()
}

/// A terminal cell's line: the name in bold, then each count's glyph and figures in its hue, or
/// the faint dash.
fn terminal_cell(stage: &LoopStage) -> Text {
    let line = text("").number_of_lines(1u32).child(
        text(stage.label)
            .font_weight(FontWeight::BOLD)
            .text_color(color(Token::Ink)),
    );
    if stage.stats.is_empty() {
        return line.child(text(format!(" {NIL}")).text_color(color(Token::Ink3)));
    }
    stage.stats.iter().fold(line, |line, stat| {
        let ink = stat_ink(Form::Terminal, stat.hue);
        line.child(" ")
            .child(text(stat.icon.glyph().to_string()).text_color(ink))
            .child(text(stat.value.clone()).text_color(ink))
    })
}

fn desktop_cell(
    app: AppState,
    cell: View,
    stage: &LoopStage,
    place: Place,
    pressable: bool,
) -> View {
    let form = app.form;
    let counts: Vec<AnyElement> = if stage.stats.is_empty() {
        vec![
            text(NIL)
                .number_of_lines(1u32)
                .flex_shrink_0()
                .font_family(FONT_TEXT)
                .text_size(px(text_size(form)))
                .text_color(color(Token::Ink4))
                .into_element(),
        ]
    } else {
        // Scoped by stage: the page header may count the same key.
        stage
            .stats
            .iter()
            .map(|stat| quiet(app, stat, format!("loop-{}-stat-{}", stage.key, stat.key)))
            .collect()
    };
    let chevron = (!place.first).then(|| {
        view()
            .absolute()
            .left(px(-(CHEVRON / 2.0 + 1.0)))
            .top(px((CELL_HEIGHT - CHEVRON) / 2.0))
            .size(px(CHEVRON))
            .border_t_1()
            .border_r_1()
            .border_color(color(Token::Rule2))
            .bg(color(Token::Surface))
            .rotate(deg(45.0))
            .pointer_events(PointerEvents::None)
            .accessibility_elements_hidden(true)
    });
    cell.relative()
        .flex_1()
        .min_w(px(0.0))
        .h(px(CELL_HEIGHT))
        .px(px(14.0))
        .gap(px(10.0))
        .when(!place.first, |cell| {
            cell.border_l_1().border_l_color(color(Token::Rule))
        })
        .when(place.first, |cell| cell.rounded_l(px(RADIUS)))
        .when(place.last, |cell| cell.rounded_r(px(RADIUS)))
        .when(pressable, |cell| {
            cell.hover(|s| s.bg(color(Token::Hover)))
                .active(|s| s.bg(color(Token::Sel)))
        })
        .focus_visible(|s| {
            s.outline(px(RING))
                .outline_color(color(Token::Focus))
                .outline_offset(px(-RING))
        })
        .children(chevron)
        .child(
            text(stage.label)
                .number_of_lines(1u32)
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .font_family(FONT_TEXT)
                .text_size(px(text_size(form)))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(Token::Ink)),
        )
        .child(view().flex_1().min_w(px(0.0)))
        .children(counts)
}

/// A task's rail with its current stage named (`.trk`): the four marks ([`mark`]) joined by
/// 8 pt rules of rule-2, 3 pt apart, the current stage's label after its mark in semibold
/// 12 pt in the rail's hue (its mark in that hue too); the others keep their own colours.
/// Phone as desktop, 13 pt marks; terminal the four glyphs a Cell apart, the label a Cell after
/// the current one. Screen readers hear the rail ("run passed, gate running, …") and the hint
/// after it; the hint is the tooltip on desktop and the status line in a terminal. No focus
/// stop of its own: it sits in a task row, which is.
pub fn stage_rail(app: AppState, rail: &StageRail) -> AnyElement {
    let form = app.form;
    let label_ink = match (rail.hue, form) {
        (Hue::Neutral, Form::Terminal) => color(Token::Ink),
        (Hue::Neutral, _) => color(Token::Ink2),
        (hue, _) => hue_ink(hue),
    };
    let mut parts: Vec<AnyElement> = Vec::with_capacity(8);
    for (stage, &each) in rail.marks.iter().enumerate() {
        let current = rail.current == Some(stage);
        if stage > 0 && form != Form::Terminal {
            parts.push(
                view()
                    .w(px(8.0))
                    .h(px(1.0))
                    .flex_shrink_0()
                    .bg(color(Token::Rule2))
                    .into_element(),
            );
        }
        let hue = if current { rail.hue } else { Hue::Neutral };
        parts.push(mark(app, each, hue));
        if current && !rail.label.is_empty() {
            let words = text(rail.label.clone())
                .number_of_lines(1u32)
                .flex_shrink_0()
                .text_color(label_ink);
            parts.push(match form {
                Form::Terminal => words.into_element(),
                Form::Desktop | Form::Phone => words
                    .font_family(FONT_TEXT)
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .into_element(),
            });
        }
    }
    let track = view()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .gap(match form {
            Form::Terminal => cells(1.0),
            Form::Desktop | Form::Phone => px(3.0),
        })
        .accessible(true)
        .role(AccessibilityRole::Image)
        .label(rail_label(rail.marks))
        .children(parts);
    hint_on(app, track, rail.hint.clone()).into_element()
}

/// How a minute's bar is drawn: its height as a share of the busiest minute's.
#[derive(Clone, Copy)]
struct Bar {
    share: f32,
    newest: bool,
}

/// The bar heights of `activity`, the newest marked: it alone takes the session's hue.
fn bars(activity: &Activity) -> Vec<Bar> {
    let heights = activity.heights();
    let newest = heights.len().saturating_sub(1);
    heights
        .into_iter()
        .enumerate()
        .map(|(index, share)| Bar {
            share,
            newest: index == newest,
        })
        .collect()
}

/// The terminal's bar: a block a Cell wide, its height in four steps; a quiet minute the
/// lowest block in rule-2, the hairline.
fn block(bar: Bar, hue: Hue) -> Text {
    let glyph = match bar.share {
        share if share <= 0.0 => '▁',
        share if share <= 0.25 => '▂',
        share if share <= 0.5 => '▃',
        share if share <= 0.75 => '▅',
        _ => '▇',
    };
    let ink = match (bar.share > 0.0, bar.newest) {
        (false, _) => color(Token::Rule2),
        (true, true) => hue_ink(hue),
        (true, false) => color(Token::Ink3),
    };
    text(glyph.to_string()).text_color(ink)
}

/// A drawn bar: a quiet minute a 2 pt hairline of rule-2; a busy one at least `floor` points
/// up to `height`, ink-4 fading with its share, the newest in the session's hue.
fn drawn_bar(bar: Bar, hue: Hue, height: f32, floor: f32) -> View {
    let base = view().flex_shrink_0().rounded(px(1.0));
    if bar.share <= 0.0 {
        return base.h(px(2.0)).bg(color(Token::Rule2));
    }
    let tall = (bar.share * height).round().max(floor);
    if bar.newest {
        base.h(px(tall)).bg(hue_ink(hue))
    } else {
        base.h(px(tall))
            .bg(color(Token::Ink4))
            .opacity(0.7 + 0.3 * bar.share)
    }
}

/// The strip in a terminal: one Cell per minute on one line.
fn terminal_strip(activity: &Activity) -> View {
    view().flex_row().flex_shrink_0().child(
        bars(activity)
            .into_iter()
            .fold(text("").number_of_lines(1u32), |line, bar| {
                line.child(block(bar, activity.hue))
            }),
    )
}

/// The frame of a strip or chart for screen readers: one image, its label the hint.
fn activity_frame(frame: View, activity: &Activity) -> View {
    frame
        .accessible(true)
        .role(AccessibilityRole::Image)
        .label(activity.hint.clone())
}

/// A session's last minutes in a roster row (`.strip`): one bar per minute, oldest first,
/// 4 pt wide and 2 pt apart on a 16 pt line (twenty minutes take 118 pt). A quiet minute is a
/// hairline of rule-2, a busy one rises with its share of the busiest minute in ink-4, and the
/// newest busy minute is in the session's hue. Phone as desktop. Terminal: `▁▂▃▅▇` one Cell per
/// minute, quiet minutes the lowest block in rule-2. Screen readers hear the hint; it is also
/// the tooltip (desktop) or the status line (terminal). No focus stop of its own: it sits in a
/// session row, which is. An empty series renders an empty element.
pub fn activity_strip(app: AppState, activity: &Activity) -> AnyElement {
    let form = app.form;
    if activity.per_minute.is_empty() {
        return view().into_element();
    }
    let strip = match form {
        Form::Terminal => terminal_strip(activity),
        Form::Desktop | Form::Phone => view()
            .flex_row()
            .items_end()
            .flex_shrink_0()
            .h(px(16.0))
            .gap(px(2.0))
            .children(
                bars(activity)
                    .into_iter()
                    .map(|bar| drawn_bar(bar, activity.hue, 16.0, 4.0).w(px(4.0)))
                    .collect::<Vec<_>>(),
            ),
    };
    let strip = activity_frame(strip, activity);
    tip(form, strip, activity.hint.clone().into()).into_element()
}

/// A session's last minutes in its panel (`.strip.big`): the strip across the full width,
/// 28 pt high, its bars sharing the width 3 pt apart. A focus stop of its own on desktop and in
/// a terminal, so its hint reaches the keyboard. Terminal: as [`activity_strip`]. An empty
/// series renders an empty element.
pub fn activity_chart(app: AppState, activity: &Activity) -> AnyElement {
    let form = app.form;
    if activity.per_minute.is_empty() {
        return view().into_element();
    }
    let chart = match form {
        Form::Terminal => terminal_strip(activity),
        Form::Desktop | Form::Phone => view()
            .flex_row()
            .items_end()
            .w_full()
            .h(px(28.0))
            .gap(px(3.0))
            .children(
                bars(activity)
                    .into_iter()
                    .map(|bar| {
                        drawn_bar(bar, activity.hue, 28.0, 6.0)
                            .flex_1()
                            .min_w(px(0.0))
                    })
                    .collect::<Vec<_>>(),
            ),
    };
    let chart = activity_frame(chart, activity)
        .when(form != Form::Phone, |chart| chart.focusable(true))
        .when(form == Form::Desktop, |chart| {
            chart.rounded(px(4.0)).focus_visible(|s| {
                s.outline(px(RING))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(RING))
            })
        });
    tip(form, chart, activity.hint.clone().into()).into_element()
}

/// The captions under an [`activity_chart`] (`.axis`): `from` at the start, `unit` in the
/// centre, `to` at the end, 4 pt under the chart. Desktop and phone 11 pt ink-3 on one line;
/// terminal the three quiet on one line. The words are the caller's.
pub fn activity_axis(app: AppState, from: &str, unit: &str, to: &str) -> AnyElement {
    let form = app.form;
    let caption = |words: &str| {
        let words = term::quiet(words.to_owned());
        match form {
            Form::Terminal => words.into_element(),
            Form::Desktop | Form::Phone => words
                .font_family(FONT_TEXT)
                .text_size(px(11.0))
                .into_element(),
        }
    };
    view()
        .flex_row()
        .items_center()
        .justify_between()
        .w_full()
        .gap(match form {
            Form::Terminal => cells(1.0),
            Form::Desktop | Form::Phone => px(8.0),
        })
        .when(form != Form::Terminal, |axis| axis.mt(px(4.0)))
        .child(caption(from))
        .child(caption(unit))
        .child(caption(to))
        .into_element()
}
