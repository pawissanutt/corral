//! A configured project's page, opened from the sidebar's PROJECTS section, the phone's More ›
//! Projects list or the palette. Its head names the project with its path and the two ways on
//! (Open in Now, Ask about this project); under it what the project holds now (open, running,
//! in review, needing the owner, PM scope and autopilot), then its open tasks by state beside
//! its plans, recent builds and goal burn-up. Desktop sets the tasks and the side column side by
//! side under the head; a terminal stacks them when narrow; a phone pushes the page and scrolls
//! it whole. Every row opens the record's own page. A project corrald no longer lists says so.
//! test ids: "project-<id>-page", "project-count-<slug>", "btn-open-in-now", "btn-ask-project",
//! "task-<id>", "plan-<id>", "build-<id>".

mod list;

pub use list::phone_list;

use nsh_core::prelude::*;

use crate::model::projects::{
    self, BUILDS_HEADING, FilterScreen, NO_BUILDS, NO_PLANS, PLANS_HEADING, ProjectBuild,
    ProjectCount, ProjectEntry, ProjectPage, ProjectPlan, ProjectTask, ProjectTaskGroup,
};
use crate::model::{Destination, Form, Hue, Id, Live};
use crate::screens::{PhonePage, timeline};
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with};
use crate::ui::kit::chip::chip;
use crate::ui::kit::marks::hue_ink;
use crate::ui::kit::notice;
use crate::ui::kit::page::{Width, columns, head, pane};
use crate::ui::kit::rows::{group_header, list_row, list_row_tall};
use crate::ui::kit::text::{dim, mono, plain};
use crate::ui::term::{self, cells};
use crate::ui::tokens::{FONT_MONO, FONT_TEXT, Token, color};

/// The side column of plans, builds and the burn-up beside the tasks.
const SIDE: f32 = 380.0;

type Data = Memo<Option<ProjectPage>>;

/// What a project row does wherever it is listed: a project opens its page; the Unassigned row
/// opens Now scoped to the tasks without a project.
pub fn open_entry(app: AppState, entry: &ProjectEntry) {
    match entry.project() {
        Some(id) => app.open_project(id.clone()),
        None => {
            app.set_project_filter(FilterScreen::Now, entry.filter.clone());
            app.go(Destination::Now);
        }
    }
}

fn data(app: AppState, id: Id) -> Data {
    computed(move || app.feed.with(|feed| projects::page(feed, &id)))
}

/// Desktop and terminal: the page over Now. Made again only when the project appears or goes;
/// its parts follow the feed themselves.
pub fn page(app: AppState, id: Id) -> AnyElement {
    let data = data(app, id.clone());
    let found = computed(move || data.with(Option::is_some));
    dynamic(move || {
        let found = found.get();
        let id = id.clone();
        untracked(move || match data.get_untracked().filter(|_| found) {
            Some(first) => wide(app, data, &first),
            None => view()
                .flex_1()
                .flex_col()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .child(head(app, id.to_string(), None, Vec::new()))
                .child(missing(app, &id))
                .into_element(),
        })
    })
}

fn wide(app: AppState, data: Data, first: &ProjectPage) -> AnyElement {
    let side = view()
        .child(plans(app, data))
        .child(builds(app, data))
        .child(burnup(app, first));
    view()
        .test_id(&format!("{}-page", first.id))
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(head(
            app,
            first.title.clone(),
            first.path.clone(),
            actions(app, first),
        ))
        .child(facts(app, data))
        .child(columns(
            app,
            vec![
                (Width::Fill, pane(app, "project-tasks", tasks(app, data))),
                (
                    Width::Fixed(SIDE),
                    pane(app, "project-side", side.into_element()),
                ),
            ],
            // The side column is the one a stacked terminal keeps under the tasks, not drops.
            1,
        ))
        .into_element()
}

/// The phone's pushed page: the path, the figures, the two actions, then every part in one
/// scroll. The page carries its own Ask, so the floating one steps aside.
pub fn project_phone(app: AppState, id: Id) -> PhonePage {
    let data = data(app, id.clone());
    let found = computed(move || data.with(Option::is_some));
    let title = id.to_string();
    let body = dynamic(move || {
        let found = found.get();
        let id = id.clone();
        untracked(move || match data.get_untracked().filter(|_| found) {
            Some(first) => view()
                .test_id(&format!("{}-page", first.id))
                .children(first.path.clone().map(|path| {
                    mono(app, path)
                        .px(px(18.0))
                        .pt(px(2.0))
                        .text_color(color(Token::Ink3))
                        .into_element()
                }))
                .child(facts(app, data))
                .child(
                    view()
                        .flex_row()
                        .flex_wrap()
                        .gap(px(8.0))
                        .px(px(18.0))
                        .pb(px(14.0))
                        .children(actions(app, &first)),
                )
                .child(tasks(app, data))
                .child(plans(app, data))
                .child(builds(app, data))
                .child(burnup(app, &first))
                .into_element(),
            None => missing(app, &id),
        })
    });
    PhonePage {
        title,
        body,
        trailing: None,
        bar: None,
        ask: false,
        scroll: true,
    }
}

/// An unknown project: the honest notice and the way back to Now.
fn missing(app: AppState, id: &Id) -> AnyElement {
    let (heading, why) = projects::not_found(id);
    notice::empty(
        app,
        heading,
        why,
        Some(button(
            app,
            format!("Open {}", Destination::Now.label()),
            ButtonKind::Ghost,
            "btn-open-now",
            move || app.go(Destination::Now),
        )),
    )
}

fn actions(app: AppState, page: &ProjectPage) -> Vec<AnyElement> {
    let now = page.project.clone();
    let ask = page.project.clone();
    vec![
        button(
            app,
            page.open_now_label,
            ButtonKind::Ghost,
            "btn-open-in-now",
            move || app.open_project_in_now(now.clone()),
        ),
        button_with(
            app,
            page.ask_label,
            ButtonKind::Primary,
            ButtonOptions {
                icon: Some(Icon::Ask),
                ..ButtonOptions::default()
            },
            "btn-ask-project",
            move || app.ask_project(ask.clone()),
        ),
    ]
}

/// What the project holds now: the four figures, then PM scope and autopilot as chips.
fn facts(app: AppState, data: Data) -> AnyElement {
    let parts =
        computed(move || data.with(|page| page.as_ref().map(|p| (p.counts(), p.state_chips()))));
    let form = app.form;
    dynamic(move || {
        let parts = parts.get();
        untracked(move || {
            parts.map(|(counts, chips)| {
                let figures = counts.into_iter().map(|count| figure(app, count));
                let chips = chips.into_iter().map(|(label, hue)| chip(app, label, hue));
                match form {
                    Form::Terminal => view()
                        .child(
                            view()
                                .flex_row()
                                .flex_wrap()
                                .px(cells(1.0))
                                .gap_x(cells(2.0))
                                .children(figures)
                                .children(chips),
                        )
                        .child(term::hrule())
                        .into_element(),
                    Form::Desktop | Form::Phone => {
                        let phone = form == Form::Phone;
                        view()
                            .flex_row()
                            .flex_wrap()
                            .items_center()
                            .gap_x(px(18.0))
                            .gap_y(px(8.0))
                            .px(px(if phone { 18.0 } else { 24.0 }))
                            .py(px(12.0))
                            .when(!phone, |v| v.border_b_1().border_color(color(Token::Rule)))
                            .children(figures)
                            .child(view().flex_row().gap(px(6.0)).children(chips))
                            .into_element()
                    }
                }
            })
        })
    })
}

/// One figure and its name; an unknown one (an older corrald) is a dash, never a zero.
fn figure(app: AppState, count: ProjectCount) -> AnyElement {
    let form = app.form;
    let lit = count.value.unwrap_or(0) > 0 && count.hue != Hue::Neutral;
    let value = count
        .value
        .map_or_else(|| "—".to_owned(), |n| n.to_string());
    let ink = if lit {
        hue_ink(count.hue)
    } else {
        color(Token::Ink)
    };
    let figures = text(value).number_of_lines(1u32).text_color(ink);
    let name = text(count.label)
        .number_of_lines(1u32)
        .text_color(color(Token::Ink3));
    let row = view()
        .test_id(&format!("project-count-{}", count.slug))
        .flex_row()
        .items_center();
    if form == Form::Terminal {
        return row
            .child(figures.font_weight(FontWeight::BOLD))
            .child(term::space(1.0))
            .child(name)
            .into_element();
    }
    row.gap(px(5.0))
        .child(
            figures
                .font_family(FONT_MONO)
                .tabular_nums()
                .text_size(px(15.0))
                .font_weight(FontWeight::SEMIBOLD),
        )
        .child(name.font_family(FONT_TEXT).text_size(px(12.0)))
        .into_element()
}

/// The open tasks under their state's header, or the empty state while the project has none.
fn tasks(app: AppState, data: Data) -> AnyElement {
    let shown =
        computed(move || data.with(|page| page.as_ref().map(|p| (p.tasks.clone(), p.empty))));
    dynamic(move || {
        let shown = shown.get();
        untracked(move || {
            shown.map(|(groups, empty)| match empty {
                Some(empty) => notice::empty(app, empty.heading, empty.body, None),
                None => view()
                    .pb(px(if app.form == Form::Terminal {
                        0.0
                    } else {
                        12.0
                    }))
                    .children(groups.into_iter().map(|group| task_group(app, group)))
                    .into_element(),
            })
        })
    })
}

fn task_group(app: AppState, group: ProjectTaskGroup) -> AnyElement {
    let count = u32::try_from(group.tasks.len()).unwrap_or(u32::MAX);
    view()
        .child(group_header(app, group.label, Some(count), group.hue, None))
        .children(group.tasks.into_iter().map(|task| task_row(app, task)))
        .into_element()
}

/// A task: its id, title and state on one line; on a phone the title over the id and state.
fn task_row(app: AppState, task: ProjectTask) -> AnyElement {
    let label = format!("{}, {}, {}", task.id, task.title, task.state);
    let test_id = format!("task-{}", task.id);
    let id = task.id.clone();
    let open = move || app.open_live(Live::Task(id.clone()));
    if app.form == Form::Phone {
        return list_row_tall(
            app,
            label,
            test_id,
            || false,
            open,
            vec![
                view()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap(px(2.0))
                    .child(plain(app, task.title).truncate())
                    .child(dim(app, format!("{} · {}", task.id, task.state)).truncate())
                    .into_element(),
            ],
        );
    }
    list_row(
        app,
        label,
        test_id,
        || false,
        open,
        vec![
            id_cell(app, &task.id),
            plain(app, task.title)
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .into_element(),
            dim(app, task.state)
                .flex_shrink(1.0)
                .min_w(px(0.0))
                .max_w(px(220.0))
                .truncate()
                .into_element(),
        ],
    )
}

/// A record id in its own column, so titles line up.
fn id_cell(app: AppState, id: &Id) -> AnyElement {
    let width = if app.form == Form::Terminal {
        cells(7.0)
    } else {
        px(58.0)
    };
    mono(app, id.to_string())
        .w(width)
        .flex_shrink_0()
        .text_color(color(Token::Ink2))
        .into_element()
}

fn plans(app: AppState, data: Data) -> AnyElement {
    let shown = computed(move || data.with(|page| page.as_ref().map(|p| p.plans.clone())));
    dynamic(move || {
        let shown = shown.get();
        untracked(move || {
            shown.map(|plans| {
                let rows: Vec<AnyElement> =
                    plans.into_iter().map(|plan| plan_row(app, plan)).collect();
                listed(app, PLANS_HEADING, rows, NO_PLANS)
            })
        })
    })
}

fn plan_row(app: AppState, plan: ProjectPlan) -> AnyElement {
    let id = plan.id.clone();
    list_row(
        app,
        format!("{}, {}", plan.id, plan.title),
        format!("plan-{}", plan.id),
        || false,
        move || app.open_live(Live::Plan(id.clone())),
        vec![
            id_cell(app, &plan.id),
            plain(app, plan.title)
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .into_element(),
        ],
    )
}

fn builds(app: AppState, data: Data) -> AnyElement {
    let shown = computed(move || data.with(|page| page.as_ref().map(|p| p.builds.clone())));
    dynamic(move || {
        let shown = shown.get();
        untracked(move || {
            shown.map(|builds| {
                let rows: Vec<AnyElement> = builds
                    .into_iter()
                    .map(|build| build_row(app, build))
                    .collect();
                listed(app, BUILDS_HEADING, rows, NO_BUILDS)
            })
        })
    })
}

fn build_row(app: AppState, build: ProjectBuild) -> AnyElement {
    let id = build.id.clone();
    list_row(
        app,
        format!("{}, {}", build.title, build.state),
        format!("build-{}", build.id),
        || false,
        move || app.open_live(Live::Build(id.clone())),
        vec![
            plain(app, build.title)
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .into_element(),
            chip(app, build.state, Hue::Neutral),
        ],
    )
}

/// A side list: its header with the count, then its rows, or the line that says there are none.
fn listed(
    app: AppState,
    heading: &'static str,
    rows: Vec<AnyElement>,
    none: &'static str,
) -> AnyElement {
    let count = u32::try_from(rows.len()).unwrap_or(u32::MAX);
    let form = app.form;
    let header = group_header(app, heading, Some(count), Hue::Neutral, None);
    if !rows.is_empty() {
        return view().child(header).children(rows).into_element();
    }
    let line = dim(app, none);
    let line = match form {
        Form::Desktop => line.pl(px(16.0)).pr(px(14.0)).py(px(6.0)),
        Form::Phone => line.px(px(18.0)).py(px(10.0)),
        Form::Terminal => line.px(cells(1.0)),
    };
    view().child(header).child(line).into_element()
}

/// The project's goal burn-up, requested while the page shows.
fn burnup(app: AppState, page: &ProjectPage) -> AnyElement {
    let chart = timeline::scoped_burnup(app, page.burnup_request.clone());
    match app.form {
        Form::Terminal => view()
            .px(cells(1.0))
            .child(term::blank())
            .child(chart)
            .into_element(),
        Form::Desktop => view()
            .px(px(16.0))
            .pt(px(14.0))
            .pb(px(20.0))
            .child(chart)
            .into_element(),
        Form::Phone => view()
            .px(px(18.0))
            .pt(px(14.0))
            .pb(px(20.0))
            .child(chart)
            .into_element(),
    }
}
