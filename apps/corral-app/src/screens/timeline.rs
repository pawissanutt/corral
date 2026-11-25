//! Activity › Timeline: who worked when. Desktop and phone draw the lanes on a canvas
//! measured to its container (`timeline/chart.rs`); a terminal draws the same lanes as rows of
//! Cells (`timeline/cells.rs`). The lanes' legend comes under them, then the goal burn-up with
//! its own legend at its foot (it counts events and does not narrow with the range), then the
//! gaps worth a look, on every form. A daemon session and the explicit sample preview draw
//! the same recorded spans from their feed (`shown`), with loading, stale and failed states said
//! in place.

mod cells;
mod chart;
mod scope;
mod shown;

use corral_protocol::detail::DetailRef;
use nsh_core::prelude::*;

use crate::model::text::span;
use crate::model::timeline::{
    self, BURNUP_RANGE_NOTE, BurnupLine, Gap, GroupOption, Grouping, Head, LIVE_GAPS_NOTE,
    LIVE_SUBTITLE, LegendItem, Range, RangeOption, TallyEntry,
};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::button::seg;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::notice::{empty, error, notice};
use crate::ui::kit::page::{body, head, measure, measured_head, pane};
use crate::ui::kit::text::{h3, mono, plain};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, MEASURE_PAGE, Token, color};

use shown::{Shown, Status};

/// The burn-up's measure on a wide page, the same as the gaps list's.
const BURNUP_MEASURE: f32 = 880.0;
/// The gaps list's measure (`max-width: 880px` with its 18 pt sides).
const GAPS_MEASURE: f32 = 844.0;

/// The Timeline. Desktop: the page head (title, sub line, the Today / 7 days and By task / By
/// run switches), then a scrolling pane of the legend, the lanes chart, the burn-up and the
/// gaps, all kept to [`MEASURE_PAGE`] and centred. Phone: the range and grouping switches across the page,
/// the legend, the compressed chart, the gaps and the burn-up stacked, in one scroll with the Ask
/// clearance. Terminal: the head, then the sub line, legend, lanes as Cells, gaps and burn-up in
/// one scroll. The burn-up's scope is a menu (`scope.rs`). Test ids: "page-title",
/// "seg-range-<i>", "seg-group-<i>", "timeline-pane", "btn-burnup-scope", "seg-burnup-<scope>".
pub fn screen(app: AppState) -> AnyElement {
    shown::keep_loaded(app);
    let data = computed(move || shown::shown(app));
    match app.form {
        Form::Desktop => desktop(app, data),
        Form::Phone => phone(app, data),
        Form::Terminal => terminal(app, data),
    }
}

fn page_head(app: AppState) -> Head {
    let phone = app.form == Form::Phone;
    untracked(|| {
        let env = app.env();
        app.timeline.with(|state| timeline::head(state, env, phone))
    })
}

fn head_actions(app: AppState, page: &Head) -> Vec<AnyElement> {
    let mut actions = vec![range_switch(app, page.ranges)];
    if let Some(options) = page.grouping {
        actions.push(grouping_switch(app, options));
    }
    actions
}

fn desktop(app: AppState, data: Memo<Shown>) -> AnyElement {
    let page = page_head(app);
    let content = view()
        .flex_col()
        .child(status(app, data))
        .child(
            view()
                .pt(px(12.0))
                .px(px(18.0))
                .pb(px(8.0))
                .child(chart::lanes(app, data)),
        )
        .child(legend_row(app))
        .child(lower(app, data));
    view()
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(measured_head(
            app,
            MEASURE_PAGE,
            page.title,
            Some(LIVE_SUBTITLE.to_owned()),
            head_actions(app, &page),
        ))
        .child(pane(
            app,
            "timeline-pane",
            measure(app, MEASURE_PAGE, content.into_element()),
        ))
        .into_element()
}

fn phone(app: AppState, data: Memo<Shown>) -> AnyElement {
    let page = page_head(app);
    let content = view()
        .flex_col()
        .child(
            view()
                .mx(px(16.0))
                .mt(px(4.0))
                .mb(px(8.0))
                .gap(px(8.0))
                .child(range_switch(app, page.ranges))
                // By task or by run: the same lanes regrouped, as on the other forms (LA-15).
                .children(page.grouping.map(|options| grouping_switch(app, options))),
        )
        .child(status(app, data))
        .child(
            view()
                .px(px(14.0))
                .pb(px(10.0))
                .child(chart::lanes(app, data)),
        )
        .child(legend_row(app))
        .child(
            view()
                .px(px(18.0))
                .pt(px(12.0))
                .pb(px(18.0))
                .child(burnup_section(app, data)),
        )
        .child(gaps(app, data));
    view()
        .flex_1()
        .flex_col()
        .min_h(px(0.0))
        .child(pane(app, "timeline-pane", content.into_element()))
        .into_element()
}

/// The sub line goes under the head: on a grid it would push the switches off the row.
fn terminal(app: AppState, data: Memo<Shown>) -> AnyElement {
    let page = page_head(app);
    let content = view()
        .flex_col()
        .child(text(LIVE_SUBTITLE).text_color(color(Token::Ink3)))
        .child(status(app, data))
        .child(term::blank())
        .child(cells::lanes(app, data))
        .child(legend_row(app))
        .child(term::blank())
        .child(burnup_section(app, data))
        .child(term::blank())
        .child(gaps(app, data));
    view()
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(head(app, page.title, None, head_actions(app, &page)))
        .child(body(app, "timeline-pane", content.into_element()))
        .into_element()
}

/// What the recorded spans' request is doing when it is not simply shown: loading, kept from
/// before a lost connection, failed, or answered with no lanes.
fn status(app: AppState, data: Memo<Shown>) -> AnyElement {
    let state = computed(move || data.with(|shown| (shown.status.clone(), shown.lanes_empty())));
    dynamic(move || {
        let (status, lanes_empty) = state.get();
        untracked(move || {
            let pad = |element: AnyElement| match app.form {
                Form::Terminal => element,
                Form::Desktop | Form::Phone => view()
                    .px(px(18.0))
                    .pt(px(8.0))
                    .child(element)
                    .into_element(),
            };
            let said = match status {
                Status::Ready if lanes_empty => Some(empty(
                    app,
                    "No recorded work in this range",
                    "corral recorded no run or task event here yet.",
                    None,
                )),
                Status::Ready => None,
                Status::Loading => Some(pad(notice(
                    app,
                    Hue::Neutral,
                    &vec![span("Loading recorded runs and events from corrald…")],
                    None,
                ))),
                Status::Stale => Some(pad(notice(
                    app,
                    Hue::Held,
                    &vec![span(
                        "Showing the last runs and events received; they may be out of date until corrald answers again.",
                    )],
                    None,
                ))),
                Status::Failed(message) => Some(error(
                    app,
                    &vec![span("Timeline unavailable: ").bold(), span(message)],
                    None,
                )),
            };
            view().children(said).into_element()
        })
    })
    .into_element()
}

/// Today / 7 days: runs the model's `set_range`; the pressed option is the model's selection.
fn range_switch(app: AppState, ranges: [RangeOption; 2]) -> AnyElement {
    let phone = app.form == Form::Phone;
    let options: Vec<(Range, &'static str)> = ranges
        .iter()
        .map(|option| (option.range, option.label))
        .collect();
    seg(
        app,
        &options,
        move || {
            let env = app.env();
            app.timeline
                .with(|state| timeline::head(state, env, phone))
                .ranges
                .iter()
                .find(|option| option.selected)
                .map(|option| option.range)
                .unwrap_or_default()
        },
        move |range| app.act(app.timeline, |state, _| state.set_range(range)),
        "seg-range",
    )
}

/// By task / By run: runs the model's `set_grouping`.
fn grouping_switch(app: AppState, groups: [GroupOption; 2]) -> AnyElement {
    let options: Vec<(Grouping, &'static str)> = groups
        .iter()
        .map(|option| (option.grouping, option.label))
        .collect();
    seg(
        app,
        &options,
        move || {
            let env = app.env();
            app.timeline
                .with(|state| timeline::head(state, env, false))
                .grouping
                .and_then(|groups| groups.into_iter().find(|option| option.selected))
                .map(|option| option.grouping)
                .unwrap_or_default()
        },
        move |grouping| app.act(app.timeline, |state, _| state.set_grouping(grouping)),
        "seg-group",
    )
}

/// The legend (`.legend`): each kind's swatch and name in ink-3, wrapping. Desktop 11.5 pt,
/// 16 pt apart, padding 0 18 4; phone 12 pt, padding 4 18 8; terminal each kind's two glyphs
/// in its colour and its name, as one wrapping line.
fn legend_row(app: AppState) -> AnyElement {
    let items = timeline::legend();
    match app.form {
        // Each entry is one unbreakable line in a wrapping row, so an entry never splits across
        // rows; its trailing blanks set it apart from the next.
        Form::Terminal => view()
            .flex_row()
            .flex_wrap()
            .children(
                items
                    .into_iter()
                    .map(|item| {
                        let mark = cells::glyph(item.kind);
                        text("")
                            .number_of_lines(1u32)
                            .flex_shrink_0()
                            .child(
                                text(format!("{mark}{mark} "))
                                    .text_color(cells::kind_ink(item.kind)),
                            )
                            .child(
                                text(format!("{}   ", item.label)).text_color(color(Token::Ink3)),
                            )
                    })
                    .collect::<Vec<_>>(),
            )
            .into_element(),
        form => {
            let phone = form == Form::Phone;
            let row = view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap_x(px(16.0))
                .gap_y(px(6.0))
                .px(px(18.0));
            let row = if phone {
                row.pt(px(4.0)).pb(px(8.0))
            } else {
                row.pb(px(4.0))
            };
            row.children(
                items
                    .into_iter()
                    .map(|item| legend_item(item, if phone { 12.0 } else { 11.5 }))
                    .collect::<Vec<_>>(),
            )
            .into_element()
        }
    }
}

fn legend_item(item: LegendItem, size: f32) -> AnyElement {
    view()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .child(chart::swatch(item.kind))
        .child(
            text(item.label)
                .number_of_lines(1u32)
                .font_family(FONT_TEXT)
                .text_size(px(size))
                .text_color(color(Token::Ink3)),
        )
        .into_element()
}

/// Under the lanes and their legend on desktop: the burn-up across the page's measure, then
/// the gaps.
fn lower(app: AppState, data: Memo<Shown>) -> AnyElement {
    view()
        .w_full()
        .child(
            view()
                .px(px(18.0))
                .pt(px(18.0))
                .pb(px(18.0))
                .max_w(px(BURNUP_MEASURE))
                .child(burnup_section(app, data)),
        )
        .child(gaps(app, data))
        .into_element()
}

/// "Gaps worth a look": each gap's span in its state colour, the task, what happened, then the
/// note that the gaps come from corral's recorded phase times.
/// Desktop: one row in 96 / 54 / rest columns, 12.5 pt; phone: the span and task on one line,
/// the words under them; terminal: one row of 13 / 7 Cells and the words wrapping beside them.
fn gaps(app: AppState, data: Memo<Shown>) -> AnyElement {
    let form = app.form;
    let content = computed(move || {
        data.with(|shown| (shown.gaps_title, shown.gaps.clone(), shown.gaps_empty))
    });
    dynamic(move || {
        let (title, items, nothing) = content.get();
        untracked(move || {
            let mut rows: Vec<AnyElement> = items.iter().map(|gap| gap_row(app, gap)).collect();
            if items.is_empty()
                && let Some(line) = nothing
            {
                rows.push(
                    plain(app, line)
                        .text_color(color(Token::Ink3))
                        .into_element(),
                );
            }
            let list = view()
                .flex_col()
                .gap(px(if form == Form::Phone { 12.0 } else { 6.0 }))
                .children(rows)
                .child(disclosure(form, LIVE_GAPS_NOTE));
            let section = view().flex_col().child(h3(app, title, None));
            match form {
                Form::Desktop => section
                    .px(px(18.0))
                    .pb(px(18.0))
                    .child(list.max_w(px(GAPS_MEASURE))),
                Form::Phone => section.px(px(18.0)).pb(px(18.0)).child(list),
                Form::Terminal => section.child(list),
            }
            .into_element()
        })
    })
}

fn gap_row(app: AppState, gap: &Gap) -> AnyElement {
    let form = app.form;
    let span = mono(app, gap.time.clone())
        .text_color(hue_ink(gap.kind.hue()))
        .flex_shrink_0();
    let task = mono(app, gap.task.clone())
        .text_color(color(Token::Ink3))
        .flex_shrink_0();
    let words = plain(app, gap.wording.clone());
    match form {
        Form::Desktop => view()
            .flex_row()
            .items_start()
            .gap(px(8.0))
            .child(span.w(px(96.0)))
            .child(task.w(px(54.0)))
            .child(
                words
                    .text_size(px(12.5))
                    .line_height(px(18.0))
                    .flex_1()
                    .min_w(px(0.0)),
            ),
        Form::Phone => view()
            .flex_col()
            .gap(px(3.0))
            .child(view().flex_row().gap(px(10.0)).child(span).child(task))
            .child(words.text_size(px(14.0)).line_height(px(20.0))),
        Form::Terminal => view()
            .flex_row()
            .items_start()
            .child(span.w(term::cells(13.0)))
            .child(task.w(term::cells(7.0)))
            .child(words.flex_1().min_w(px(0.0))),
    }
    .into_element()
}

/// The `.illus` note at paragraph length: the kit's tag is one line, this one wraps.
fn disclosure(form: Form, words: &'static str) -> AnyElement {
    let note = text(words).italic();
    match form {
        Form::Terminal => note.text_color(color(Token::Ink3)),
        Form::Desktop | Form::Phone => {
            let size = if form == Form::Phone { 12.0 } else { 10.5 };
            note.font_family(FONT_TEXT)
                .text_size(px(size))
                .line_height(px((size * 1.45).round()))
                .letter_spacing(px(0.1))
                .text_color(color(Token::Ink4))
        }
    }
    .into_element()
}

/// The goal burn-up: its caption with the period at the trailing edge, the scope it covers (a
/// menu of the workspace, its projects and plans) and the chart (a canvas, or block rows in a
/// terminal). A burn-up corral has not recorded, or one still loading or failed, says so in
/// place of the chart; a failure with the spans' own cause is not said twice.
fn burnup_section(app: AppState, data: Memo<Shown>) -> AnyElement {
    let form = app.form;
    let series = computed(move || data.with(|shown| shown.burnup.clone()));
    let state = computed(move || data.with(|shown| shown.burnup_status.clone()));
    let alike = computed(move || data.with(Shown::burnup_failed_alike));
    dynamic(move || {
        let series = series.get();
        let state = state.get();
        let alike = alike.get();
        untracked(move || {
            let scope = view()
                .flex_row()
                .when(form != Form::Terminal, |row| row.pb(px(6.0)))
                .child(scope::picker(app));
            burnup_body(app, (series, state), alike, Some(scope.into_element()))
        })
    })
}

/// A goal burn-up of one fixed scope outside the Timeline (a project's page): the same chart and
/// states as the Timeline's, without its scope menu, requested while shown.
pub(crate) fn scoped_burnup(app: AppState, request: DetailRef) -> AnyElement {
    shown::keep_burnup(app, request.clone());
    let parts = computed(move || shown::burnup_of(app, &request));
    dynamic(move || {
        let parts = parts.get();
        untracked(move || burnup_body(app, parts, false, None))
    })
}

/// The burn-up's caption, `scope` under it and the chart, or the line that says why there is
/// none.
fn burnup_body(
    app: AppState,
    (series, state): shown::BurnupParts,
    alike: bool,
    scope: Option<AnyElement>,
) -> AnyElement {
    let form = app.form;
    let Some(series) = series else {
        let line = match state {
            Status::Loading => "Loading the goal burn-up…".to_owned(),
            Status::Failed(_) if alike => "Goal burn-up unavailable.".to_owned(),
            Status::Failed(message) => format!("Goal burn-up unavailable: {message}"),
            Status::Ready | Status::Stale => {
                "corral has recorded no created or landed task to plot yet.".to_owned()
            }
        };
        return view()
            .flex_col()
            .child(h3(app, "Goal burn-up", None))
            .children(scope)
            .child(plain(app, line).text_color(color(Token::Ink3)))
            .into_element();
    };
    let title = series.title.clone();
    let period = series.period.clone();
    // The Timeline's burn-up says the range above leaves it whole; elsewhere there is no range.
    let note = if scope.is_some() {
        BURNUP_RANGE_NOTE
    } else {
        series.counts
    };
    let tally = series.tally.clone();
    let drawn = if form == Form::Terminal {
        cells::burnup(&series)
    } else {
        chart::burnup(app, series)
    };
    view()
        .flex_col()
        .child(h3(app, title, Some(period)))
        .children(scope)
        .child(drawn)
        .child(
            view()
                .pt(px(if form == Form::Terminal { 0.0 } else { 6.0 }))
                .gap(px(4.0))
                .test_id("burnup-counts")
                .child(tally_row(form, &tally))
                .child(disclosure(form, note)),
        )
        .into_element()
}

/// The burn-up's legend (`.burn .legend`): each count in ink-3, the two the chart draws behind
/// a short stroke of their line's ink, wrapping.
fn tally_row(form: Form, tally: &[TallyEntry]) -> AnyElement {
    let ink = |line: BurnupLine| match line {
        BurnupLine::Scope => color(Token::Ink3),
        BurnupLine::Landed => color(Token::Ink),
    };
    let items = tally.iter().map(|entry| {
        let words = text(entry.words.clone())
            .number_of_lines(1u32)
            .text_color(color(Token::Ink3));
        match form {
            Form::Terminal => text("")
                .number_of_lines(1u32)
                .flex_shrink_0()
                .children(entry.line.map(|line| text("── ").text_color(ink(line))))
                .child(words)
                .child(text("   "))
                .into_element(),
            Form::Desktop | Form::Phone => view()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .children(
                    entry
                        .line
                        .map(|line| view().w(px(14.0)).h(px(2.0)).bg(ink(line))),
                )
                .child(
                    words
                        .font_family(FONT_TEXT)
                        .text_size(px(if form == Form::Phone { 12.0 } else { 11.5 })),
                )
                .into_element(),
        }
    });
    view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .when(form != Form::Terminal, |row| {
            row.gap_x(px(14.0)).gap_y(px(4.0))
        })
        .children(items.collect::<Vec<_>>())
        .into_element()
}
