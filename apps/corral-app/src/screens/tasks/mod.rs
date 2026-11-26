//! Tasks: every task in scope as one list grouped by status, beside the inspector of the
//! selected one (`model::tasks`, `ui::kit::list`). A row shows the task's status, id, title
//! with its tags, its priority, what is happening, its pipeline, size, difficulty, project,
//! profile, cost, age and its one next step; More holds every other operation. The head's
//! Columns control (`columns.rs`) chooses which of those optional columns show on this device. A plan's caret unfolds its
//! same-status children. Done and Dropped fold at the bottom, read a page at a time with Load
//! older under them. The head carries the filter field (words, and `size:`, `difficulty:` and `tag:`
//! met exactly), the open count with the running slots, the inspector's switch and New task;
//! under it the board's health (`ui::kit::board_status`), and the page fades while the board is
//! last known.

mod columns;
mod inspector;

use std::rc::Rc;

use corral_protocol::app::CLOSED_V1;
use corral_protocol::detail::DetailRef;
use nsh_core::element::color as for_appearance;
use nsh_core::prelude::*;

use crate::model::list::{self, ListKey};
use crate::model::tasks::{self, closed, meta};
use crate::model::work::{Press, Record, Verb};
use crate::model::{Form, Hue, Id, Live};
use crate::screens::task::keep::keep_details;
use crate::state::AppState;
use crate::ui::composer::copyable_input;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::board_status::{board_status, dim_stale};
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::chip::chip;
use crate::ui::kit::hint::hinted;
use crate::ui::kit::list::{
    ListPage, Row, cell_mono, cell_text, cell_title_kept, list_page_with, phone_lines,
};
use crate::ui::kit::more::press;
use crate::ui::kit::next::{more_button, next_button};
use crate::ui::kit::page::head;
use crate::ui::kit::page_phone::search;
use crate::ui::kit::status::{pipeline_marks, status_mark};
use crate::ui::kit::text::dim;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, MEASURE_FORM, Token, color, text_size};

/// The filter field's width beside the head's title: points on a desktop, Cells in a terminal.
const FILTER_WIDTH: f32 = 220.0;
const FILTER_CELLS: f32 = 18.0;

/// The Tasks place on every form. The list is built again when a project comes into or leaves
/// scope, since its Project column goes and comes with it.
pub fn screen(app: AppState) -> AnyElement {
    let scoped = computed(move || app.project_scope().project().is_some());
    dynamic(move || {
        scoped.get();
        untracked(|| list(app))
    })
}

fn list(app: AppState) -> AnyElement {
    let sections = computed(move || {
        let scope = app.project_scope();
        app.feed
            .with(|feed| app.tasks.with(|state| tasks::sections(feed, &scope, state)))
    });
    let selected = computed(move || {
        let order = sections.with(|sections| list::order(sections));
        app.nav
            .with(|nav| tasks::settle_selection(&order, nav.selection.task.as_ref()))
    });
    // The first row is selected on arrival, and the palette and the keys read the selection
    // the list shows.
    effect(move || {
        let shown = selected.get();
        if shown.is_some() && untracked(|| app.selection().task) != shown {
            untracked(|| app.nav.update(|nav| nav.selection.task = shown));
        }
    });
    keep_closed(app);
    let notice = computed(move || {
        let empty = sections.with(Vec::is_empty);
        let filtered = app.tasks.with(|state| !state.filter.trim().is_empty());
        app.feed
            .with(|feed| tasks::notice(feed, empty, filtered))
            .map(str::to_owned)
    });
    let columns = untracked(|| tasks::columns(&app.project_scope()));
    let chosen = {
        let columns = columns.clone();
        computed(move || app.task_columns.with(|chosen| chosen.mask(&columns)))
    };
    // The board's health reads as a line under the head, not a band across a wide list.
    let health = view()
        .when(app.form == Form::Desktop, |health| {
            health.max_w(px(MEASURE_FORM))
        })
        .child(board_status(app));
    let head = view()
        .flex_none()
        .child(page_head(app))
        .child(health)
        .into_element();
    let page = list_page_with(
        app,
        ListPage {
            key: "tasks",
            head,
            columns: columns.clone(),
            chosen: Some(chosen),
            sections,
            selected,
            row: Rc::new(move |key| row(app, key, &columns)),
            inspector: Some(Rc::new(move |key| inspector::inspector(app, key))),
            on_key: Rc::new(move |key| match key {
                ListKey::Select(key) => app.select(Live::Task(key)),
                ListKey::Open(key) => app.open_live(Live::Task(tasks::task_of(&key))),
                ListKey::Inspector | ListKey::More(_) => {}
            }),
            on_fold: Rc::new(move |key| app.tasks.update(|state| state.toggle(key))),
            notice,
        },
        Some(closed_foot(app)),
    );
    dim_stale(
        app,
        view().size_full().flex_grow(1.0).flex_col().min_h(px(0.0)),
    )
    .child(page)
    .into_element()
}

/// The head: the title with the open count and the slots, the filter field, the inspector's
/// switch and New task. A phone puts the filter under its large title.
fn page_head(app: AppState) -> AnyElement {
    let form = app.form;
    let summary = computed(move || {
        let scope = app.project_scope();
        app.feed.with(|feed| tasks::summary(feed, &scope))
    });
    let new = move || {
        button(
            app,
            "New task",
            ButtonKind::Primary,
            "btn-new-task",
            move || {
                press(
                    app,
                    Press::Verb {
                        record: Record::Workspace,
                        verb: Verb::NewTask,
                    },
                );
            },
        )
    };
    match form {
        Form::Phone => view()
            .flex_none()
            .child(head(app, tasks::TITLE, None, vec![search(app), new()]))
            .child(view().px(px(18.0)).pb(px(8.0)).child(filter(app)))
            .into_element(),
        Form::Desktop | Form::Terminal => dynamic(move || {
            let summary = summary.get();
            untracked(move || {
                let toggle = button(
                    app,
                    "Inspector",
                    ButtonKind::Quiet,
                    "btn-inspector",
                    move || app.toggle_inspector(),
                );
                head(
                    app,
                    tasks::TITLE,
                    Some(summary),
                    vec![filter(app), columns::picker(app), toggle, new()],
                )
            })
        }),
    }
}

/// The filter field (`field-tasks`): every word must be in a task's id, title or project.
fn filter(app: AppState) -> AnyElement {
    let form = app.form;
    let displayed = move || app.tasks.with(|state| state.filter.clone());
    let entry = copyable_input(app, None, displayed)
        .test_id("field-tasks")
        .label(tasks::FILTER_LABEL)
        .placeholder(tasks::FILTER)
        .on_change(move |typed: &str| {
            let typed = typed.to_owned();
            app.tasks.update(|state| state.filter = typed);
        })
        .flex_1()
        .min_w(px(0.0))
        .text_color(color(Token::Ink))
        .placeholder_color(move || {
            for_appearance(color(if form == Form::Terminal {
                Token::Ink3
            } else {
                Token::Ink4
            }))
        })
        .caret_color(move || for_appearance(color(Token::Ink)))
        .selection_color(move || for_appearance(color(Token::Sel)))
        .bg(AdaptiveColor::TRANSPARENT);
    match form {
        Form::Terminal => view()
            .w(term::cells(FILTER_CELLS))
            .ml(term::cells(1.0))
            .flex_shrink(1.0)
            .min_w(term::cells(6.0))
            .border_b_1()
            .border_color(color(Token::Rule2))
            .child(entry.h(term::rows(1.0)))
            .into_element(),
        Form::Desktop | Form::Phone => {
            let (height, size) = if form == Form::Phone {
                (40.0, 16.0)
            } else {
                (28.0, text_size(form))
            };
            view()
                .when(form == Form::Desktop, |field| field.w(px(FILTER_WIDTH)))
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .h(px(height))
                .px(px(10.0))
                .rounded(px(6.0))
                .border_1()
                .border_color(color(Token::Rule2))
                .bg(color(Token::Surface))
                .focus_within(|s| s.border_color(color(Token::Focus)))
                .child(icon(Icon::Search, 14.0, color(Token::Ink3)))
                .child(
                    entry
                        .h(px(height))
                        .font_family(FONT_TEXT)
                        .text_size(px(size)),
                )
                .into_element()
        }
    }
}

/// Keeps the closed pages the fold reads loaded once corrald has sent the open list: the
/// newest always (its total is the fold's count), the older ones while loaded.
fn keep_closed(app: AppState) {
    let targets = computed(move || {
        if !app.feed.with(closed::offered) {
            return Vec::<DetailRef>::new();
        }
        app.tasks.with(|state| state.closed.targets())
    });
    keep_details(app, targets, Some(CLOSED_V1));
}

/// Under the open fold: why it lists nothing, Load older (`btn-closed-older`) or Retry
/// (`btn-closed-retry`).
fn closed_foot(app: AppState) -> AnyElement {
    let form = app.form;
    let foot = computed(move || {
        let open = app.tasks.with(|state| state.unfolded.contains(closed::KEY));
        app.feed.with(|feed| {
            app.tasks
                .with(|state| closed::foot(feed, &state.closed, open))
        })
    });
    dynamic(move || {
        let foot = foot.get()?;
        Some(untracked(move || {
            let older = foot.older.then(|| {
                button(
                    app,
                    closed::OLDER,
                    ButtonKind::Ghost,
                    "btn-closed-older",
                    move || {
                        app.feed.with_untracked(|feed| {
                            app.tasks.update(|state| state.closed.more(feed));
                        });
                    },
                )
            });
            let retry = foot.retry.then(|| {
                button(
                    app,
                    "Retry",
                    ButtonKind::Ghost,
                    "btn-closed-retry",
                    move || {
                        let targets = app.tasks.with_untracked(|state| state.closed.targets());
                        for target in targets {
                            app.refresh(target);
                        }
                    },
                )
            });
            let row = view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(10.0))
                .children(foot.note.map(|note| dim(app, note).into_element()))
                .children(older)
                .children(retry);
            match form {
                Form::Terminal => row.px(term::cells(2.0)).into_element(),
                Form::Desktop | Form::Phone => row.px(px(18.0)).py(px(10.0)).into_element(),
            }
        }))
    })
}

/// One row, a cell per column, each following the task's line. A row under an unfolded plan
/// stands for its same-status child: set in from the edge, without the plan's tag.
fn row(app: AppState, key: &Id, columns: &[list::Column]) -> Row {
    let form = app.form;
    let id = tasks::task_of(key);
    let under = id != *key;
    let line = {
        let id = id.clone();
        computed(move || app.feed.with(|feed| tasks::line(feed, id.as_str())))
    };
    let spoken = line
        .with_untracked(|line| line.as_ref().map(tasks::TaskLine::spoken))
        .unwrap_or_default();
    let record = Record::Task(id.clone());
    let cells = if form == Form::Phone {
        let shown = id.to_string();
        vec![
            dynamic(move || {
                line.with(|line| line.as_ref().map(|line| status_mark(app, line.status)))
            }),
            dynamic(move || {
                line.with(|line| {
                    line.as_ref().map(|line| {
                        let lines =
                            phone_lines(app, shown.clone(), line.title.clone(), line.now.clone());
                        let facts = meta::line(line.size.as_deref(), line.difficulty, &line.tags);
                        view()
                            .flex_1()
                            .min_w(px(0.0))
                            .gap(px(2.0))
                            .child(lines)
                            .child(view().flex_row().child(cell_text(app, facts, true)))
                            .into_element()
                    })
                })
            }),
            caret(app, &id, under),
            icon(Icon::Chevron, 16.0, color(Token::Ink3)),
        ]
    } else {
        columns
            .iter()
            .map(|column| cell(app, column.key, &id, under, &record, line))
            .collect()
    };
    Row {
        test_id: list::row_id(&Live::Task(key.clone())),
        spoken,
        cells,
        menu: Some(record),
    }
}

/// The caret is shown only if expanding the plan would reveal children in this section.
/// The plan's page and inspector show children across statuses.
fn caret(app: AppState, id: &Id, under: bool) -> AnyElement {
    let form = app.form;
    let plan = {
        let id = id.clone();
        computed(move || {
            !under && {
                let scope = app.project_scope();
                app.feed.with(|feed| {
                    app.tasks
                        .with(|state| tasks::has_inline_children(feed, &scope, state, id.as_str()))
                })
            }
        })
    };
    let id = id.clone();
    dynamic(move || {
        if !plan.get() {
            return None;
        }
        let id = id.clone();
        let open = {
            let id = id.clone();
            computed(move || app.tasks.with(|state| state.plans.contains(&id)))
        };
        let press_id = id.clone();
        let mark = match form {
            Form::Terminal => text(move || if open.get() { "▾" } else { "▸" })
                .text_color(color(Token::Ink3))
                .into_element(),
            Form::Desktop | Form::Phone => view()
                .style_when(open, |s| s.rotate(deg(90.0)))
                .child(icon(Icon::Chevron, 12.0, color(Token::Ink3)))
                .into_element(),
        };
        Some(
            view()
                .test_id(&format!("btn-unfold-{id}"))
                .role(AccessibilityRole::Button)
                .label(format!("Children of {id} in this status group"))
                .accessibility_state(move || AccessibilityState {
                    expanded: Some(open.get()),
                    ..AccessibilityState::default()
                })
                .flex_shrink_0()
                .cursor_pointer()
                .when(form == Form::Phone, |caret| caret.p(px(10.0)))
                .on_press(move || app.tasks.update(|state| state.unfold(&press_id)))
                .child(mark)
                .into_element(),
        )
    })
}

/// The cell of column `key` for the task.
fn cell(
    app: AppState,
    key: &'static str,
    id: &Id,
    under: bool,
    record: &Record,
    line: Memo<Option<tasks::TaskLine>>,
) -> AnyElement {
    let fact = move |pick: fn(&tasks::TaskLine) -> Option<String>, quiet: bool| {
        dynamic(move || {
            let words = line
                .with(|line| line.as_ref().and_then(pick))
                .unwrap_or_else(|| tasks::ABSENT.to_owned());
            cell_text(app, words, quiet)
        })
    };
    let shown = id.to_string();
    match key {
        "status" => dynamic(move || {
            line.with(|line| line.as_ref().map(|line| status_mark(app, line.status)))
        }),
        "id" => cell_mono(app, shown, false),
        "title" => {
            let title = dynamic(move || {
                // The Priority column says it; its tag on the title is the fallback, kept in a
                // narrow cell where the other tags step out, since the row shows it nowhere else.
                let column = app.task_columns.with(|chosen| chosen.shows("priority"));
                line.with(|line| {
                    line.as_ref().map(|line| {
                        let kept = line
                            .priority
                            .filter(|_| !column)
                            .map(|priority| chip(app, priority, Hue::Neutral))
                            .into_iter()
                            .collect();
                        let tags = line
                            .parent
                            .as_ref()
                            .filter(|_| !under)
                            .map(|parent| format!("in {parent}"))
                            .into_iter()
                            .map(|tag| chip(app, tag, Hue::Neutral))
                            .chain(
                                meta::row_tags(&line.tags)
                                    .into_iter()
                                    .map(|tag| chip(app, tag, Hue::Neutral)),
                            )
                            .collect();
                        cell_title_kept(app, line.title.clone(), kept, tags)
                    })
                })
            });
            view()
                .flex_1()
                .min_w(px(0.0))
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .when(under, |cell| {
                    cell.pl(if app.form == Form::Terminal {
                        term::cells(2.0)
                    } else {
                        px(18.0)
                    })
                })
                .child(caret(app, id, under))
                .child(title)
                .into_element()
        }
        "priority" => dynamic(move || {
            line.with(|line| {
                line.as_ref()
                    .map(|line| cell_mono(app, line.rank, line.priority.is_none()))
            })
        }),
        "now" => fact(|line| Some(line.now.clone()), true),
        "pipeline" => dynamic(move || {
            line.with(|line| {
                line.as_ref()
                    .map(|line| pipeline_marks(app, &line.pipeline))
            })
        }),
        "size" => fact(|line| Some(meta::size_word(line.size.as_deref())), true),
        "difficulty" => fact(|line| Some(meta::difficulty_word(line.difficulty)), true),
        "project" => fact(|line| line.project.clone(), true),
        "profile" => fact(|line| line.profile.clone(), true),
        "cost" => hinted(app, cell_mono(app, tasks::ABSENT, true), tasks::COST_ABSENT),
        "age" => dynamic(move || {
            let age = line.with(|line| line.as_ref().map(|line| line.age.clone()));
            cell_mono(app, age.unwrap_or_default(), true)
        }),
        _ => {
            let record = record.clone();
            dynamic(move || {
                let next = line.with(|line| line.as_ref().and_then(|line| line.next.clone()));
                let (shown, record) = (shown.clone(), record.clone());
                untracked(move || {
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .children(next.map(|next| next_button(app, &shown, &next, false)))
                        .child(more_button(app, record, true))
                        .into_element()
                })
            })
        }
    }
}
