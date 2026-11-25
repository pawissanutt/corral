//! Sessions: every session in scope as one list grouped by state (Blocked, Working, Idle, then
//! Gone folded), beside the inspector of the selected one (`model::sessions`, `ui::kit::list`).
//! A row shows the session's state, id, name, role, what it does now, project, model, context
//! share, cost, age and its one next step; More holds its controls. The head says how many are
//! live and what they spent, and holds New (a chat, a build). The inspector leads with what the
//! session asks, answerable there, then its facts, its newest transcript rows and its Details
//! in brief. The session page itself is `screens::agents::session`.

use std::rc::Rc;

use corral_protocol::detail::DetailRef;
use nsh_core::prelude::*;

use crate::model::hue::Hue;
use crate::model::list::{self, ListKey};
use crate::model::status::SessionState;
use crate::model::text::span;
use crate::model::work::Record;
use crate::model::{Form, Id, Live, sessions};
use crate::screens::agents::{self, Compact, ask_session, ask_view};
use crate::state::AppState;
use crate::ui::composer::copyable_input;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, button};
use crate::ui::kit::chip::chip;
use crate::ui::kit::list::{
    ListPage, Row, cell_mono, cell_text, cell_title, list_page_with_sections, phone_lines,
};
use crate::ui::kit::next::{more_button, next_button};
use crate::ui::kit::notice::{error, notice};
use crate::ui::kit::page::head;
use crate::ui::kit::status::{session_mark, session_word_mark};
use crate::ui::kit::table::kv;
use crate::ui::kit::text::{dim, mono, plain};
use crate::ui::term;
use crate::ui::tokens::{MEASURE_FORM, Token, color};

/// The same filtered order for the list and its keyboard navigation.
pub(crate) fn listing(app: AppState) -> sessions::Listing {
    let scope = app.project_scope();
    let env = app.env();
    app.feed.with(|feed| {
        app.agents.with(|state| {
            sessions::listing(feed, &scope, &state.list, env, |id| {
                app.agent_pinned(id.as_str())
            })
        })
    })
}

/// The Sessions place on every form. A new chat opens its page from wherever it was started
/// (`AppState::follow_agents` settles the chat operations for the app's life).
pub fn screen(app: AppState) -> AnyElement {
    let form = app.form;
    let shown = computed(move || listing(app));
    let sections = computed(move || shown.with(|shown| shown.sections.clone()));
    let selected = computed(move || {
        let order = sections.with(|sections| list::order(sections));
        app.nav
            .with(|nav| list::settled(&order, nav.selection.session.as_ref()))
    });
    // The first row is selected on arrival, and the palette and the keys read the selection
    // the list shows.
    effect(move || {
        let shown = selected.get();
        if untracked(|| app.selection().session) != shown {
            untracked(|| app.nav.update(|nav| nav.selection.session = shown));
        }
    });
    let standing = computed(move || {
        let has_rows = shown.with(|shown| shown.had_rows);
        app.feed.with(|feed| sessions::standing(feed, has_rows))
    });
    let notice = computed(move || {
        if sections.with(Vec::is_empty) && shown.with(|shown| shown.had_rows) {
            return Some(sessions::NO_MATCH.to_owned());
        }
        standing.with(|standing| match &standing.fill {
            Some(fill @ (sessions::Fill::Loading | sessions::Fill::Empty)) => Some(fill.words()),
            Some(sessions::Fill::Failed(_)) | None => None,
        })
    });
    let summary = computed(move || shown.with(|shown| shown.summary.clone()));
    let page_head = dynamic(move || {
        let summary = summary.get();
        untracked(move || {
            let new = agents::new_menu(app);
            let actions = match form {
                Form::Phone => vec![new],
                Form::Desktop | Form::Terminal => vec![
                    button(
                        app,
                        "Inspector",
                        ButtonKind::Quiet,
                        "btn-inspector",
                        move || app.toggle_inspector(),
                    ),
                    new,
                ],
            };
            head(app, sessions::TITLE, Some(summary), actions)
        })
    });
    let page_head = view()
        .child(page_head)
        .child(filter_bar(app, shown))
        .child(stale(app, standing))
        .into_element();
    let columns = sessions::columns();
    list_page_with_sections(
        app,
        ListPage {
            key: "sessions",
            head: page_head,
            columns: columns.clone(),
            sections,
            selected,
            row: Rc::new(move |id| row(app, id, &columns)),
            inspector: Some(Rc::new(move |id| inspector(app, id))),
            on_key: Rc::new(move |key| match key {
                ListKey::Select(id) => app.select(Live::Session(id)),
                ListKey::Open(id) => app.open_live(Live::Session(id)),
                ListKey::Inspector | ListKey::More(_) => {}
            }),
            on_fold: Rc::new(move |key| app.agents.update(|state| state.list.toggle(key))),
            notice,
        },
        Some(failed(app, standing)),
        Some(Rc::new(move |section| {
            (section.fold != Some(false)).then(|| sort_bar(app, section.key.clone()))
        })),
    )
}

fn filter_bar(app: AppState, shown: Memo<sessions::Listing>) -> AnyElement {
    let field = copyable_input(app, None, move || {
        app.agents.with(|state| state.list.filter.clone())
    })
    .test_id("field-sessions")
    .label(sessions::FILTER_LABEL)
    .placeholder(sessions::FILTER_LABEL)
    .on_change(move |typed: &str| {
        app.agents
            .update(|state| state.list.filter = typed.to_owned())
    })
    .w_full()
    .text_color(color(Token::Ink));
    let chips = computed(move || shown.with(|shown| shown.chips.clone()));
    view()
        .px(if app.form == Form::Terminal {
            term::cells(1.0)
        } else {
            px(16.0)
        })
        .child(field)
        .child(dynamic(move || {
            let chips = chips.get();
            view()
                .flex_row()
                .flex_wrap()
                .children(chips.into_iter().map(|chip| {
                    button(
                        app,
                        chip.label,
                        choice(chip.selected),
                        format!("seg-mode-{}", chip.mode),
                        move || app.agents.update(|state| state.list.set_mode(&chip.mode)),
                    )
                }))
                .into_element()
        }))
        .into_element()
}

fn choice(selected: bool) -> ButtonKind {
    if selected {
        ButtonKind::Primary
    } else {
        ButtonKind::Quiet
    }
}

fn sort_bar(app: AppState, group: String) -> AnyElement {
    dynamic(move || {
        let sort = app.agents.with(|state| state.list.sort(&group));
        view()
            .flex_row()
            .flex_wrap()
            .children(sessions::Sort::ALL.into_iter().map(|option| {
                let group = group.clone();
                button(
                    app,
                    option.label(),
                    choice(sort == option),
                    format!("seg-sort-{group}-{}", option.slug()),
                    move || {
                        app.agents.update(|state| {
                            state.list.sorts.insert(group.clone(), option);
                        })
                    },
                )
            }))
            .into_element()
    })
    .into_element()
}

/// The line over sessions shown from the last update while their source is stale, a line's
/// measure wide on a desktop rather than across the whole list.
fn stale(app: AppState, standing: Memo<sessions::Standing>) -> AnyElement {
    let words = computed(move || standing.with(|standing| standing.stale.clone()));
    dynamic(move || {
        words.get().map(|words| {
            untracked(|| {
                let shown = notice(app, Hue::Held, &vec![span(words)], None);
                match app.form {
                    Form::Terminal => shown,
                    Form::Phone => view().px(px(16.0)).pt(px(10.0)).child(shown).into_element(),
                    Form::Desktop => view()
                        .max_w(px(MEASURE_FORM))
                        .px(px(18.0))
                        .py(px(8.0))
                        .child(shown)
                        .into_element(),
                }
            })
        })
    })
}

/// A failed subscription in the rows' place, with corrald's message and code: the list's
/// quiet notice would read like an empty list.
fn failed(app: AppState, standing: Memo<sessions::Standing>) -> AnyElement {
    let words = computed(move || {
        standing.with(|standing| match &standing.fill {
            Some(fill @ sessions::Fill::Failed(_)) => Some(fill.words()),
            Some(sessions::Fill::Loading | sessions::Fill::Empty) | None => None,
        })
    });
    dynamic(move || {
        words.get().map(|words| {
            untracked(|| {
                let shown = error(app, &vec![span(words)], None);
                let frame = view().px(px(18.0)).py(px(16.0));
                match app.form {
                    Form::Terminal => shown,
                    Form::Desktop => frame.max_w(px(MEASURE_FORM)).child(shown).into_element(),
                    Form::Phone => frame.child(shown).into_element(),
                }
            })
        })
    })
}

/// One session's facts as the model reads them now (tracked).
fn facts(app: AppState, id: &Id) -> Memo<Option<sessions::SessionRow>> {
    let key = id.clone();
    computed(move || {
        let env = app.env();
        app.feed.with(|feed| sessions::row(feed, &key, env))
    })
}

/// One session's row: a cell per column, each following the session's facts.
fn row(app: AppState, id: &Id, columns: &[list::Column]) -> Row {
    let form = app.form;
    let line = facts(app, id);
    let spoken = line
        .with_untracked(|line| line.as_ref().map(sessions::SessionRow::spoken))
        .unwrap_or_default();
    let record = Record::Session(id.clone());
    let cells = if form == Form::Phone {
        let shown = id.to_string();
        vec![
            dynamic(move || {
                line.with(|line| line.as_ref().map(|line| session_mark(app, line.line.state)))
            }),
            dynamic(move || {
                let (title, sub) = line.with(|line| {
                    line.as_ref()
                        .map(|line| {
                            (
                                line.title.clone(),
                                [
                                    line.role.clone(),
                                    line.doing.clone(),
                                    app.agents
                                        .with(|state| {
                                            state.list.sort(line.line.state.slug()).phone_fact(line)
                                        })
                                        .unwrap_or_default(),
                                ]
                                .into_iter()
                                .filter(|part| !part.is_empty())
                                .collect::<Vec<_>>()
                                .join(" · "),
                            )
                        })
                        .unwrap_or_default()
                });
                let shown = line
                    .with(|line| line.as_ref().map(|line| line.shown_id.clone()))
                    .unwrap_or_else(|| shown.clone());
                untracked(|| phone_lines(app, shown, title, sub))
            }),
            icon(Icon::Chevron, 16.0, color(Token::Ink3)),
        ]
    } else {
        columns
            .iter()
            .map(|column| cell(app, column.key, id, &record, line))
            .collect()
    };
    Row {
        test_id: list::row_id(&Live::Session(id.clone())),
        spoken,
        cells,
        menu: Some(record),
    }
}

/// The cell of column `key` for the session.
fn cell(
    app: AppState,
    key: &'static str,
    id: &Id,
    record: &Record,
    line: Memo<Option<sessions::SessionRow>>,
) -> AnyElement {
    let fact = move |pick: fn(&sessions::SessionRow) -> Option<String>, quiet: bool| {
        dynamic(move || {
            let words = line
                .with(|line| line.as_ref().and_then(pick))
                .unwrap_or_else(|| "–".to_owned());
            cell_text(app, words, quiet)
        })
    };
    let figure = move |pick: fn(&sessions::SessionRow) -> Option<String>| {
        dynamic(move || {
            let words = line
                .with(|line| line.as_ref().and_then(pick))
                .unwrap_or_else(|| "–".to_owned());
            cell_mono(app, words, true)
        })
    };
    match key {
        "state" => dynamic(move || {
            line.with(|line| line.as_ref().map(|line| session_mark(app, line.line.state)))
        }),
        "id" => dynamic(move || {
            let shown = line.with(|line| line.as_ref().map(|line| line.shown_id.clone()));
            cell_mono(app, shown.unwrap_or_default(), false)
        }),
        "name" => dynamic(move || {
            line.with(|line| {
                line.as_ref()
                    .map(|line| untracked(|| cell_title(app, line.title.clone(), Vec::new())))
            })
        }),
        "role" => fact(|line| Some(line.role.clone()), true),
        "now" => fact(
            |line| (!line.doing.is_empty()).then(|| line.doing.clone()),
            true,
        ),
        "project" => fact(|line| line.line.project.clone(), true),
        "model" => fact(|line| line.model.clone(), true),
        "context" => figure(|line| line.context.clone()),
        "cost" => figure(|line| line.cost.clone()),
        "activity" => figure(|line| line.activity.clone()),
        "age" => figure(|line| line.age.clone()),
        _ => {
            let (shown, record) = (id.to_string(), record.clone());
            dynamic(move || {
                let (next, remove) = line.with(|line| {
                    line.as_ref()
                        .map(|line| (line.next.clone(), line.remove_in_row()))
                        .unwrap_or_default()
                });
                let (shown, record) = (shown.clone(), record.clone());
                untracked(move || {
                    view()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .children(next.map(|next| next_button(app, &shown, &next, false)))
                        .children(remove.map(|action| remove_button(app, &shown, action)))
                        .child(more_button(app, record, true))
                        .into_element()
                })
            })
        }
    }
}

/// `Remove…` of a build stopped here: prune's dry run first, in the Remove sheet.
fn remove_button(app: AppState, id: &str, action: crate::model::agents::Action) -> AnyElement {
    button(
        app,
        "Remove…",
        ButtonKind::Ghost,
        format!("btn-remove-{id}"),
        move || agents::run_agents(app, action.clone()),
    )
}

fn keep_mode(app: AppState, id: Id) {
    let session = id.clone();
    let host = computed(move || app.feed.with(|feed| sessions::watch_host(feed, &session)));
    let session = id.clone();
    effect(move || {
        if host.get() {
            untracked(|| agents::watch_transcript(app, session.clone()));
        }
    });

    let target = sessions::changes_target(&id);
    let changes = computed(move || app.feed.with(|feed| sessions::watch_changes(feed, &id)));
    effect(move || {
        if changes.get() {
            untracked(|| app.watch_detail(target.clone(), true));
            let pinned = target.clone();
            cleanup(move || app.watch_detail(pinned, false));
            let target = target.clone();
            effect(move || {
                let needed = app.feed.with(|feed| {
                    feed.detail(&target)
                        .is_none_or(|cache| cache.stale && !cache.loading && cache.error.is_none())
                });
                if needed {
                    untracked(|| app.load(target.clone()));
                }
            });
        }
    });
}

/// Answer first, then actions, general and mode facts, transcript and Details in brief.
fn inspector(app: AppState, id: &Id) -> AnyElement {
    let form = app.form;
    let key = id.clone();
    // Open the recorded tail even if there is no reachable host to watch.
    untracked(|| {
        app.scope().load(DetailRef::Session {
            id: key.to_string(),
        })
    });
    keep_mode(app.scope(), id.clone());
    let shown = computed(move || {
        let env = app.env();
        app.feed.with(|feed| sessions::inspect(feed, &key, env))
    });
    let blocked = computed(move || {
        shown.with(|shown| {
            shown
                .as_ref()
                .is_some_and(|shown| shown.row.line.state == SessionState::Blocked)
        })
    });
    let gap = if form == Form::Terminal { 0.0 } else { 12.0 };
    let session = id.clone();
    let answer = dynamic(move || {
        blocked.get().then(|| {
            let session = session.clone();
            untracked(|| ask_session(app, session.clone(), ask_view(app, session), Compact::Card))
        })
    });
    let top = dynamic(move || {
        let shown = shown.get();
        untracked(move || {
            let shown = shown?;
            let row = &shown.row;
            let lead = match &shown.question {
                Some(question) => view()
                    .test_id("inspect-question")
                    .child(chip(app, question.clone(), crate::model::Hue::You))
                    .into_element(),
                None => plain(app, shown.state.clone()).into_element(),
            };
            Some(
                view()
                    .gap(px(gap))
                    .child(
                        view()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(session_word_mark(app, row.line.state))
                            .child(mono(app, row.shown_id.clone()))
                            .child(dim(app, row.role.clone())),
                    )
                    .child(plain(app, row.title.clone()).font_weight(FontWeight::BOLD))
                    .child(lead)
                    .into_element(),
            )
        })
    });
    let record = Record::Session(id.clone());
    let body = dynamic(move || {
        let shown = shown.get();
        let record = record.clone();
        untracked(move || {
            let shown = shown?;
            let blank = || (form == Form::Terminal).then(term::blank);
            let row = &shown.row;
            let live = shown.live();
            let actions = view()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(8.0))
                .children(
                    row.next
                        .as_ref()
                        .map(|next| next_button(app, row.line.id.as_str(), next, false)),
                )
                .children(
                    row.remove
                        .clone()
                        .map(|action| remove_button(app, row.line.id.as_str(), action)),
                )
                .children(crate::ui::ask::open_beside(app, &live))
                .child(button(
                    app,
                    "Open page",
                    ButtonKind::Ghost,
                    "btn-open-page",
                    move || app.open_live(live.clone()),
                ))
                .child(more_button(app, record, false));
            let facts = shown
                .facts
                .iter()
                .map(|(label, value)| {
                    (
                        (*label).to_owned(),
                        plain(app, value.clone()).into_element(),
                    )
                })
                .collect();
            let mode: Vec<(String, AnyElement)> = shown
                .mode
                .facts
                .iter()
                .map(|(label, value)| {
                    let value = match (*label, &shown.task) {
                        ("task", Some(task)) => {
                            let task = task.clone();
                            button(
                                app,
                                value.clone(),
                                ButtonKind::Quiet,
                                "btn-inspect-task",
                                move || app.open_live(Live::Task(task.clone())),
                            )
                        }
                        _ => plain(app, value.clone()).into_element(),
                    };
                    ((*label).to_owned(), value)
                })
                .collect();
            let latest: Vec<(String, AnyElement)> = shown
                .latest
                .iter()
                .map(|line| {
                    (
                        line.who.clone(),
                        dim(app, line.words.clone())
                            .number_of_lines(2u32)
                            .into_element(),
                    )
                })
                .collect();
            let details: Vec<(String, AnyElement)> = shown
                .details
                .iter()
                .map(|(label, summary)| (label.clone(), dim(app, summary.clone()).into_element()))
                .collect();
            Some(
                view()
                    .gap(px(gap))
                    .child(actions)
                    .children(blank())
                    .child(kv(app, facts))
                    .when(!mode.is_empty(), |body| {
                        body.children(blank()).child(
                            view()
                                .test_id("inspect-mode")
                                .child(dim(app, shown.mode.title).font_weight(FontWeight::BOLD))
                                .child(kv(app, mode)),
                        )
                    })
                    .when(!latest.is_empty(), |body| {
                        body.children(blank())
                            .child(dim(app, "Latest").font_weight(FontWeight::BOLD))
                            .child(kv(app, latest))
                    })
                    .when(!details.is_empty(), |body| {
                        body.children(blank())
                            .child(dim(app, "Details").font_weight(FontWeight::BOLD))
                            .child(kv(app, details))
                    })
                    .into_element(),
            )
        })
    });
    // What the session asks leads, with the card that answers it, before its facts.
    let column = view().gap(px(gap)).child(top).child(answer).child(body);
    match form {
        Form::Terminal => column.px(term::cells(1.0)).into_element(),
        Form::Desktop | Form::Phone => column.p(px(16.0)).into_element(),
    }
}
