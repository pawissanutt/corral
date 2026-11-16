//! Builds: the published records of interactive build sessions, beside the selected build's
//! detail (desktop and terminal), or the list under More with each build's pushed page
//! (phone). A build is finished-or-in-flight work the owner did in a session, not a task: its
//! rows carry no rail, actor or task commands.
//!
//! Everything shown comes from the served feed (corrald's, or the explicitly chosen sample
//! preview) through `model::builds`: `live_list_filtered` in the screen's own project scope for
//! the rows and `detail_of` for the detail.
//! Presses run `builds::reduce`; `live.rs` keeps the shown build's detail loaded and current.

mod detail;
mod live;

pub(crate) use detail::line_view;

use nsh_core::prelude::*;

use crate::model::builds::{self, Action, BuildList, ListNotice, ListRow, ListSection};
use crate::model::projects::FilterScreen;
use crate::model::text::span;
use crate::model::{Form, Hue, Id};
use crate::screens::PhonePage;
use crate::state::AppState;
use crate::ui::kit::chip::chip;
use crate::ui::kit::marks::dot;
use crate::ui::kit::notice::{empty, error, loading, notice};
use crate::ui::kit::page::{Width, columns, group_head, pane};
use crate::ui::kit::project_filter::project_filter;
use crate::ui::kit::rows::{list_row, list_row_tall, measured, project_chip};
use crate::ui::kit::text::{dim, rich_small};
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// The list column beside the detail, and its compact width on a desktop screen narrower than
/// `LIST_BESIDE`, as Plans sets its list.
const LIST: f32 = 380.0;
const LIST_COMPACT: f32 = 280.0;
const LIST_BESIDE: f32 = 800.0;

/// The Builds destination. Desktop: the list beside the detail, the list narrower on a small
/// screen. Terminal: the same two panes (stacked on a narrow grid by the kit's columns). Phone:
/// the list alone (the shell draws the pushed More page's head); a row pushes its build.
/// test ids: "builds-list", "build-<id>" (rows), "build-detail", "build-section-<key>",
/// "build-override", "btn-retry".
pub fn screen(app: AppState) -> AnyElement {
    if untracked(|| app.daemon()) && app.form != Form::Phone {
        live::keep_selected_loaded(app);
    }
    match app.form {
        Form::Phone => pane(app, "builds-list", list(app)),
        Form::Terminal => columns(
            app,
            vec![
                (Width::Fixed(LIST), pane(app, "builds-list", list(app))),
                (Width::Fill, detail::pane_view(app)),
            ],
            1,
        ),
        Form::Desktop => {
            // Measured, not assumed from the form: a small window beside the sidebar keeps the
            // detail usable by narrowing the list.
            let room = signal(None::<f32>);
            let compact = computed(move || room.get().is_some_and(|width| width < LIST_BESIDE));
            view()
                .flex_1()
                .flex_row()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .on_layout(move |frame: &Bounds<Points>| room.set(Some(frame.size.width.0)))
                .child(dynamic(move || {
                    let list_width = if compact.get() { LIST_COMPACT } else { LIST };
                    untracked(move || {
                        columns(
                            app,
                            vec![
                                (
                                    Width::Fixed(list_width),
                                    pane(app, "builds-list", list(app)),
                                ),
                                (Width::Fill, detail::pane_view(app)),
                            ],
                            1,
                        )
                    })
                }))
                .into_element()
        }
    }
}

/// A build's pushed phone page (`Live::Build`): its title in the bar, the detail as the body;
/// the detail is kept current while the page is up.
pub fn live_build_phone(app: AppState, id: Id) -> PhonePage {
    if untracked(|| app.daemon()) {
        live::keep_loaded(app, id.clone());
    }
    let title = untracked(|| app.feed.with(|feed| builds::page_title(feed, &id)));
    PhonePage {
        title,
        body: detail::phone_body(app, id),
        trailing: None,
        bar: None,
        ask: true,
        scroll: true,
    }
}

fn content(app: AppState) -> BuildList {
    let env = app.env();
    let selection = app.selection();
    let filter = app.project_filter(FilterScreen::Builds);
    app.feed
        .with(|feed| builds::live_list_filtered(&selection, feed, env, &filter))
}

fn press(app: AppState, action: Action) {
    app.run(builds::reduce(action));
}

/// The grouped list, the destination's keyboard list (`AppState::list_keys`): the project scope,
/// the loading, empty, error or stale notice where corrald says so, then each outcome's head and
/// rows.
fn list(app: AppState) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let rows = dynamic(move || {
        let content = content(app);
        let lead = content
            .notice
            .as_ref()
            .map(|notice_view| match notice_view {
                ListNotice::Loading => view()
                    .px(px(if terminal { 0.0 } else { 16.0 }))
                    .py(px(8.0))
                    .child(loading(
                        app,
                        builds::LIST_LOADING,
                        &[(0.7, 12.0), (0.5, 10.0), (0.6, 12.0)],
                    ))
                    .into_element(),
                ListNotice::Empty { heading, body } => empty(app, *heading, *body, None),
                ListNotice::Error(line) => error(app, &vec![span(line.as_str())], None),
                ListNotice::Stale(line) => view()
                    .px(px(if terminal { 0.0 } else { 12.0 }))
                    .pb(px(6.0))
                    .child(notice(app, Hue::Held, &vec![span(line.as_str())], None))
                    .into_element(),
            });
        view()
            .children(lead)
            .children(
                content
                    .sections
                    .iter()
                    .map(|section| group(app, section))
                    .collect::<Vec<_>>(),
            )
            .into_element()
    });
    view()
        .element_ref(&app.list_keys)
        .pt(px(if app.form == Form::Desktop { 6.0 } else { 0.0 }))
        .pb(px(if terminal { 0.0 } else { 14.0 }))
        .role(AccessibilityRole::List)
        .label("Builds")
        .child(project_filter(app, FilterScreen::Builds))
        .child(rows)
        .into_element()
}

fn group(app: AppState, section: &ListSection) -> AnyElement {
    view()
        .child(group_head(
            app,
            section.label,
            Some(section.count.to_string()),
            section.hue,
            None,
            None,
        ))
        .children(
            section
                .rows
                .iter()
                .map(|row| build_row(app, row))
                .collect::<Vec<_>>(),
        )
        .into_element()
}

fn build_row(app: AppState, row: &ListRow) -> AnyElement {
    let action = row.action.clone();
    let selected = row.selected;
    let id = format!("build-{}", row.id);
    let label = match &row.project_chip {
        Some(project) => format!("{}, {project}", row.label),
        None => row.label.clone(),
    };
    let on_press = move || press(app, action.clone());
    match app.form {
        Form::Desktop => list_row_tall(
            app,
            label,
            id,
            move || selected,
            on_press,
            desktop_cells(app, row),
        ),
        Form::Phone => list_row(
            app,
            label,
            id,
            move || selected,
            on_press,
            phone_cells(app, row),
        ),
        Form::Terminal => list_row(
            app,
            label,
            id,
            move || selected,
            on_press,
            terminal_cells(app, row),
        ),
    }
}

/// The title cut to one line, the project and override chips after it.
fn title_line(app: AppState, row: &ListRow, size: f32, weight: FontWeight) -> AnyElement {
    let words = text(row.title.clone())
        .flex_shrink(1.0)
        .min_w(px(0.0))
        .truncate()
        .font_weight(weight)
        .text_color(color(Token::Ink));
    let words = if app.form == Form::Terminal {
        words
    } else {
        words.font_family(FONT_TEXT).text_size(px(size))
    };
    let room = signal(0.0_f32);
    // The override chip beside the project's takes its own room from the title.
    let reserved = row
        .flag
        .map_or(0.0, |(label, _)| label.chars().count() as f32 + 3.0);
    let tag = row
        .project_chip
        .clone()
        .map(|project| project_chip(app, project, room, reserved));
    measured(
        app,
        view()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .min_w(px(0.0))
            // A terminal row measures this line, so it takes its width from the cell around it.
            .when(app.form == Form::Terminal, |line| line.w_full()),
        room,
    )
    .child(words)
    .children(tag)
    .children(row.flag.map(|(label, hue)| chip(app, label, hue)))
    .into_element()
}

/// The row's quiet project chip, while the list spans projects.
fn project(app: AppState, row: &ListRow) -> Option<AnyElement> {
    row.project_chip
        .clone()
        .map(|project| chip(app, project, Hue::Neutral))
}

/// Updated time and cost, in mono; an unknown cost in the quiet ink, never a zero. Each its own
/// element: a terminal row sets them a Cell apart, the desktop row stacks them.
fn figures(app: AppState, row: &ListRow, size: f32) -> Vec<AnyElement> {
    let figure = |words: String, ink: Token| {
        let words = text(words)
            .flex_shrink_0()
            .number_of_lines(1u32)
            .text_color(color(ink));
        if app.form == Form::Terminal {
            words.into_element()
        } else {
            words
                .font_family(FONT_MONO)
                .tabular_nums()
                .text_size(px(size))
                .into_element()
        }
    };
    let cost_ink = if row.cost_known {
        Token::Ink2
    } else {
        Token::Ink3
    };
    [
        (!row.updated.is_empty()).then(|| figure(row.updated.clone(), Token::Ink3)),
        Some(figure(row.cost.clone(), cost_ink)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Desktop: the dot, then the title over the outcome and facts (and the override's callout),
/// the update and cost at the trailing edge.
fn desktop_cells(app: AppState, row: &ListRow) -> Vec<AnyElement> {
    let mut facts = vec![span(format!("{} · ", row.state)).hue(row.hue)];
    facts.extend(row.facts.iter().cloned());
    vec![
        view()
            .w(px(14.0))
            .flex_shrink_0()
            .child(dot(app, row.hue))
            .into_element(),
        view()
            .flex_1()
            .min_w(px(0.0))
            .gap(px(1.0))
            .child(title_line(app, row, 13.0, FontWeight::NORMAL))
            .child(rich_small(app, &facts))
            .children(row.alert.as_ref().map(|alert| rich_small(app, alert)))
            .into_element(),
        view()
            .flex_shrink_0()
            .items_end()
            .gap(px(2.0))
            .children(figures(app, row, 11.0))
            .into_element(),
    ]
}

/// Phone: the dot, the title free to take two lines, then the outcome and facts, the
/// override's callout, and the update and cost.
fn phone_cells(app: AppState, row: &ListRow) -> Vec<AnyElement> {
    let title = view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(
            text(row.title.clone())
                .flex_1()
                .min_w(px(0.0))
                .number_of_lines(2u32)
                .font_family(FONT_TEXT)
                .text_size(px(16.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(Token::Ink)),
        )
        .children(project(app, row))
        .children(row.flag.map(|(label, hue)| chip(app, label, hue)));
    let mut facts = vec![span(format!("{} · ", row.state)).hue(row.hue)];
    facts.extend(row.facts.iter().cloned());
    let when = [row.updated.as_str(), row.cost.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    vec![
        view()
            .w(px(12.0))
            .flex_shrink_0()
            .self_start()
            .pt(px(5.0))
            .child(dot(app, row.hue))
            .into_element(),
        view()
            .flex_1()
            .min_w(px(0.0))
            .gap(px(3.0))
            .child(title)
            .child(rich_small(app, &facts))
            .children(row.alert.as_ref().map(|alert| rich_small(app, alert)))
            .child(dim(app, when))
            .into_element(),
    ]
}

/// Terminal: one line of Cells: dot, title (and the override chip), update and cost. The
/// facts are the detail's to say.
fn terminal_cells(app: AppState, row: &ListRow) -> Vec<AnyElement> {
    let mut cells = vec![
        dot(app, row.hue),
        view()
            .flex_1()
            .min_w(px(0.0))
            .child(title_line(app, row, 13.0, FontWeight::NORMAL))
            .into_element(),
    ];
    cells.extend(figures(app, row, 13.0));
    cells
}
