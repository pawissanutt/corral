//! Home on the desktop and web: the owner's board (`model::home::board`). Widgets flow over a
//! grid of twelve columns in the order the owner chose, each as wide as its span; a row's
//! widgets share its height, so the board ends level. Customize (`board_edit`) adds, removes,
//! moves and resizes them; the arrangement is kept on this device only.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use nsh_core::prelude::*;

use crate::model::home::board::{self, COLUMNS, Item, Widget, shown_span};
use crate::model::home::{self};
use crate::model::projects::Scope;
use crate::state::AppState;
use crate::ui::kit::board_status::{board_status, dim_stale};
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::page;
use crate::ui::tokens::{MEASURE_PAGE, Token, color};

use super::board_edit::{self, Moving};
use super::dash::{composer, filled_tile};
use super::parts::{ask_project, empty, surface};
use super::{cards, panels};

/// The room between two widgets, split as half a gap around each.
pub(super) const GAP: f32 = 12.0;

/// What a widget's cell is measured by while one is moved.
pub(super) type Cells = Rc<RefCell<HashMap<Widget, ElementRef>>>;

/// The board page: its head with Customize, the board's status, the widgets and Ask's dock.
pub(super) fn page(app: AppState, scope: Memo<Scope>) -> AnyElement {
    let editing = app.board.editing;
    cleanup(move || app.board.set_editing(false));
    let said = computed(move || {
        let scope = scope.get();
        (
            home::title(&scope),
            app.feed.with(|feed| home::dash_summary(feed, &scope)),
            scope,
            editing.get(),
        )
    });
    let head = dynamic(move || {
        let (title, summary, scope, editing) = said.get();
        untracked(move || {
            let mut actions: Vec<AnyElement> = vec![customize(app, editing)];
            actions.extend(ask_project(app, &scope));
            page::measured_head(app, MEASURE_PAGE, title, Some(summary), actions)
        })
    });
    let body = view()
        .gap(px(GAP))
        .min_w(px(0.0))
        .px(px(14.0))
        .pt(px(14.0))
        .pb(px(20.0))
        .child(dynamic(move || {
            editing.get().then(|| untracked(|| board_edit::tray(app)))
        }))
        .child(grid(app, scope));
    view()
        .size_full()
        .flex_grow(1.0)
        .flex_col()
        .min_h(px(0.0))
        .bg(color(Token::Bg))
        .child(head)
        .child(page::measure(app, MEASURE_PAGE, board_status(app)))
        .child(scroll().flex_1().min_h(px(0.0)).child(page::measure(
            app,
            MEASURE_PAGE,
            dim_stale(app, body).into_element(),
        )))
        .child(composer(app))
        .into_element()
}

/// Customize, or Done while it is on.
fn customize(app: AppState, editing: bool) -> AnyElement {
    let (words, kind, test_id) = if editing {
        (board::DONE, ButtonKind::Primary, "btn-board-done")
    } else {
        (board::CUSTOMIZE, ButtonKind::Ghost, "btn-board-edit")
    };
    let button = button_with(
        app,
        words,
        kind,
        ButtonOptions::default(),
        test_id,
        move || app.board.set_editing(!editing),
    );
    crate::ui::kit::hint::hinted(app, button, board::CUSTOMIZE_HINT)
}

/// The widgets in order, each in a cell as wide as its span.
fn grid(app: AppState, scope: Memo<Scope>) -> AnyElement {
    let width = signal(0.0_f32);
    let moving: Signal<Option<Moving>> = signal(None);
    // Done removes the grips, mid-drag too: nothing stays lifted or marked.
    effect(move || {
        if !app.board.editing.get() && untracked(|| moving.with(Option::is_some)) {
            moving.set(None);
        }
    });
    let cells: Cells = Rc::new(RefCell::new(HashMap::new()));
    let items = computed(move || app.board.board.with(|b| b.items.clone()));
    let none = computed(move || items.with(Vec::is_empty) && !app.board.editing.get());
    let rows = each(
        move || items.get(),
        |item: &Item| item.widget,
        move |item, _| cell(app, scope, item, width, moving, cells.clone()),
    );
    view()
        .test_id("home-dashboard")
        .flex_row()
        .flex_wrap()
        .min_w(px(0.0))
        // Half a gap around each cell; the page's padding keeps the outer edge.
        .mx(px(-GAP / 2.0))
        .on_layout(move |bounds: &Bounds<Points>| {
            let w = bounds.size.width.0 - GAP;
            if (width.get_untracked() - w).abs() > 0.5 {
                width.set(w);
            }
        })
        .child(rows)
        .child(dynamic(move || {
            none.get().then(|| {
                untracked(|| {
                    view()
                        .w(relative(1.0))
                        .p(px(GAP / 2.0))
                        .child(surface("board-empty").child(empty(app, board::EMPTY)))
                        .into_element()
                })
            })
        }))
        .into_element()
}

/// One widget's cell: half a gap around it, as wide as its span shows at this width; Customize
/// lays its handles over it.
fn cell(
    app: AppState,
    scope: Memo<Scope>,
    item: ReadSignal<Item>,
    width: Signal<f32>,
    moving: Signal<Option<Moving>>,
    cells: Cells,
) -> AnyElement {
    let widget = item.get_untracked().widget;
    let me = element_ref();
    cells.borrow_mut().insert(widget, me);
    let editing = app.board.editing;
    let lifted = computed(move || moving.with(|m| m.as_ref().is_some_and(|m| m.widget == widget)));
    view()
        .test_id(&format!("board-{}", widget.slug()))
        .element_ref(&me)
        .relative()
        .flex_col()
        .min_w(px(0.0))
        .p(px(GAP / 2.0))
        .w(move || {
            let span = shown_span(item.get().span, width.get());
            Length::from(relative(f32::from(span) / f32::from(COLUMNS)))
        })
        .opacity(move || if lifted.get() { 0.45_f32 } else { 1.0_f32 })
        .child(
            view()
                .flex_grow(1.0)
                .flex_col()
                .min_w(px(0.0))
                .child(content(app, scope, widget)),
        )
        .child(dynamic(move || {
            editing.get().then(|| {
                untracked(|| board_edit::handles(app, widget, item, width, moving, cells.clone()))
            })
        }))
        .child(board_edit::drop_mark(widget, moving))
        .into_element()
}

/// What a widget draws: a panel of what Corral is doing, the Needs you cards or one number.
fn content(app: AppState, scope: Memo<Scope>, widget: Widget) -> AnyElement {
    match widget {
        Widget::NeedsYou => cards::needs_you(app, scope),
        Widget::Working => panels::working(app, scope),
        Widget::ByStatus => panels::by_status(app, scope),
        Widget::Project => panels::projects(app, scope),
        Widget::Cost => panels::cost(app),
        Widget::Events => panels::events(app, scope),
        Widget::Finished => panels::finished(app, scope),
        Widget::Burnup => panels::burnup(app, scope),
        Widget::StatNeeds
        | Widget::StatWorking
        | Widget::StatReady
        | Widget::StatSessions
        | Widget::StatLanded
        | Widget::StatCost => number(app, scope, widget.tile().unwrap_or_default()),
    }
}

/// One of Home's numbers, filling its cell.
fn number(app: AppState, scope: Memo<Scope>, key: &'static str) -> AnyElement {
    let shown = computed(move || {
        let scope = scope.get();
        app.feed.with(|feed| {
            let since = home::since(feed, &scope);
            home::tiles(feed, &scope, &since, &home::cost(feed))
                .into_iter()
                .find(|tile| tile.key == key)
        })
    });
    dynamic(move || {
        let tile = shown.get()?;
        Some(untracked(|| filled_tile(app, tile)))
    })
    .into_element()
}
