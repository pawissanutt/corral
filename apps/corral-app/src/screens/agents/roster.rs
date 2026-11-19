//! The Agents page around its roster (`pgAgents`): the page head with the counts, the role tabs
//! (each with its count) and New; the project scope with Show (Live | All) beside it; the stale
//! notice; then the roster (`roster_rows`), one line per session of every role, which opens the
//! session page. Desktop: the column heads over 40 pt lines, the optional columns giving way as
//! the room runs short; phone: the role tabs and the scope each on a sideways scrolling line over
//! two-line rows; terminal: the same table in Cells. The selection moves with j/k, Enter opens it
//! and `N` opens the New build sheet.

use nsh_core::prelude::*;

use crate::model::agents::{Roster, Show};
use crate::model::assistant::QUICK;
use crate::model::glance::Stat;
use crate::model::hue::Hue;
use crate::model::projects::FilterScreen;
use crate::model::session::Fill;
use crate::model::text::span;
use crate::model::{Form, Id};
use crate::screens::assistant;
use crate::state::AppState;
use crate::ui::kit::glance::page_head;
use crate::ui::kit::notice::{empty, error, loading, notice};
use crate::ui::kit::page::{pane, search};
use crate::ui::kit::project_filter::project_filter;
use crate::ui::term;
use crate::ui::tokens::{FONT_TEXT, Token, color};

use super::roster_rows::{self, Columns};
use super::{roster_view, run_agents, segs, start};

/// Roster width (pt) that holds every column.
const WIDE: f32 = 1040.0;

/// The page for the form. With the daemon, the page holds the quick chat's subscription (the
/// pinned row says what it last answered) and settles new assistant sessions, which open their
/// page, for as long as it stays open.
pub(super) fn screen(app: AppState) -> AnyElement {
    if untracked(|| app.daemon()) {
        assistant::open_scope(app, Id::from(QUICK), false);
        assistant::watch_ops(app);
    }
    let roster = computed(move || roster_view(app));
    match app.form {
        Form::Desktop => desktop(app, roster),
        Form::Phone => phone(app, roster),
        Form::Terminal => terminal(app, roster),
    }
}

fn desktop(app: AppState, roster: Memo<Roster>) -> AnyElement {
    let room = signal(None::<f32>);
    let columns = computed(move || Columns::points(room.get().is_none_or(|width| width >= WIDE)));
    view()
        .size_full()
        .flex_col()
        .bg(color(Token::Bg))
        .child(head(app, roster))
        .child(scope_row(app, roster))
        .child(stale(app, roster))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .on_layout(move |frame: &Bounds<Points>| {
                    let width = Some(frame.size.width.0);
                    if room.get_untracked() != width {
                        room.set(width);
                    }
                })
                .child(list(app, roster, columns)),
        )
        .into_element()
}

fn phone(app: AppState, roster: Memo<Roster>) -> AnyElement {
    let content = view()
        .child(stale(app, roster))
        .child(roster_rows::lines(
            app,
            roster,
            computed(|| Columns::points(true)),
        ))
        .child(fill(app, roster))
        .into_element();
    view()
        .flex_1()
        .min_h(px(0.0))
        .flex_col()
        .bg(color(Token::Bg))
        .child(head(app, roster))
        .child(
            scroll().horizontal().flex_shrink_0().child(
                view()
                    .flex_row()
                    .flex_shrink_0()
                    .gap(px(10.0))
                    .px(px(18.0))
                    .pb(px(8.0))
                    .child(role_tabs(app, roster))
                    .child(show(app, roster)),
            ),
        )
        .child(project_filter(app, FilterScreen::Agents))
        .child(pane(app, "agents-roster", content))
        .into_element()
}

fn terminal(app: AppState, roster: Memo<Roster>) -> AnyElement {
    let columns = computed(move || Columns::cells(term::page_columns(app), state_cells(roster)));
    view()
        .size_full()
        .flex_col()
        .child(head(app, roster))
        .child(scope_row(app, roster))
        .child(stale(app, roster))
        .child(list(app, roster, columns))
        .into_element()
}

/// Desktop and terminal: the project scope with Show at its trailing edge.
fn scope_row(app: AppState, roster: Memo<Roster>) -> AnyElement {
    view()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(px(if app.form == Form::Terminal {
            16.0
        } else {
            12.0
        }))
        .child(
            view()
                .flex_1()
                .min_w(px(0.0))
                .child(project_filter(app, FilterScreen::Agents)),
        )
        .child(
            view()
                .flex_none()
                .when(app.form == Form::Desktop, |row| row.pr(px(18.0)))
                .when(app.form == Form::Terminal, |row| row.pr(term::cells(1.0)))
                .child(show(app, roster)),
        )
        .into_element()
}

/// Desktop and terminal: the column heads over the scrolling lines, the empty, loading or
/// failed state in their place.
fn list(app: AppState, roster: Memo<Roster>, columns: Memo<Columns>) -> AnyElement {
    let content = view()
        .child(roster_rows::lines(app, roster, columns))
        .child(fill(app, roster))
        .into_element();
    view()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .flex_col()
        .child(roster_rows::heads(app, roster, columns))
        .child(pane(app, "agents-roster", content))
        .into_element()
}

/// The terminal's State column: the longest state shown (or its head) and the Cell either side.
fn state_cells(roster: Memo<Roster>) -> u16 {
    roster.with(|r| {
        let head = r.columns[0].chars().count();
        let widest = r
            .rows
            .iter()
            .map(|row| roster_rows::state_words(row).chars().count() + 2)
            .max()
            .unwrap_or(0);
        u16::try_from(widest.max(head)).unwrap_or(u16::MAX)
    })
}

/// The head's facts, apart from the roles and rows: the head is redrawn when these change.
#[derive(Clone, PartialEq)]
struct Head {
    title: &'static str,
    stats: Vec<Stat>,
}

/// The page head (`.phead`): the title and its counts; at the trailing edge the role tabs and
/// New, the phone's search and New.
fn head(app: AppState, roster: Memo<Roster>) -> AnyElement {
    let shown = computed(move || {
        roster.with(|r| Head {
            title: r.title,
            stats: r.stats.clone(),
        })
    });
    dynamic(move || {
        shown.with(|head| {
            untracked(|| {
                let actions = match app.form {
                    Form::Phone => vec![search(app), start::new_menu(app)],
                    Form::Desktop | Form::Terminal => {
                        vec![role_tabs(app, roster), start::new_menu(app)]
                    }
                };
                page_head(app, head.title, &head.stats, actions)
            })
        })
    })
}

/// The role tabs (`seg-agents-<role>`): each role with its count, the chosen one pressed.
fn role_tabs(app: AppState, roster: Memo<Roster>) -> AnyElement {
    let shape = computed(move || {
        roster.with(|r| {
            r.roles
                .iter()
                .map(|tab| {
                    (
                        tab.id.clone(),
                        tab.label.clone(),
                        tab.count,
                        tab.action.clone(),
                    )
                })
                .collect::<Vec<_>>()
        })
    });
    let chosen = computed(move || {
        roster.with(|r| {
            r.roles
                .iter()
                .find(|tab| tab.selected)
                .map(|tab| tab.id.clone())
        })
    });
    dynamic(move || {
        shape.with(|tabs| {
            untracked(|| {
                let options = tabs
                    .iter()
                    .map(|(id, label, count, action)| {
                        let mine = id.clone();
                        let on = computed(move || {
                            chosen.with(|chosen| chosen.as_deref() == Some(mine.as_str()))
                        });
                        let action = action.clone();
                        segs::option(
                            app,
                            id,
                            label.clone(),
                            Some(count.to_string()),
                            on,
                            move || run_agents(app, action.clone()),
                        )
                    })
                    .collect();
                segs::track(app, options)
            })
        })
    })
}

/// Show (`seg-show-<live|all>`): the sessions whose host runs or waits, or every one the daemon
/// lists. The choice is kept per workspace on this device.
fn show(app: AppState, roster: Memo<Roster>) -> AnyElement {
    let form = app.form;
    let options = untracked(|| {
        roster.with(|r| {
            r.show
                .iter()
                .map(|tab| (tab.id.clone(), tab.label, tab.action.clone()))
                .collect::<Vec<_>>()
        })
    });
    let caption = text("Show")
        .number_of_lines(1u32)
        .text_color(color(Token::Ink3))
        .when(form != Form::Terminal, |t| {
            t.font_family(FONT_TEXT).text_size(px(12.0))
        });
    let options = options
        .into_iter()
        .map(|(id, label, action)| {
            let mine = id.clone();
            let on = computed(move || {
                roster.with(|r| r.show.iter().any(|tab| tab.selected && tab.id == mine))
            });
            let choice = match action {
                crate::model::agents::Action::SetShow(choice) => choice,
                _ => Show::Live,
            };
            segs::option(app, &id, label.to_owned(), None, on, move || {
                app.set_agents_show(choice)
            })
        })
        .collect();
    view()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .child(caption)
        .child(segs::track(app, options))
        .into_element()
}

/// The notice over sessions shown from the last update while their source is stale.
fn stale(app: AppState, roster: Memo<Roster>) -> AnyElement {
    let words = computed(move || roster.with(|r| r.stale.clone()));
    dynamic(move || {
        words.get().map(|words| {
            untracked(|| {
                let shown = notice(app, Hue::Held, &vec![span(words)], None);
                match app.form {
                    Form::Terminal => shown,
                    Form::Phone => view().px(px(16.0)).pt(px(10.0)).child(shown).into_element(),
                    Form::Desktop => view().px(px(18.0)).py(px(8.0)).child(shown).into_element(),
                }
            })
        })
    })
}

/// What stands in for the rows while there are none to show: the loading state while the
/// first rows load, the failure, or the empty state in the model's words.
fn fill(app: AppState, roster: Memo<Roster>) -> AnyElement {
    let shown = computed(move || roster.with(|r| r.fill.clone()));
    dynamic(move || shown.with(|fill| untracked(|| fill_view(app, fill))))
}

fn fill_view(app: AppState, fill: &Fill) -> Option<AnyElement> {
    match fill {
        Fill::Ready => None,
        Fill::Loading(caption) => Some(
            view()
                .when(app.form != Form::Terminal, |pad| {
                    pad.px(px(18.0)).py(px(8.0))
                })
                .child(loading(
                    app,
                    *caption,
                    &[(0.4, 10.0), (0.7, 10.0), (0.55, 10.0)],
                ))
                .into_element(),
        ),
        Fill::Failed(words) => Some(error(app, &vec![span(words.clone())], None)),
        Fill::Empty { heading, paragraph } => Some(empty(app, *heading, paragraph.clone(), None)),
    }
}
