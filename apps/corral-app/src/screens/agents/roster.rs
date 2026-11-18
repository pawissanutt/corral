//! The Agents page around its roster (`pgAgents`): the page head with the counts and the role
//! tabs; the project scope; the stale notice; the roster (`roster_rows`) of live runs and the
//! pinned quick thread with the selected session's panel (`panel`); then the assistant sessions
//! (`assistant::agents_tail`).
//!
//! Desktop: the panel sits beside the roster while the content is at least [`BESIDE`] wide,
//! overlays the roster's right side below that, and takes the whole content below [`STACK`] (the
//! mock's 1180 and 900 rules); the roster drops its project, quiet and started columns while a
//! panel is open or its room runs short. Phone: the head's counts, the role tabs and the scope
//! chips each on a sideways scrolling line, then two-line rows that push the session page; no
//! panel. Terminal: the same table in Cells, the panel beside it on a grid wide enough and in
//! its place otherwise (Esc closes it).

use nsh_core::prelude::*;

use crate::model::agents::{Fill, Roster};
use crate::model::assistant::QUICK;
use crate::model::glance::Stat;
use crate::model::hue::Hue;
use crate::model::projects::FilterScreen;
use crate::model::text::span;
use crate::model::{Form, Id};
use crate::screens::assistant;
use crate::state::AppState;
use crate::ui::kit::glance::page_head;
use crate::ui::kit::notice::{empty, error, loading, notice};
use crate::ui::kit::page::{pane, search};
use crate::ui::kit::project_filter::project_filter;
use crate::ui::term;
use crate::ui::tokens::{Token, color};

use super::panel::{self, Place};
use super::roster_rows::{self, Columns};
use super::{panel_view, roster_view, run_roster, segs};

/// Content width (pt) from which the panel sits beside the roster.
const BESIDE: f32 = 1180.0;
/// Content width (pt) under which the panel takes the whole content.
const STACK: f32 = 900.0;
/// Roster width (pt) that holds all eight columns: the mock's grid, its gaps and sides.
const WIDE: f32 = 1040.0;
/// Terminal: the roster's least Cells beside the panel.
const TERM_ROSTER: f32 = 56.0;

/// The page for the form. With the daemon, the page holds the quick thread's subscription (the
/// pinned row says what it last answered) and settles the assistant blocks' session operations,
/// both for as long as it stays open.
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
    let panel = computed(move || panel_view(app));
    let room = signal(None::<f32>);
    let place = computed(move || match room.get() {
        Some(width) if width < STACK => Place::Full,
        Some(width) if width < BESIDE => Place::Over,
        _ => Place::Beside,
    });
    let columns = computed(move || {
        let open = panel.with(Option::is_some);
        Columns::points(!open && room.get().is_none_or(|width| width >= WIDE), open)
    });
    view()
        .size_full()
        .flex_col()
        .bg(color(Token::Bg))
        .child(head(app, roster))
        .child(project_filter(app, FilterScreen::Agents))
        .child(stale(app, roster))
        .child(
            view()
                .relative()
                .flex_1()
                .flex_row()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .on_layout(move |frame: &Bounds<Points>| {
                    let width = Some(frame.size.width.0);
                    if room.get_untracked() != width {
                        room.set(width);
                    }
                })
                .child(list(app, roster, columns))
                .child(panel::panel(app, panel, place)),
        )
        .into_element()
}

fn phone(app: AppState, roster: Memo<Roster>) -> AnyElement {
    let content = view()
        .child(stale(app, roster))
        .child(roster_rows::lines(app, roster, fixed_columns()))
        .child(fill(app, roster))
        .child(untracked(|| assistant::agents_tail(app)))
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
                    .px(px(18.0))
                    .pb(px(8.0))
                    .child(role_tabs(app, roster)),
            ),
        )
        .child(project_filter(app, FilterScreen::Agents))
        .child(pane(app, "agents-roster", content))
        .into_element()
}

fn terminal(app: AppState, roster: Memo<Roster>) -> AnyElement {
    let panel = computed(move || panel_view(app));
    let open = computed(move || panel.with(Option::is_some));
    let beside = computed(move || term::page_columns(app) >= TERM_ROSTER + 1.0 + panel::TERM_PANEL);
    let place = computed(move || {
        if beside.get() {
            Place::Beside
        } else {
            Place::Full
        }
    });
    let columns = computed(move || {
        let page = term::page_columns(app);
        let open = open.get();
        let room = if open {
            page - 1.0 - panel::TERM_PANEL
        } else {
            page
        };
        Columns::cells(room, state_cells(roster), open)
    });
    view()
        .size_full()
        .flex_col()
        .child(head(app, roster))
        .child(project_filter(app, FilterScreen::Agents))
        .child(stale(app, roster))
        .child(
            view()
                .flex_1()
                .flex_row()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .child(
                    view()
                        .flex_1()
                        .flex_col()
                        .min_w(px(0.0))
                        .min_h(px(0.0))
                        // The panel takes the roster's place on a narrow grid; Esc gives it back.
                        .style_when(move || open.get() && !beside.get(), |s| s.hidden())
                        .child(list(app, roster, columns)),
                )
                .child(panel::panel(app, panel, place)),
        )
        .into_element()
}

/// Desktop and terminal: the column heads over the scrolling lines, the empty, loading or
/// failed state in their place, then the assistant blocks.
fn list(app: AppState, roster: Memo<Roster>, columns: Memo<Columns>) -> AnyElement {
    let content = view()
        .child(roster_rows::lines(app, roster, columns))
        .child(fill(app, roster))
        .when(app.form == Form::Terminal, |content| {
            content.child(term::hrule())
        })
        .child(untracked(|| assistant::agents_tail(app)))
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

/// The phone's rows draw every fact they have room for.
fn fixed_columns() -> Memo<Columns> {
    computed(|| Columns::points(false, false))
}

/// The terminal's State column: the longest chip shown (or its head) and the Cell either side
/// that a terminal chip keeps (tracked).
fn state_cells(roster: Memo<Roster>) -> u16 {
    roster.with(|r| {
        let head = r.columns[0].chars().count();
        let widest = r
            .rows
            .iter()
            .map(|row| row.chip.chars().count() + 2)
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

/// The page head (`.phead`): the title, its counts, and the role tabs at the trailing edge. The
/// phone's head keeps search at its trailing edge; its role tabs sit under it.
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
                    Form::Phone => vec![search(app)],
                    Form::Desktop | Form::Terminal => vec![role_tabs(app, roster)],
                };
                page_head(app, head.title, &head.stats, actions)
            })
        })
    })
}

/// The role tabs (`seg-agents-<role>`): each role with its count, the chosen one pressed. They
/// are rebuilt when a role or count comes or goes; choosing one restyles them in place.
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
                            move || run_roster(app, action.clone()),
                        )
                    })
                    .collect();
                segs::track(app, options)
            })
        })
    })
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
