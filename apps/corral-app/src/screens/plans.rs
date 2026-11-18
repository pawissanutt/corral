//! Plans: the plan list beside the selected plan, whose brief answers what holds it up, what is
//! moving and what is next (desktop and terminal), the Plans tab and the pushed plan page
//! (phone), and the dependency graph Now's panel shows too. A plan is a parent task with
//! children: its document is the parent's body, its members the children.
//!
//! Everything shown comes from the served feed (corrald's, or the explicitly chosen synthetic
//! preview) through `model::plans`: `plan_outline` in the screen's own project scope for the
//! head's counts and the rows, `live_detail` for the selected plan (its brief with the graph
//! and path to done, and its document), `live_phone_page` for the pushed page and `layout` for
//! the graph. Presses run `plans::State::reduce_live` through `AppState::reduce`, whose effects
//! the root executes; the plan head, list rows and member cards carry the shared Actions menu.
//! `live.rs` keeps the shown plan's detail loaded and current.
//!
//! Desktop follows the window: from `THREE_COLUMNS` of content width the list, the plan and its
//! side column (path to done and the member story) stand side by side; narrower, the side
//! column moves under the plan; under `TWO_COLUMNS` the list is the page and a chosen plan
//! replaces it, with a way back.

mod detail;
mod doc;
mod graph;
mod list;
mod live;
mod overview;
mod path;
mod story;
mod verbs;

use nsh_core::prelude::*;

use crate::model::plans::{self, Action, ButtonKind as Weight, LiveAction, PlanOutline};
use crate::model::projects::FilterScreen;
use crate::model::{Form, Id, Live};
use crate::screens::PhonePage;
use crate::state::AppState;
use crate::ui::icons::{Icon, icon};
use crate::ui::kit::button::{ButtonKind, ButtonOptions, button_with};
use crate::ui::kit::glance::page_head;
use crate::ui::kit::hint::hint_on;
use crate::ui::kit::page::{Width, columns, pane, search};
use crate::ui::kit::project_filter::project_filter;
use crate::ui::tokens::{Token, color};

pub use live::{live_graph, live_plan_phone};
pub(crate) use verbs::menu_trigger;

const RING: f32 = 2.0;
/// The list column (`.split.three`), and its width once the side column has moved under the
/// plan (`max-width: 1180px`).
const LIST: f32 = 280.0;
const LIST_NARROW: f32 = 260.0;
/// The plan's side column: path to done and the selected member's story.
const SIDE: f32 = 300.0;
/// The terminal's list, in desktop points (a Cell for every 12).
const TERM_LIST: f32 = 380.0;
/// Content widths from which the side column stands beside the plan, and the list beside it.
const THREE_COLUMNS: f32 = 1180.0;
const TWO_COLUMNS: f32 = 900.0;

/// How much of the page stands side by side on desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Room {
    /// List, plan, side column.
    Three,
    /// List, then the plan with its side column under it.
    Two,
    /// The list, or the opened plan with the way back.
    One { opened: bool },
}

/// The Plans destination: the page head with its counts and the project scope over the list
/// and the selected plan. Phone: the large title over the scrolling list; a row pushes its plan.
/// Terminal: the list and the plan side by side, stacked on a narrow grid.
pub fn screen(app: AppState) -> AnyElement {
    if untracked(|| app.daemon()) && app.form != Form::Phone {
        live::keep_selected_loaded(app);
    }
    let outline = computed(move || content(app));
    let face = computed(move || outline.with(|outline| (outline.title, outline.stats.clone())));
    let head = dynamic(move || {
        let (title, stats) = face.get();
        let actions = if app.form == Form::Phone {
            vec![search(app)]
        } else {
            vec![]
        };
        untracked(|| page_head(app, title, &stats, actions))
    });
    let page = view()
        .flex_1()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .child(head)
        .child(project_filter(app, FilterScreen::Plans));
    match app.form {
        Form::Phone => page
            .child(pane(app, "plans-list", list::list(app, outline, None)))
            .into_element(),
        Form::Terminal => page
            .child(columns(
                app,
                vec![
                    (
                        Width::Fixed(TERM_LIST),
                        pane(app, "plans-list", list::list(app, outline, None)),
                    ),
                    (Width::Fill, detail::pane(app, detail::Side::Under)),
                ],
                1,
            ))
            .into_element(),
        Form::Desktop => desktop(app, page, outline),
    }
}

/// The desktop body under the head, laid out by the measured content width.
fn desktop(app: AppState, page: View, outline: Memo<PlanOutline>) -> AnyElement {
    let width = signal(None::<f32>);
    let opened = signal(false);
    // Below `TWO_COLUMNS` the list comes first after a plain `go`; a plan opened by name (from
    // Now, the palette, a link) shows at once, and again on each further open. A row
    // pressed here opens it too.
    let named = computed(move || {
        app.nav
            .with(|nav| matches!(nav.opened, Some(Live::Plan(_))).then_some(nav.opens))
    });
    effect(move || {
        let shown = named.get().is_some();
        untracked(|| opened.set(shown));
    });
    let room = computed(move || match width.get() {
        Some(width) if width < TWO_COLUMNS => Room::One {
            opened: opened.get(),
        },
        Some(width) if width < THREE_COLUMNS => Room::Two,
        _ => Room::Three,
    });
    page.on_layout(move |frame: &Bounds<Points>| width.set(Some(frame.size.width.0)))
        .child(dynamic(move || {
            let room = room.get();
            untracked(move || {
                let list_pane = |opens: Option<Signal<bool>>| {
                    pane(app, "plans-list", list::list(app, outline, opens))
                };
                match room {
                    Room::Three => columns(
                        app,
                        vec![
                            (Width::Fixed(LIST), list_pane(None)),
                            (Width::Fill, detail::pane(app, detail::Side::Beside)),
                        ],
                        1,
                    ),
                    Room::Two => columns(
                        app,
                        vec![
                            (Width::Fixed(LIST_NARROW), list_pane(None)),
                            (Width::Fill, detail::pane(app, detail::Side::Under)),
                        ],
                        1,
                    ),
                    Room::One { opened: false } => list_pane(Some(opened)),
                    Room::One { opened: true } => view()
                        .flex_1()
                        .flex_col()
                        .min_w(px(0.0))
                        .min_h(px(0.0))
                        .child(back(app, outline, opened))
                        .child(detail::pane(app, detail::Side::Under))
                        .into_element(),
                }
            })
        }))
        .into_element()
}

/// The way back from an opened plan to the list it replaced, named by the list's title.
fn back(app: AppState, outline: Memo<PlanOutline>, opened: Signal<bool>) -> AnyElement {
    view()
        .flex_row()
        .flex_none()
        .items_center()
        .h(px(40.0))
        .px(px(12.0))
        .border_b(px(1.0))
        .border_color(color(Token::Rule))
        .child(button_with(
            app,
            outline.with_untracked(|outline| outline.title),
            ButtonKind::Quiet,
            ButtonOptions {
                icon: Some(Icon::Back),
                ..ButtonOptions::default()
            },
            "page-back",
            move || {
                opened.set(false);
                app.nav.update(|nav| nav.dismiss_opened());
            },
        ))
        .into_element()
}

/// A plan's pushed phone page from the model's page: its title, Ask in the navigation bar
/// (which then stands for the floating Ask) and the detail as the body.
fn phone_page(app: AppState, page: plans::PhonePage, plan: Id) -> PhonePage {
    PhonePage {
        title: page.title,
        body: detail::phone_body(app, Some(plan)),
        ask: false,
        trailing: Some(icon_command(app, &page.right, page.right_label)),
        bar: None,
        scroll: true,
    }
}

/// The list content from the served feed, in the Plans scope.
fn content(app: AppState) -> PlanOutline {
    let selection = app.selection();
    let filter = app.project_filter(FilterScreen::Plans);
    app.feed
        .with(|feed| plans::plan_outline(&selection, feed, &filter))
}

/// Runs one Plans press with the served feed.
fn press(app: AppState, action: Action) {
    let Action::Live(live) = action;
    app.reduce(app.plans, move |state, feed, _, env| {
        state.reduce_live(live, feed, env)
    });
}

/// The test id of a press target, from its action.
fn target_id(action: &Action) -> String {
    let Action::Live(live) = action;
    match live {
        LiveAction::Plan(id) => format!("plan-{id}"),
        LiveAction::Member { member: id, .. } | LiveAction::Task(id) => format!("task-{id}"),
        LiveAction::Session(id) => format!("btn-open-session-{id}"),
        LiveAction::Review(_) => "btn-review-candidate".into(),
        LiveAction::Ask { record } => format!("btn-ask-{}", record.id()),
        LiveAction::Next { task, index } => format!("btn-next-{task}-{index}"),
        LiveAction::Retry(_) => "btn-retry".into(),
    }
}

/// A model button as a kit button: standard is the filled main action, ghost outlined, quiet
/// bare. Ask and session commands lead with the chat icon. The button's hint (what it does) is
/// its tooltip on desktop and its status line in a terminal.
fn command(app: AppState, button: &plans::Button, wide: bool) -> AnyElement {
    let action = button.action.clone();
    let kind = match button.kind {
        Weight::Standard => ButtonKind::Primary,
        Weight::Ghost => ButtonKind::Ghost,
        Weight::Quiet => ButtonKind::Quiet,
    };
    let chat = matches!(
        action,
        Action::Live(LiveAction::Ask { .. } | LiveAction::Session(_))
    );
    let drawn = button_with(
        app,
        button.label.clone(),
        kind,
        ButtonOptions {
            icon: chat.then_some(Icon::Ask),
            wide,
            ..ButtonOptions::default()
        },
        target_id(&action),
        move || press(app, action.clone()),
    );
    if button.hint.is_empty() {
        return drawn;
    }
    // The kit's button is no element a hint attaches to; its frame is, and the focus landing
    // on the button inside still shows it.
    let frame = view()
        .flex_row()
        .min_w(px(0.0))
        .when(wide, |frame| frame.flex_1())
        .when(!wide, |frame| frame.flex_shrink_0())
        .child(drawn);
    hint_on(app, frame, button.hint.clone()).into_element()
}

/// An icon-only control in the phone's navigation bar (`.pnav .rt button`): 44 by 44 pt, the
/// phone's touch-target floor, the chat icon in ink-2, named by `label`.
fn icon_command(app: AppState, button: &plans::Button, label: &'static str) -> AnyElement {
    let action = button.action.clone();
    view()
        .flex_row()
        .items_center()
        .justify_center()
        .min_w(px(44.0))
        .min_h(px(44.0))
        .rounded(px(10.0))
        .role(AccessibilityRole::Button)
        .label(label)
        .test_id(&target_id(&action))
        .cursor_pointer()
        .active(|s| s.opacity(0.6))
        .focus_visible(|s| {
            s.outline(px(RING))
                .outline_color(color(Token::Focus))
                .outline_offset(px(-RING))
        })
        .on_press(move || press(app, action.clone()))
        .child(icon(Icon::Ask, 18.0, color(Token::Ink2)))
        .into_element()
}

#[cfg(test)]
#[path = "plans_tests.rs"]
mod tests;
