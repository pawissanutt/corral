//! Changes: the time feed since this device's last visit, a mark for each kind of event, each
//! new event opening its task, then the per-device seen line and what came before it.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::work::{self, Changes};
use crate::screens::now::feed::{row, seen_line};
use crate::screens::now::parts::{dim_when, model_button, stale};
use crate::state::AppState;
use crate::ui::kit::button::seg;
use crate::ui::kit::chip::illus;
use crate::ui::kit::page::{head, pane};
use crate::ui::term;

/// The feed's measure on a wide screen (`.feed` at most 900 pt).
const MEASURE: f32 = 900.0;

/// The Changes destination. Desktop and terminal: the page head (title, the per-device note, the
/// filters and Mark seen) over the scrolling feed. Phone: the shell heads the pushed page; this
/// is its scrolling body, the filters over the feed.
pub fn screen(app: AppState) -> AnyElement {
    let changes = computed(move || {
        let env = app.env();
        app.work.with(|state| work::changes(state, env))
    });
    let stale = stale(app);
    let body = dim_when(view(), stale)
        .child(dynamic(move || changes.with(|changes| feed(app, changes))))
        .into_element();
    if app.form == Form::Phone {
        let content = view()
            .child(
                view()
                    .px(px(16.0))
                    .pt(px(4.0))
                    .pb(px(8.0))
                    .child(filters(app, changes)),
            )
            .child(body)
            .into_element();
        return pane(app, "changes", content);
    }
    let (title, subtitle, mark_seen) = untracked(|| {
        changes.with(|changes| {
            (
                changes.title,
                changes.subtitle.map(str::to_owned),
                changes.mark_seen.clone(),
            )
        })
    });
    let mark = move || {
        mark_seen
            .as_ref()
            .map(|button| model_button(app, button, "btn-mark-seen".into(), false))
    };
    if app.form == Form::Desktop {
        let mut actions = vec![filters(app, changes)];
        actions.extend(mark());
        return view()
            .flex_1()
            .min_h(px(0.0))
            .child(head(app, title, subtitle, actions))
            .child(pane(app, "changes", body))
            .into_element();
    }
    // A terminal keeps every label whole: the filters sit beside the title while the row holds
    // them, and take a row of their own under it when it does not.
    let need = untracked(|| changes.with(|changes| inline_cells(title, changes)));
    let width = signal(0.0_f32);
    let beside = computed(move || {
        let points = width.get();
        points <= 0.0 || points / term::cells(1.0).0 >= need
    });
    view()
        .flex_1()
        .min_h(px(0.0))
        .on_layout(move |frame: &Bounds<Points>| width.set(frame.size.width.0))
        .child(dynamic(move || {
            if beside.get() {
                let mut actions = vec![filters(app, changes)];
                actions.extend(mark());
                return head(app, title, subtitle.clone(), actions);
            }
            view()
                .child(head(
                    app,
                    title,
                    subtitle.clone(),
                    mark().into_iter().collect(),
                ))
                .child(
                    view()
                        .flex_row()
                        .px(term::cells(1.0))
                        .child(filters(app, changes)),
                )
                .child(term::hrule())
                .into_element()
        }))
        .child(pane(app, "changes", body))
        .into_element()
}

/// The least subtitle worth keeping beside the title before the filters take their own row.
const SUBTITLE_CELLS: f32 = 12.0;

/// The Cells a terminal head needs to hold the filters beside the title: its Cell either side,
/// the title, two Cells, the least subtitle, then the filters (each label with a Cell either side,
/// a Cell between them) and Mark seen (its label with a Cell either side), two Cells apart.
fn inline_cells(title: &str, changes: &Changes) -> f32 {
    let count = |words: &str| words.chars().count() as f32;
    let filters: f32 = changes
        .filters
        .iter()
        .map(|filter| count(filter.label) + 2.0)
        .sum::<f32>()
        + changes.filters.len().saturating_sub(1) as f32;
    let mark = changes
        .mark_seen
        .as_ref()
        .map_or(0.0, |button| 2.0 + count(&*button.label) + 2.0);
    1.0 + count(title) + 2.0 + SUBTITLE_CELLS + 2.0 + filters + mark + 1.0
}

/// The model's filters as a segmented control. The mock's filters have no handler: the selection
/// stays where the model puts it.
fn filters(app: AppState, changes: Memo<Changes>) -> AnyElement {
    changes.with_untracked(|changes| {
        let options: Vec<(usize, &'static str)> = changes
            .filters
            .iter()
            .enumerate()
            .map(|(index, filter)| (index, filter.label))
            .collect();
        let selected = changes
            .filters
            .iter()
            .position(|filter| filter.selected)
            .unwrap_or(0);
        seg(app, &options, move || selected, |_| {}, "seg-changes")
    })
}

/// The recent events, the seen line, the dimmed history and the illustrative note.
fn feed(app: AppState, changes: &Changes) -> AnyElement {
    let form = app.form;
    let recent = changes
        .recent
        .iter()
        .enumerate()
        .map(|(index, event)| row(app, event, format!("change-{index}")));
    let history = changes
        .history
        .iter()
        .enumerate()
        .map(|(index, event)| row(app, event, format!("change-seen-{index}")));
    let mut events: Vec<AnyElement> = recent.collect();
    events.push(seen_line(app, changes.seen_line));
    events.extend(history);
    view()
        .child(
            view()
                .py(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
                .when(form == Form::Desktop, |feed| feed.max_w(px(MEASURE)))
                .children(events),
        )
        .children(changes.illustrative.map(|note| {
            view()
                .flex_row()
                .px(px(16.0))
                .py(px(8.0))
                .child(illus(app, note))
                .into_element()
        }))
        .into_element()
}
