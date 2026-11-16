//! A configured project's page, opened from the sidebar's PROJECTS section, the phone's More ›
//! Projects list or the palette: Now scoped to one project, beside the project's own facts.
//!
//! Desktop: the head names the project with its counts and the two ways on (Open in Now, Ask).
//! The main column is the loop band (a stage opens its destination in this project's scope),
//! the owner moves and moving tasks in Now's own rows, the project's plans and sessions, Waiting,
//! Closed, then its recent builds and goal burn-up. The side column holds the facts (path, PM
//! scope, autopilot, open and running) over the project's digest of what changed. It stands
//! beside the list while the page has Now's room for both; narrower, it follows the list in its
//! scroll. Terminal: the same, by Now's terminal rule. Phone: the pushed page scrolls whole: the
//! counts, the facts, the two actions, the sections in Now's phone rows, plans, sessions,
//! builds, the burn-up and the digest.
//!
//! The page has no selection: every row opens its record (a task on Now, a question, plan,
//! review or session on its page). A project corrald no longer lists says so. Every part renders
//! `projects::page` or `projects::project_now`; the screen adds no words. test ids:
//! "project-<id>-page", "project-count-<slug>", "btn-open-in-now", "btn-ask-project",
//! "task-<id>", "plan-<id>", "agent-<id>", "build-<id>".

mod list;

pub use list::phone_list;

use corral_protocol::detail::DetailRef;
use nsh_core::prelude::*;

use crate::model::projects::{
    self, BUILDS_HEADING, FACTS_LABEL, FilterScreen, NO_BUILDS, PLANS_HEADING, PLANS_HINT,
    ProjectBuild, ProjectEmpty, ProjectEntry, ProjectFact, ProjectPage, ProjectPlan,
    ProjectSession, SESSIONS_HEADING, SESSIONS_HINT,
};
use crate::model::work::{NowPage, NowRow, NowSection, Press};
use crate::model::{Destination, Form, Hue, Id, Live};
use crate::screens::now::closed::closed_section;
use crate::screens::now::list::{board_status, loading, section_head, waiting};
use crate::screens::now::parts::{dim_when, go_scoped, quiet, stale};
use crate::screens::now::rows::{self, Fit, ask_row, task_row};
use crate::screens::now::{BESIDE, LIST_COLUMNS, SIDE, SIDE_COLUMNS, side};
use crate::screens::{PhonePage, timeline};
use crate::state::AppState;
use crate::ui::icons::Icon;
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button, button_with};
use crate::ui::kit::chip::chip;
use crate::ui::kit::glance::{loop_band, page_head, stats};
use crate::ui::kit::hint::{hint_on, hinted};
use crate::ui::kit::notice;
use crate::ui::kit::rows::{group_header, list_row};
use crate::ui::kit::text::{dim, plain};
use crate::ui::term::{self, cells};
use crate::ui::tokens::{Token, color, text_size};

const RING: f32 = 2.0;

type Data = Memo<Option<ProjectPage>>;

/// What the page's columns are made from: the project's own facts, Now in its scope, and the
/// burn-up it requests.
#[derive(Clone)]
struct Parts {
    data: Data,
    now: Memo<NowPage>,
    burnup: DetailRef,
}

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

/// Now's page in the project's scope, tracked: the head's counts, the band, the rows, Waiting,
/// Closed and the digest.
fn scoped(app: AppState, project: Id) -> Memo<NowPage> {
    let form = app.form;
    computed(move || {
        app.feed.with(|feed| {
            app.work
                .with(|state| projects::project_now(state, feed, form, &project))
        })
    })
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
                .child(page_head(app, id.to_string(), &[], Vec::new()))
                .child(missing(app, &id))
                .into_element(),
        })
    })
}

fn wide(app: AppState, data: Data, first: &ProjectPage) -> AnyElement {
    let now = scoped(app, first.project.clone());
    let parts = Parts {
        data,
        now,
        burnup: first.burnup_request.clone(),
    };
    let counts = computed(move || now.with(|page| page.stats.clone()));
    let title = first.title.clone();
    let head_page = first.clone();
    let head = dynamic(move || {
        counts.with(|counts| page_head(app, title.clone(), counts, actions(app, &head_page)))
    });
    let body = match app.form {
        Form::Terminal => terminal_body(app, parts),
        Form::Desktop | Form::Phone => desktop_body(app, parts),
    };
    // A terminal keeps a box's height only while it grows into its parent.
    view()
        .test_id(&format!("{}-page", first.id))
        .size_full()
        .flex_grow(1.0)
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .bg(color(Token::Bg))
        .child(head)
        .child(body)
        .into_element()
}

/// Desktop: measured, not assumed from the window, since the sidebar and Ask take their share.
fn desktop_body(app: AppState, parts: Parts) -> AnyElement {
    let room = signal(None::<f32>);
    let beside = computed(move || room.get().is_none_or(|width| width >= BESIDE));
    view()
        .flex_1()
        .flex_col()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .on_layout(move |frame: &Bounds<Points>| room.set(Some(frame.size.width.0)))
        .child(dynamic(move || {
            let parts = parts.clone();
            if beside.get() {
                untracked(move || {
                    view()
                        .flex_1()
                        .flex_row()
                        .min_h(px(0.0))
                        .min_w(px(0.0))
                        .child(main_scroll(app, &parts, None))
                        .child(
                            view()
                                .w(px(SIDE))
                                .flex_shrink_0()
                                .flex_col()
                                .min_h(px(0.0))
                                .border_l_1()
                                .border_l_color(color(Token::Rule))
                                .child(side_scroll(app, &parts)),
                        )
                        .into_element()
                })
            } else {
                untracked(move || main_scroll(app, &parts, Some(side_below(app, &parts))))
            }
        }))
        .into_element()
}

/// Terminal: the side column beside the list while the grid holds both at their least widths
/// and the line between; otherwise it follows the list in its scroll.
fn terminal_body(app: AppState, parts: Parts) -> AnyElement {
    let beside = computed(move || term::page_columns(app) >= LIST_COLUMNS + 1.0 + SIDE_COLUMNS);
    view()
        .flex_1()
        .flex_grow(1.0)
        .flex_col()
        .min_h(px(0.0))
        .child(dynamic(move || {
            let parts = parts.clone();
            if beside.get() {
                untracked(move || {
                    view()
                        .flex_1()
                        .flex_row()
                        .min_h(px(0.0))
                        .child(main_scroll(app, &parts, None))
                        .child(term::vrule())
                        .child(
                            view()
                                .w(cells(SIDE_COLUMNS))
                                .flex_shrink_0()
                                .flex_col()
                                .min_h(px(0.0))
                                .child(side_scroll(app, &parts)),
                        )
                        .into_element()
                })
            } else {
                untracked(move || main_scroll(app, &parts, Some(side_below(app, &parts))))
            }
        }))
        .into_element()
}

/// The main column in its own scroll, dimmed while the board is stale.
fn main_scroll(app: AppState, parts: &Parts, below: Option<AnyElement>) -> AnyElement {
    let main = view()
        .child(board_status(app, parts.now))
        .child(band(app, parts.now))
        .child(work_list(app, parts.data, parts.now))
        .child(builds(app, parts.data))
        .child(burnup(app, parts.burnup.clone()));
    scroll()
        .test_id("project-main")
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(dim_when(view(), stale(app)).child(main).children(below))
        .into_element()
}

/// The facts over the digest, in their own scroll.
fn side_scroll(app: AppState, parts: &Parts) -> AnyElement {
    scroll()
        .test_id("project-side")
        .flex_1()
        .min_h(px(0.0))
        .child(dim_when(view(), stale(app)).child(side_column(app, parts)))
        .into_element()
}

/// The side column under the list in the one scroll, set off by a line.
fn side_below(app: AppState, parts: &Parts) -> AnyElement {
    let column = side_column(app, parts);
    match app.form {
        Form::Terminal => view().child(term::hrule()).child(column).into_element(),
        Form::Desktop | Form::Phone => view()
            .border_t_1()
            .border_t_color(color(Token::Rule))
            .child(column)
            .into_element(),
    }
}

fn side_column(app: AppState, parts: &Parts) -> AnyElement {
    view()
        .test_id("project-facts")
        .child(facts(app, parts.data))
        .child(side::panel(app, parts.now))
        .into_element()
}

/// The loop band in the project's scope: a stage opens its destination with this project as
/// that destination's filter. A phone draws none.
fn band(app: AppState, now: Memo<NowPage>) -> AnyElement {
    let shown = computed(move || now.with(|page| page.loop_band.clone()));
    let band = dynamic(move || {
        shown.with(|band| {
            loop_band(app, band, move |stage| {
                if let Some(to) = stage.to {
                    go_scoped(app, now, to);
                }
            })
        })
    });
    match app.form {
        Form::Terminal => view().px(cells(1.0)).child(band).into_element(),
        Form::Desktop | Form::Phone => view()
            .mx(px(20.0))
            .mt(px(14.0))
            .mb(px(2.0))
            .child(band)
            .into_element(),
    }
}

/// Owner moves and moving tasks, then plans and sessions, Waiting and Closed, in Now's rows.
/// It measures its width once, and the rows give way by it as Now's do.
fn work_list(app: AppState, data: Data, now: Memo<NowPage>) -> AnyElement {
    let form = app.form;
    let width = signal(0.0_f32);
    let fit = computed(move || rows::fit(form, width.get()));
    let moving = computed(move || {
        now.with(|page| {
            (
                page.loading_rows,
                page.notice.clone(),
                page.sections.clone(),
            )
        })
    });
    let empty = computed(move || data.with(|page| page.as_ref().and_then(|page| page.empty)));
    let records = computed(move || {
        data.with(|page| {
            page.as_ref()
                .map(|page| (page.plans.clone(), page.sessions.clone()))
                .unwrap_or_default()
        })
    });
    let held = computed(move || now.with(|page| page.waiting.clone()));
    view()
        .on_layout(move |frame: &Bounds<Points>| width.set(frame.size.width.0))
        .role(AccessibilityRole::List)
        .label(NowPage::LIST_LABEL)
        .pb(px(if form == Form::Terminal { 0.0 } else { 14.0 }))
        .child(dynamic(move || {
            let empty = empty.get();
            moving.with(|(rows, notice, sections)| {
                moving_rows(app, *rows, notice.as_deref(), sections, empty, fit)
            })
        }))
        .child(dynamic(move || {
            records.with(|(plans, sessions)| record_sections(app, plans, sessions))
        }))
        .child(dynamic(move || {
            held.with(|held| (held.total > 0).then(|| waiting(app, held, now, fit)))
        }))
        .child(closed_section(app, now))
        .into_element()
}

/// Needs you, Corral and Workers; the project's empty state while it has no open work.
fn moving_rows(
    app: AppState,
    loading_rows: usize,
    notice: Option<&str>,
    sections: &[NowSection],
    empty: Option<ProjectEmpty>,
    fit: Memo<Fit>,
) -> AnyElement {
    if loading_rows != 0 {
        return loading(app, loading_rows);
    }
    if let Some(empty) = empty.filter(|_| sections.is_empty()) {
        return notice::empty(app, empty.heading, empty.body, None);
    }
    // The page selects nothing: a row opens its record.
    let unselected = computed(|| false);
    let mut out = Vec::new();
    if let Some(words) = notice {
        out.push(quiet(app, words.to_owned()));
    }
    for section in sections {
        out.push(section_head(
            app,
            &section.label,
            section.count.to_string(),
            section.hue,
            &section.hint,
        ));
        for row in &section.rows {
            out.push(match row {
                NowRow::Ask(row) => ask_row(app, row, unselected, fit),
                NowRow::Task(row) => task_row(app, row, unselected, fit),
            });
        }
    }
    view().children(out).into_element()
}

/// The project's plans, then its sessions, each under its heading while it has any.
fn record_sections(
    app: AppState,
    plans: &[ProjectPlan],
    sessions: &[ProjectSession],
) -> AnyElement {
    let mut out = Vec::new();
    if !plans.is_empty() {
        out.push(section_head(
            app,
            PLANS_HEADING,
            plans.len().to_string(),
            Hue::Neutral,
            PLANS_HINT,
        ));
        out.extend(plans.iter().map(|plan| {
            record_row(
                app,
                Record {
                    test_id: format!("plan-{}", plan.id),
                    id: plan.id.as_str(),
                    title: &plan.title,
                    state: plan.state,
                    hue: plan.hue,
                    hint: "",
                    open: &plan.open,
                },
            )
        }));
    }
    if !sessions.is_empty() {
        out.push(section_head(
            app,
            SESSIONS_HEADING,
            sessions.len().to_string(),
            Hue::Neutral,
            SESSIONS_HINT,
        ));
        out.extend(sessions.iter().map(|session| {
            record_row(
                app,
                Record {
                    test_id: format!("agent-{}", session.id),
                    id: &session.shown_id,
                    title: &session.name,
                    state: session.state,
                    hue: session.hue,
                    hint: &session.doing,
                    open: &session.open,
                },
            )
        }));
    }
    view().children(out).into_element()
}

/// What a plan or session row shows and opens.
struct Record<'a> {
    test_id: String,
    /// The id the row shows; the test id names the record itself.
    id: &'a str,
    title: &'a str,
    state: &'static str,
    hue: Hue,
    hint: &'a str,
    open: &'a Press,
}

/// A plan or session (`.srow`): the mono id in the task rows' id column, the title in semibold,
/// the state chip in its hue, on one line on every form; a press opens the record.
fn record_row(app: AppState, record: Record<'_>) -> AnyElement {
    let form = app.form;
    let label = format!("{}, {}, {}", record.id, record.title, record.state);
    let content = vec![
        rows::column(
            form,
            rows::ID,
            false,
            rows::figures(form, record.id.to_owned(), 12.0, Token::Ink3),
        )
        .into_element(),
        rows::words(
            form,
            record.title.to_owned(),
            text_size(form),
            color(Token::Ink),
        )
        .font_weight(FontWeight::SEMIBOLD)
        .flex_1()
        .min_w(px(0.0))
        .into_element(),
        chip(app, record.state, record.hue),
    ];
    let unselected = computed(|| false);
    let line = rows::pressable_line(
        app,
        label,
        record.test_id,
        unselected,
        record.open.clone(),
        content,
        true,
    );
    let line = match form {
        Form::Desktop => line
            .px(px(rows::DESK_PAD))
            .border_b_1()
            .border_b_color(color(Token::Rule)),
        Form::Phone => line
            .items_center()
            .gap(px(10.0))
            .min_h(px(48.0))
            .px(px(rows::PHONE_PAD))
            .py(px(10.0))
            .border_b_1()
            .border_b_color(color(Token::Rule)),
        Form::Terminal => line,
    };
    hint_on(app, line, record.hint.to_owned()).into_element()
}

/// The phone's pushed page: the counts, the facts, the two actions, then every part in one
/// scroll, the digest last. The page carries its own Ask, so the floating one steps aside.
pub fn project_phone(app: AppState, id: Id) -> PhonePage {
    let data = data(app, id.clone());
    let found = computed(move || data.with(Option::is_some));
    let title = id.to_string();
    let body = dynamic(move || {
        let found = found.get();
        let id = id.clone();
        untracked(move || match data.get_untracked().filter(|_| found) {
            Some(first) => phone_body(app, data, &first),
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

fn phone_body(app: AppState, data: Data, first: &ProjectPage) -> AnyElement {
    let now = scoped(app, first.project.clone());
    let counts = computed(move || now.with(|page| page.stats.clone()));
    let head = dynamic(move || {
        counts.with(|counts| {
            (!counts.is_empty()).then(|| {
                view()
                    .flex_row()
                    .px(px(18.0))
                    .pb(px(10.0))
                    .child(stats(app, counts))
                    .into_element()
            })
        })
    });
    view()
        .test_id(&format!("{}-page", first.id))
        .child(head)
        .child(facts(app, data))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .gap(px(8.0))
                .px(px(18.0))
                .py(px(14.0))
                .children(actions(app, first)),
        )
        .child(
            dim_when(view(), stale(app))
                .child(board_status(app, now))
                .child(work_list(app, data, now)),
        )
        .child(builds(app, data))
        .child(burnup(app, first.burnup_request.clone()))
        .child(
            view()
                .border_t_1()
                .border_t_color(color(Token::Rule))
                .child(side::panel(app, now)),
        )
        .into_element()
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
        hinted(
            app,
            button_with(
                app,
                page.ask_label,
                ButtonKind::Ghost,
                ButtonOptions {
                    icon: Some(Icon::Ask),
                    ..ButtonOptions::default()
                },
                "btn-ask-project",
                move || app.ask_project(ask.clone()),
            ),
            page.ask_hint,
        ),
    ]
}

/// The project's facts (`dl.facts`): each label in quiet ink in its column, the value beside
/// it with its sentence on hover or focus. An unknown fact says so.
fn facts(app: AppState, data: Data) -> AnyElement {
    let shown = computed(move || {
        data.with(|page| {
            page.as_ref()
                .map(|page| page.facts.clone())
                .unwrap_or_default()
        })
    });
    dynamic(move || {
        shown.with(|facts| {
            let frame = match app.form {
                Form::Desktop | Form::Phone => view()
                    .px(px(18.0))
                    .py(px(14.0))
                    .gap(px(6.0))
                    .border_b_1()
                    .border_b_color(color(Token::Rule)),
                Form::Terminal => view().px(cells(1.0)),
            };
            frame
                .role(AccessibilityRole::List)
                .label(FACTS_LABEL)
                .children(
                    facts
                        .iter()
                        .map(|fact| fact_line(app, fact))
                        .collect::<Vec<_>>(),
                )
                .into_element()
        })
    })
}

fn fact_line(app: AppState, fact: &ProjectFact) -> AnyElement {
    let form = app.form;
    let size = if form == Form::Phone { 14.0 } else { 13.0 };
    let value = if fact.mono {
        rows::figures(form, fact.value.clone(), size - 0.5, Token::Ink)
    } else {
        rows::words(form, fact.value.clone(), size, color(Token::Ink))
    };
    let cell = view()
        .flex_1()
        .min_w(px(0.0))
        .flex_row()
        .accessible(true)
        .role(AccessibilityRole::Text)
        .label(format!("{}: {}", fact.label, fact.value))
        .when(form != Form::Phone, |cell| cell.focusable(true))
        .when(form == Form::Desktop, |cell| {
            cell.rounded(px(4.0)).focus_visible(|s| {
                s.outline(px(RING))
                    .outline_color(color(Token::Focus))
                    .outline_offset(px(1.0))
            })
        })
        .child(value.flex_1().min_w(px(0.0)));
    let label = view()
        .w(if form == Form::Terminal {
            cells(10.0)
        } else {
            px(78.0)
        })
        .flex_shrink_0()
        .accessibility_elements_hidden(true)
        .child(rows::words(form, fact.label, size, color(Token::Ink3)));
    view()
        .flex_row()
        .items_center()
        .gap(if form == Form::Terminal {
            cells(1.0)
        } else {
            px(10.0)
        })
        .when_some(fact.test_id.clone(), |line, id| line.test_id(&id))
        .child(label)
        .child(hint_on(app, cell, fact.hint.clone()))
        .into_element()
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

/// A list under the page's sections: its header with the count, then its rows, or the line that
/// says there are none.
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
fn burnup(app: AppState, request: DetailRef) -> AnyElement {
    let chart = timeline::scoped_burnup(app, request);
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
