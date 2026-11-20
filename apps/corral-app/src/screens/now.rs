//! The Now destination: what waits on the owner, what is moving and what is next, at a glance.
//!
//! Desktop and terminal: the page head with its counts, the project scope with the Group toggle,
//! then the main column (the loop band, owner moves, moving tasks, Waiting, Closed) beside one
//! side column driven by one selection (the digest of what changed, an owner move, or a task's
//! story). Desktop: the side column stands 360 pt wide beside the list while the page has room
//! for both; narrower, the digest follows the list in its scroll and a selection lays its panel
//! over the right edge. Terminal: the side column stands beside the list while the grid holds
//! both at their least widths; otherwise the panel follows the list. Phone: the tab's list, and
//! the task page pushed from it.
//!
//! Every part renders `work::now_page`; the screen adds no words.

pub(crate) mod closed;
mod context;
pub(crate) mod feed;
pub(crate) mod inspector;
pub(crate) mod list;
pub(crate) mod parts;
mod phone;
pub(crate) mod rows;
pub(crate) mod side;

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::work::{NowPage, NowPanel};
use crate::state::AppState;
use crate::ui::kit::glance::page_head;
use crate::ui::term;
use crate::ui::tokens::{Token, color};

pub use phone::live_task_phone;

use list::{main_column, scope_note};
use parts::{dim_when, keep_loaded, now_page, shown_task, stale};

/// The desktop side column's width (`.split`'s second column), and the narrowest the page may be
/// with the list still beside it (the mock's 1180 pt window less its 208 pt sidebar).
pub(crate) const SIDE: f32 = 360.0;
pub(crate) const BESIDE: f32 = 972.0;
/// The panel a selection lays over a narrower page (`.split.has-sel>.aside`).
const OVERLAY: f32 = 460.0;
/// A terminal's list keeps its id, title, rail and age in this many Cells; the side column takes
/// a quarter of the old dock's grid, with the line between them.
pub(crate) const LIST_COLUMNS: f32 = 44.0;
pub(crate) const SIDE_COLUMNS: f32 = 35.0;

/// The Now destination on every form.
pub fn screen(app: AppState) -> AnyElement {
    match app.form {
        Form::Phone => phone::root(app),
        Form::Desktop | Form::Terminal => workspace(app),
    }
}

/// The head, the scope bar and the scope's note over the two columns. The task the side column
/// shows stays loaded.
fn workspace(app: AppState) -> AnyElement {
    let page = now_page(app);
    keep_loaded(app, shown_task(app, page));
    let title = untracked(|| page.with(|page| page.title));
    let stats = computed(move || page.with(|page| page.stats.clone()));
    let body = match app.form {
        Form::Terminal => terminal_body(app, page),
        Form::Desktop | Form::Phone => desktop_body(app, page),
    };
    // A terminal keeps a box's height only while it grows into its parent.
    view()
        .size_full()
        .flex_grow(1.0)
        .flex_col()
        .min_h(px(0.0))
        .bg(color(Token::Bg))
        .child(dynamic(move || {
            stats.with(|stats| page_head(app, title, stats, Vec::new()))
        }))
        .child(scope_note(app, page))
        .child(body)
        .into_element()
}

/// Whether the side column holds a selection rather than the digest (tracked).
fn selecting(page: Memo<NowPage>) -> Memo<bool> {
    computed(move || page.with(|page| !matches!(page.panel, NowPanel::Digest(_))))
}

/// The main column in its own scroll, dimmed while the board is stale.
fn main_scroll(app: AppState, page: Memo<NowPage>, below: Option<AnyElement>) -> AnyElement {
    scroll()
        .test_id("now-main")
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(
            dim_when(view(), stale(app))
                .child(main_column(app, page))
                .children(below),
        )
        .into_element()
}

/// The side panel in its own scroll.
fn side_scroll(app: AppState, page: Memo<NowPage>) -> AnyElement {
    scroll()
        .test_id("now-side")
        .flex_1()
        .min_h(px(0.0))
        .child(dim_when(view(), stale(app)).child(side::panel(app, page)))
        .into_element()
}

/// The side panel under the list in the one scroll, set off by a line.
fn panel_below(app: AppState, page: Memo<NowPage>) -> AnyElement {
    match app.form {
        Form::Terminal => view()
            .child(term::hrule())
            .child(side::panel(app, page))
            .into_element(),
        Form::Desktop | Form::Phone => view()
            .border_t_1()
            .border_t_color(color(Token::Rule))
            .child(side::panel(app, page))
            .into_element(),
    }
}

/// Desktop: measured, not assumed from the window, since the sidebar and Ask take their share.
fn desktop_body(app: AppState, page: Memo<NowPage>) -> AnyElement {
    let room = signal(None::<f32>);
    let beside = computed(move || room.get().is_none_or(|width| width >= BESIDE));
    view()
        .flex_1()
        .flex_col()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .on_layout(move |frame: &Bounds<Points>| room.set(Some(frame.size.width.0)))
        .child(dynamic(move || {
            if beside.get() {
                untracked(|| split(app, page))
            } else {
                untracked(|| stacked(app, page))
            }
        }))
        .into_element()
}

/// The list beside the side column, each scrolling on its own.
fn split(app: AppState, page: Memo<NowPage>) -> AnyElement {
    view()
        .flex_1()
        .flex_row()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .child(main_scroll(app, page, None))
        .child(
            view()
                .w(px(SIDE))
                .flex_shrink_0()
                .flex_col()
                .min_h(px(0.0))
                .border_l_1()
                .border_l_color(color(Token::Rule))
                .child(side_scroll(app, page)),
        )
        .into_element()
}

/// A narrower page: the digest follows the list in its scroll; a selection's panel lies over the
/// page's right edge, at most 460 pt wide, until it is closed.
fn stacked(app: AppState, page: Memo<NowPage>) -> AnyElement {
    let selecting = selecting(page);
    let digest = dynamic(move || (!selecting.get()).then(|| untracked(|| panel_below(app, page))));
    let overlay = dynamic(move || {
        selecting.get().then(|| {
            untracked(|| {
                view()
                    .absolute()
                    .top(px(0.0))
                    .right(px(0.0))
                    .bottom(px(0.0))
                    .w(px(OVERLAY))
                    .max_w(relative(1.0))
                    .flex_col()
                    .bg(color(Token::Bg))
                    .border_l_1()
                    .border_l_color(color(Token::Rule2))
                    .shadow_lg()
                    .child(side_scroll(app, page))
                    .into_element()
            })
        })
    });
    view()
        .relative()
        .flex_1()
        .flex_col()
        .min_h(px(0.0))
        .min_w(px(0.0))
        .child(main_scroll(app, page, Some(digest)))
        .child(overlay)
        .into_element()
}

/// Terminal: the side column beside the list while the grid holds the list's and the column's
/// least widths and the line between; otherwise the panel follows the list in its scroll.
fn terminal_body(app: AppState, page: Memo<NowPage>) -> AnyElement {
    let beside = computed(move || term::page_columns(app) >= LIST_COLUMNS + 1.0 + SIDE_COLUMNS);
    view()
        .flex_1()
        .flex_grow(1.0)
        .flex_col()
        .min_h(px(0.0))
        .child(dynamic(move || {
            if beside.get() {
                untracked(|| {
                    view()
                        .flex_1()
                        .flex_row()
                        .min_h(px(0.0))
                        .child(main_scroll(app, page, None))
                        .child(term::vrule())
                        .child(
                            view()
                                .w(term::cells(SIDE_COLUMNS))
                                .flex_shrink_0()
                                .flex_col()
                                .min_h(px(0.0))
                                .child(side_scroll(app, page)),
                        )
                        .into_element()
                })
            } else {
                untracked(|| main_scroll(app, page, Some(panel_below(app, page))))
            }
        }))
        .into_element()
}
