//! The Timeline destination: who worked when today. Desktop and phone draw the model's lanes on a
//! canvas measured to its container (`timeline/chart.rs`); a terminal draws the same lanes as rows
//! of Cells (`timeline/cells.rs`). The legend comes before the chart, the gaps worth a look and the
//! goal burn-up after it, on every form.

mod cells;
mod chart;

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::timeline::{
    self, GAPS, GAPS_DISCLOSURE, GAPS_TITLE, Gap, GroupOption, Grouping, Head, LegendItem, Range,
    RangeOption,
};
use crate::state::AppState;
use crate::ui::kit::button::seg;
use crate::ui::kit::chip::illus;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::page::{body, head, pane};
use crate::ui::kit::text::{h3, mono, plain};
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

/// Below this width the burn-up moves under the gaps instead of its 440 pt column beside them.
const SIDE_BY_SIDE: f32 = 880.0;
/// The burn-up column (`grid-template-columns: minmax(0,1fr) 440px`).
const BURNUP_COLUMN: f32 = 440.0;
/// The gaps list's measure (`max-width: 880px` with its 18 pt sides).
const GAPS_MEASURE: f32 = 844.0;
/// The burn-up's nominal size on each form, for its words (the canvas measures its own).
const BURNUP_NOMINAL: (f32, f32) = (404.0, 150.0);
const BURNUP_NOMINAL_PHONE: (f32, f32) = (354.0, 140.0);

/// The Timeline. Desktop: the page head (title, sub line, the Today / 7 days and By task / By
/// agent switches), then a scrolling pane of the legend, the lanes chart, and the gaps beside
/// the burn-up (stacked when narrow). Phone: the range switch across the page, the legend, the
/// compressed chart, the gaps and the burn-up stacked, in one scroll with the Ask clearance.
/// Terminal: the head, then the sub line, legend, lanes as Cells, gaps and burn-up in one scroll.
/// Test ids: "page-title", "seg-range-<i>", "seg-group-<i>", "timeline-pane".
pub fn screen(app: AppState) -> AnyElement {
    match app.form {
        Form::Desktop => desktop(app),
        Form::Phone => phone(app),
        Form::Terminal => terminal(app),
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

fn desktop(app: AppState) -> AnyElement {
    let page = page_head(app);
    let content = view()
        .flex_col()
        .child(legend_row(app))
        .child(
            view()
                .pt(px(8.0))
                .px(px(18.0))
                .pb(px(18.0))
                .child(chart::lanes(app)),
        )
        .child(lower(app));
    view()
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(head(
            app,
            page.title,
            Some(page.subtitle.to_owned()),
            head_actions(app, &page),
        ))
        .child(pane(app, "timeline-pane", content.into_element()))
        .into_element()
}

fn phone(app: AppState) -> AnyElement {
    let page = page_head(app);
    let content = view()
        .flex_col()
        .child(
            view()
                .mx(px(16.0))
                .mt(px(4.0))
                .mb(px(8.0))
                .child(range_switch(app, page.ranges)),
        )
        .child(legend_row(app))
        .child(view().px(px(14.0)).pb(px(10.0)).child(chart::lanes(app)))
        .child(gaps(app))
        .child(view().px(px(18.0)).pb(px(24.0)).child(burnup_section(app)));
    view()
        .flex_1()
        .flex_col()
        .min_h(px(0.0))
        .child(pane(app, "timeline-pane", content.into_element()))
        .into_element()
}

/// The sub line goes under the head: on a grid it would push the switches off the row.
fn terminal(app: AppState) -> AnyElement {
    let page = page_head(app);
    let content = view()
        .flex_col()
        .child(text(page.subtitle).text_color(color(Token::Ink3)))
        .child(term::blank())
        .child(legend_row(app))
        .child(term::blank())
        .child(cells::lanes(app))
        .child(term::blank())
        .child(gaps(app))
        .child(term::blank())
        .child(burnup_section(app));
    view()
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(head(app, page.title, None, head_actions(app, &page)))
        .child(body(app, "timeline-pane", content.into_element()))
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

/// By task / By agent (desktop): runs the model's `set_grouping`.
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

/// Under the chart on desktop: the gaps and the burn-up's 440 pt column side by side when there
/// is room, else stacked.
fn lower(app: AppState) -> AnyElement {
    let width = signal(0.0_f32);
    let wide = computed(move || width.get() >= SIDE_BY_SIDE);
    view()
        .w_full()
        .on_layout(move |frame: &Bounds<Points>| width.set(frame.size.width.0))
        .child(dynamic(move || {
            let burnup = view().px(px(18.0)).pb(px(18.0)).child(burnup_section(app));
            Some(if wide.get() {
                view()
                    .flex_row()
                    .items_start()
                    .child(view().flex_1().min_w(px(0.0)).child(gaps(app)))
                    .child(burnup.w(px(BURNUP_COLUMN)).flex_shrink_0())
                    .into_element()
            } else {
                view()
                    .flex_col()
                    .child(gaps(app))
                    .child(burnup.max_w(px(BURNUP_COLUMN)))
                    .into_element()
            })
        }))
        .into_element()
}

/// "Gaps worth a look today": each gap's span in its state colour, the task, what happened, then
/// the illustrative-data note. Desktop: one row in 96 / 54 / rest columns, 12.5 pt; phone: the
/// span and task on one line, the words under them; terminal: one row of 13 / 7 Cells and the
/// words wrapping beside them.
fn gaps(app: AppState) -> AnyElement {
    let form = app.form;
    let list = view()
        .flex_col()
        .gap(px(if form == Form::Phone { 12.0 } else { 6.0 }))
        .children(GAPS.iter().map(|gap| gap_row(app, gap)).collect::<Vec<_>>())
        .child(disclosure(form, GAPS_DISCLOSURE));
    let section = view().flex_col().child(h3(app, GAPS_TITLE, None));
    match form {
        Form::Desktop => section
            .px(px(18.0))
            .pb(px(18.0))
            .child(list.max_w(px(GAPS_MEASURE))),
        Form::Phone => section.px(px(18.0)).pb(px(18.0)).child(list),
        Form::Terminal => section.child(list),
    }
    .into_element()
}

fn gap_row(app: AppState, gap: &Gap) -> AnyElement {
    let form = app.form;
    let span = mono(app, gap.time)
        .text_color(hue_ink(gap.kind.hue()))
        .flex_shrink_0();
    let task = mono(app, gap.task)
        .text_color(color(Token::Ink3))
        .flex_shrink_0();
    let words = plain(app, gap.wording);
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

/// The goal burn-up: its caption with the period at the trailing edge, the chart (a canvas, or
/// block rows in a terminal), and the illustrative-numbers tag.
fn burnup_section(app: AppState) -> AnyElement {
    let form = app.form;
    let (width, height) = if form == Form::Phone {
        BURNUP_NOMINAL_PHONE
    } else {
        BURNUP_NOMINAL
    };
    let words = timeline::burnup(width, height);
    let drawn = if form == Form::Terminal {
        cells::burnup()
    } else {
        chart::burnup(app)
    };
    view()
        .flex_col()
        .child(h3(app, words.title, Some(words.period.to_owned())))
        .child(drawn)
        .child(illus(app, words.disclosure))
        .into_element()
}
