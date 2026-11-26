//! The lane inspector: the selected lane's facts (`model::timeline::inspect`). Desktop and
//! web: a side column of the list pages' inspector width beside the chart where the page has
//! room, otherwise under the chart; terminal: beside the chart from 100 Cells of page, else
//! under it; phone: a block under the chart. Test ids: "inspector", "inspector-title",
//! "btn-open-task", "btn-inspector-close", "inspect-wait-<id>", "inspect-unblock-<id>".

use nsh_core::prelude::*;

use super::shown::Shown;
use crate::model::list::inspector_width;
use crate::model::timeline::{Inspector, Link};
use crate::model::{Form, Hue};
use crate::state::AppState;
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::status::status_word_mark;
use crate::ui::kit::text::{h3, label_value, mono, plain};
use crate::ui::term::{self, cells};
use crate::ui::tokens::{FONT_MONO, Token, color};

/// The least desktop page that keeps the inspector beside the chart, as a list page's.
const BESIDE: f32 = 980.0;
/// The same in a terminal: the chart keeps its label column and a readable track.
const BESIDE_CELLS: f32 = 100.0;

/// Whether the inspector stands beside the chart on a page `width` wide (points; before the
/// first layout, as wide as the window allows). Never on the phone.
fn beside(form: Form, width: f32) -> bool {
    match form {
        Form::Phone => false,
        Form::Desktop => width <= 0.0 || width >= BESIDE,
        Form::Terminal => width <= 0.0 || width / cells(1.0).0 >= BESIDE_CELLS,
    }
}

/// The inspector's column beside the chart, while a lane is selected and the page has room.
pub fn side(app: AppState, data: Memo<Shown>, width: Signal<f32>) -> AnyElement {
    let form = app.form;
    let inspector = computed(move || data.with(|shown| shown.inspector.clone()));
    let shown = computed(move || beside(form, width.get()));
    dynamic(move || {
        let inspector = inspector.get()?;
        if !shown.get() {
            return None;
        }
        let room = untracked(|| width.get());
        Some(untracked(move || {
            let content = view()
                .test_id("inspector")
                .flex_shrink_0()
                .min_h(px(0.0))
                .child(scroll().flex_1().min_h(px(0.0)).child(body(app, inspector)));
            match form {
                Form::Terminal => view()
                    .flex_row()
                    .flex_shrink_0()
                    .min_h(px(0.0))
                    .child(term::vrule())
                    .child(content.w(cells(inspector_width(room / cells(1.0).0, true))))
                    .into_element(),
                Form::Desktop | Form::Phone => content
                    .w(px(inspector_width(room, false)))
                    .border_l(px(1.0))
                    .border_color(color(Token::Rule))
                    .bg(color(Token::Surface))
                    .into_element(),
            }
        }))
    })
    .into_element()
}

/// The inspector under the chart, while a lane is selected and the page has no room beside it.
pub fn below(app: AppState, data: Memo<Shown>, width: Signal<f32>) -> AnyElement {
    let form = app.form;
    let inspector = computed(move || data.with(|shown| shown.inspector.clone()));
    let shown = computed(move || !beside(form, width.get()));
    dynamic(move || {
        let inspector = inspector.get()?;
        if !shown.get() {
            return None;
        }
        Some(untracked(move || {
            let block = view().test_id("inspector").child(body(app, inspector));
            match form {
                Form::Terminal => view()
                    .flex_col()
                    .child(term::hrule())
                    .child(block)
                    .child(term::hrule())
                    .into_element(),
                Form::Desktop | Form::Phone => block
                    .mx(px(14.0))
                    .mb(px(10.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(color(Token::Rule))
                    .bg(color(Token::Surface))
                    .into_element(),
            }
        }))
    })
    .into_element()
}

/// A dependency or dependent: its id, title and where it stands, the words in the danger
/// colour when it has not landed.
fn link_row(app: AppState, link: &Link, prefix: &str) -> AnyElement {
    let words = plain(app, link.words.clone()).text_color(if link.danger {
        hue_ink(Hue::Fail)
    } else {
        color(Token::Ink3)
    });
    view()
        .test_id(&format!("{prefix}-{}", link.id))
        .flex_row()
        .flex_wrap()
        .gap_x(px(if app.form == Form::Terminal { 8.0 } else { 6.0 }))
        .child(mono(app, link.id.clone()).flex_shrink_0())
        .children((!link.title.is_empty()).then(|| plain(app, link.title.clone())))
        .child(words)
        .into_element()
}

fn section(app: AppState, title: String, rows: Vec<AnyElement>) -> AnyElement {
    view()
        .flex_col()
        .gap(px(4.0))
        .child(h3(app, title, None))
        .children(rows)
        .into_element()
}

fn body(app: AppState, inspector: Inspector) -> AnyElement {
    let form = app.form;
    let title = text(
        format!("{} {}", inspector.task, inspector.title)
            .trim_end()
            .to_owned(),
    )
    .test_id("inspector-title")
    .font_weight(FontWeight::BOLD)
    .text_color(color(Token::Ink))
    .when(form != Form::Terminal, |t| {
        t.font_family(FONT_MONO).text_size(px(15.0))
    });
    let warning = inspector.warning.clone().map(|words| {
        plain(app, format!("! {words}"))
            .font_weight(FontWeight::BOLD)
            .text_color(hue_ink(Hue::Fail))
            .into_element()
    });
    let facts = inspector
        .facts
        .iter()
        .map(|(label, value)| label_value(app, *label, plain(app, value.clone()).into_element()));
    let waits: Vec<AnyElement> = inspector
        .waits
        .iter()
        .map(|link| link_row(app, link, "inspect-wait"))
        .collect();
    let unblocks: Vec<AnyElement> = inspector
        .unblocks
        .iter()
        .map(|link| link_row(app, link, "inspect-unblock"))
        .collect();
    let gaps: Vec<AnyElement> = inspector
        .gaps
        .iter()
        .map(|gap| super::gap_row(app, gap))
        .collect();
    let events: Vec<AnyElement> = inspector
        .events
        .iter()
        .map(|line| plain(app, line.clone()).into_element())
        .collect();
    let open = inspector.open.live.clone();
    let actions = view()
        .flex_row()
        .flex_wrap()
        .gap(px(8.0))
        .pt(px(if form == Form::Terminal { 0.0 } else { 10.0 }))
        .child(button(
            app,
            inspector.open.label.clone(),
            ButtonKind::Primary,
            "btn-open-task",
            move || {
                let open = open.clone();
                app.act(app.timeline, move |state, _| state.open(open));
            },
        ))
        .child(button(
            app,
            "Close",
            ButtonKind::Quiet,
            "btn-inspector-close",
            move || app.act(app.timeline, |state, _| state.deselect()),
        ));
    let mut parts = vec![
        view()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(px(8.0))
            .child(title)
            .children(inspector.status.map(|status| status_word_mark(app, status)))
            .into_element(),
    ];
    parts.extend(warning);
    parts.push(
        view()
            .flex_col()
            .gap(px(5.0))
            .children(facts)
            .into_element(),
    );
    if !waits.is_empty() {
        parts.push(section(app, inspector.waits_title.clone(), waits));
    }
    if !unblocks.is_empty() {
        parts.push(section(app, inspector.unblocks_title.clone(), unblocks));
    }
    if !gaps.is_empty() {
        parts.push(section(app, inspector.gaps_title.to_owned(), gaps));
    }
    if !events.is_empty() {
        parts.push(section(app, inspector.events_title.to_owned(), events));
    }
    parts.push(actions.into_element());
    let column = view().flex_col().gap(px(6.0)).children(parts);
    match form {
        Form::Terminal => column.px(cells(1.0)).into_element(),
        Form::Desktop | Form::Phone => column.p(px(16.0)).into_element(),
    }
}
