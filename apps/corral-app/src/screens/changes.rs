//! Changes: the durable change lines since this device's last visit with their attribution, a
//! mark for each kind of event, each event opening its task, then the per-device seen line and
//! what came before it. Its own project scope and the filters narrow it, older pages load on
//! request, and Mark seen moves this device's own seen cursor.

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::projects::FilterScreen;
use crate::model::work::{self, Action, ChangeFilter, Changes};
use crate::screens::now::feed::{row, seen_line};
use crate::screens::now::parts::{act, dim_when, model_button, pressable, quiet, stale};
use crate::state::AppState;
use crate::ui::kit::button::seg;
use crate::ui::kit::chip::illus;
use crate::ui::kit::page::{head, pane};
use crate::ui::kit::project_filter::project_filter;
use crate::ui::term;

/// The feed's measure on a wide screen (`.feed` at most 900 pt).
const MEASURE: f32 = 900.0;

/// The Changes destination. Desktop and terminal: the page head (title, the per-device note, the
/// filters and Mark seen) over the scrolling feed. Phone: the shell heads the pushed page; this
/// is its scrolling body, the filters over the feed.
pub fn screen(app: AppState) -> AnyElement {
    let form = app.form;
    let changes = computed(move || {
        let device = app.device_name();
        let filter = app.project_filter(FilterScreen::Changes);
        app.feed.with(|feed| {
            app.work.with(|state| {
                work::live_changes_filtered(state, feed, device.as_deref(), form, &filter)
            })
        })
    });
    let stale = stale(app);
    let body = dim_when(view(), stale)
        .when(form != Form::Phone, |body| {
            body.child(project_filter(app, FilterScreen::Changes))
        })
        .child(dynamic(move || changes.with(|changes| feed(app, changes))))
        .into_element();
    if form == Form::Phone {
        let content = view()
            .child(project_filter(app, FilterScreen::Changes))
            .child(
                view()
                    .px(px(16.0))
                    .pt(px(4.0))
                    .pb(px(8.0))
                    .gap(px(8.0))
                    .child(filters(app, changes))
                    .child(dynamic(move || {
                        changes.with(|changes| {
                            changes.mark_seen.as_ref().map(|button| {
                                view()
                                    .flex_row()
                                    .child(model_button(app, button, "btn-mark-seen".into(), false))
                                    .into_element()
                            })
                        })
                    })),
            )
            .child(body)
            .into_element();
        return pane(app, "changes", content);
    }
    let heading = computed(move || {
        changes.with(|changes| {
            (
                changes.title,
                changes.subtitle.clone(),
                changes.mark_seen.clone(),
                inline_cells(changes.title, changes),
            )
        })
    });
    let mark = move |button: &Option<work::Button>| {
        button
            .as_ref()
            .map(|button| model_button(app, button, "btn-mark-seen".into(), false))
    };
    if form == Form::Desktop {
        return view()
            .flex_1()
            .min_h(px(0.0))
            .child(dynamic(move || {
                heading.with(|(title, subtitle, mark_seen, _)| {
                    let mut actions = vec![filters(app, changes)];
                    actions.extend(mark(mark_seen));
                    head(app, *title, subtitle.clone(), actions)
                })
            }))
            .child(pane(app, "changes", body))
            .into_element();
    }
    // A terminal keeps every label whole: the filters sit beside the title while the row holds
    // them, and take a row of their own under it when it does not.
    let width = signal(0.0_f32);
    view()
        .flex_1()
        .min_h(px(0.0))
        .on_layout(move |frame: &Bounds<Points>| width.set(frame.size.width.0))
        .child(dynamic(move || {
            let points = width.get();
            heading.with(|(title, subtitle, mark_seen, need)| {
                let beside = points <= 0.0 || points / term::cells(1.0).0 >= *need;
                if beside {
                    let mut actions = vec![filters(app, changes)];
                    actions.extend(mark(mark_seen));
                    return head(app, *title, subtitle.clone(), actions);
                }
                view()
                    .child(head(
                        app,
                        *title,
                        subtitle.clone(),
                        mark(mark_seen).into_iter().collect(),
                    ))
                    .child(
                        view()
                            .flex_row()
                            .px(term::cells(1.0))
                            .child(filters(app, changes)),
                    )
                    .child(term::hrule())
                    .into_element()
            })
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
        .map_or(0.0, |button| 2.0 + count(&button.label) + 2.0);
    1.0 + count(title) + 2.0 + SUBTITLE_CELLS + 2.0 + filters + mark + 1.0
}

/// The model's filters as a segmented control: picking one narrows the feed.
fn filters(app: AppState, changes: Memo<Changes>) -> AnyElement {
    let options: Vec<(ChangeFilter, &'static str)> = changes.with_untracked(|changes| {
        changes
            .filters
            .iter()
            .map(|filter| (filter.filter, filter.label))
            .collect()
    });
    seg(
        app,
        &options,
        move || app.work.with(|state| state.filter),
        move |filter| act(app, Action::Filter(filter)),
        "seg-changes",
    )
}

/// The recent events, the seen line, the dimmed history, the older-page control and the notes.
fn feed(app: AppState, changes: &Changes) -> AnyElement {
    let form = app.form;
    let notice = changes
        .notice
        .as_ref()
        .filter(|_| changes.recent.is_empty() && changes.history.is_empty());
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
    // An empty loaded page with a notice shows the notice in the feed's place, so the older-page
    // control below stays reachable: an older page may hold what the filter matches.
    let mut events: Vec<AnyElement> = Vec::new();
    if let Some(notice) = notice {
        events.push(quiet(app, notice.clone()));
    } else {
        events.extend(recent);
        events.push(seen_line(app, changes.seen_line.clone()));
        events.extend(history);
    }
    view()
        .child(
            view()
                .py(px(if form == Form::Terminal { 0.0 } else { 4.0 }))
                .when(form == Form::Desktop, |feed| feed.max_w(px(MEASURE)))
                .children(events),
        )
        .children(changes.older.as_ref().map(|older| {
            view()
                .flex_row()
                .px(px(if form == Form::Terminal { 8.0 } else { 16.0 }))
                .py(px(if form == Form::Terminal { 0.0 } else { 8.0 }))
                .child(pressable(app, older, false))
                .into_element()
        }))
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
