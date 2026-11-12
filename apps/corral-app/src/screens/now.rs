//! The Now destination. Desktop and terminal: a dock of tab groups the user arranges (who acts
//! next, the inspector, what changed, live context), whose layout is app data kept in Storage;
//! too small for its 2x2 grid it shows the list and the inspector as two panes. Phone: the tab's
//! grouped list, and the task page pushed from it.
//!
//! Every panel renders the work model; the displayed tabs follow the model's `now_tab`,
//! `low_tab` and `ctx_tab`, and the user's tab choices go back to it.

mod cells;
mod context;
pub(crate) mod feed;
mod inspector;
mod layout;
mod list;
pub(crate) mod parts;
mod phone;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use nsh_base::dock::{DockAreaLayout, DockPanel, DockPanelOptions, DockTab};
use nsh_base::dock_area;
use nsh_core::element::cell_size;
use nsh_core::geometry::Axis;
use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::work::{self, NowTab};
use crate::state::AppState;
use crate::ui::kit::button::tab_strip;
use crate::ui::term;
use crate::ui::tokens::{Token, color, row_height, text_size};

pub use phone::task_phone;

use parts::{act, dim_when, stale};

/// Every resize and tab selection reports a layout; Storage hears the one the user settles on.
const SAVE_DELAY: Duration = Duration::from_millis(400);
/// The smallest a quarter of the grid may be on a screen and still read as a panel.
const PANE_MIN_WIDTH: f32 = 280.0;
const PANE_MIN_HEIGHT: f32 = 144.0;
/// The smallest a quarter may be on a terminal grid: 35 Cells wide; its tab row, the line under
/// it and six rows of content high.
const QUARTER_COLUMNS: f32 = 35.0;
const QUARTER_HEAD_ROWS: f32 = 2.0;
const QUARTER_CONTENT_ROWS: f32 = 6.0;
/// Tab labels sit a step below the body text, as in the mock (11.5 against 13).
const TAB_SCALE: f32 = 11.5 / 13.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arrangement {
    Grid,
    /// The list and the inspector, side by side (`Horizontal`) or stacked.
    Panes(Axis),
}

/// The least room the 2x2 grid needs, width and height in points: two quarters each way, and on a
/// terminal grid the one-Cell divider the user drags between them.
fn grid_minimum() -> (f32, f32) {
    match cell_size() {
        Some(cell) => (
            (2.0 * QUARTER_COLUMNS + 1.0) * cell.width.0,
            (2.0 * (QUARTER_HEAD_ROWS + QUARTER_CONTENT_ROWS) + 1.0) * cell.height.0,
        ),
        None => (2.0 * PANE_MIN_WIDTH, 2.0 * PANE_MIN_HEIGHT),
    }
}

fn arrangement(width: f32, height: f32, (min_width, min_height): (f32, f32)) -> Arrangement {
    if width >= min_width && height >= min_height {
        Arrangement::Grid
    } else if width >= height {
        Arrangement::Panes(Axis::Horizontal)
    } else {
        Arrangement::Panes(Axis::Vertical)
    }
}

/// The Now destination on every form: the dock workspace on desktop and terminal, the grouped
/// list as the phone tab's root (it scrolls itself under its large title).
pub fn screen(app: AppState) -> AnyElement {
    match app.form {
        Form::Phone => phone::root(app),
        Form::Desktop | Form::Terminal => workspace(app),
    }
}

/// The tab titles, from the model: the list's label, the Goals control's, and each tab enum's.
type Titles = Rc<Vec<(&'static str, SharedString)>>;

fn titles(app: AppState) -> Titles {
    let view = untracked(|| {
        let env = app.env();
        app.work.with(|state| work::workspace(state, env))
    });
    let mut titles: Vec<(&'static str, SharedString)> = vec![
        (layout::WHO, view.list.label.into()),
        (layout::GOALS, view.goals.label.to_string().into()),
    ];
    titles.extend(
        NowTab::ALL
            .iter()
            .map(|&tab| (layout::inspector_panel(tab), tab.label().into())),
    );
    titles.extend(
        work::LowTab::ALL
            .iter()
            .map(|&tab| (layout::lower_panel(tab), tab.label().into())),
    );
    titles.extend(
        work::CtxTab::ALL
            .iter()
            .map(|&tab| (layout::context_panel(tab), tab.label().into())),
    );
    Rc::new(titles)
}

/// The title of panel `key`, tracked: in the Daemon state the lower tabs carry the board's count
/// instead of the fixture's clock and count.
fn title(app: AppState, titles: &Titles, key: &str) -> impl Fn() -> SharedString + use<> {
    let fixed = titles
        .iter()
        .find(|(panel, _)| *panel == key)
        .map_or_else(|| SharedString::from(""), |(_, title)| title.clone());
    let lower = work::LowTab::ALL
        .into_iter()
        .find(|tab| layout::lower_panel(*tab) == key);
    move || match lower {
        Some(tab) if app.daemon() => SharedString::from(
            app.feed
                .with(|feed| work::daemon_lower_label(tab, &feed.board)),
        ),
        _ => fixed.clone(),
    }
}

/// The dock workspace. Wide enough (measured, not guessed): a DockArea, 2x2, tab groups
/// "Who acts next" + "Goals"; "Story" + "Evidence" + "Session"; "Since your last visit" +
/// "Parked"; "Live" + "Plan graph". The layout is stored under "corral.now.layout" once changes
/// settle, restored on start, and reset to the default when `app.now_reset` changes. Too small
/// for 2x2 (terminal grids, narrow windows): the list and the inspector as two panes.
fn workspace(app: AppState) -> AnyElement {
    let layout = signal(layout::stored_layout().unwrap_or_else(layout::default_layout));
    // Plain Rust rather than a signal: the cleanup below must still read it while the page goes.
    let unsaved: Rc<RefCell<Option<DockAreaLayout>>> = Rc::default();
    let pending = stored(None::<Spawned<()>>);
    let size = signal(None::<(f32, f32)>);
    let minimum = grid_minimum();
    let shape = computed(move || {
        size.get()
            .map(|(width, height)| arrangement(width, height, minimum))
    });
    let titles = titles(app);

    let seen_reset = stored(app.now_reset.get_untracked());
    {
        let unsaved = unsaved.clone();
        effect(move || {
            let reset = app.now_reset.get();
            if seen_reset.replace(reset) != reset {
                unsaved.borrow_mut().take();
                if let Some(task) = pending.replace(None) {
                    task.cancel();
                }
                layout.set(layout::default_layout());
                follow_model(app, layout);
            }
        });
    }
    // The model's tab state shows its panels: Tab cycling the inspector, a reset, a restored
    // layout that displayed other tabs.
    effect(move || follow_model(app, layout));
    {
        // Leaving Now ends the save timer with the page; what it would have written goes now.
        let unsaved = unsaved.clone();
        cleanup(move || {
            if let Some(last) = unsaved.borrow_mut().take() {
                layout::save(&last);
            }
        });
    }
    let on_change = move |next: &DockAreaLayout| {
        let before = layout.get_untracked();
        let shown: Vec<&'static str> = {
            let was = layout::displayed(&before);
            layout::displayed(next)
                .into_iter()
                .filter(|key| !was.contains(key))
                .filter_map(|key| layout::PANELS.iter().copied().find(|panel| *panel == key))
                .collect()
        };
        layout.set(next.clone());
        *unsaved.borrow_mut() = Some(next.clone());
        let unsaved = unsaved.clone();
        let task = set_timeout(SAVE_DELAY, move || {
            if let Some(last) = unsaved.borrow_mut().take() {
                layout::save(&last);
            }
        });
        if let Some(old) = pending.replace(Some(task)) {
            old.cancel();
        }
        // A tab the user brought forward is the model's tab now.
        for key in shown {
            if let Some(action) = layout::panel_action(key) {
                act(app, action);
            }
        }
    };

    // The view measured is the room Now has, whatever it holds. A terminal keeps a percentage
    // height only on an empty box: once the dock or the panes are in it, it would shrink to their
    // height and the next measure would pick the panes. Growing into its parent keeps it the room's
    // size before and after its content arrives (the dock grows into it the same way).
    view()
        .size_full()
        .flex_grow(1.0)
        .bg(color(Token::Bg))
        .on_layout(move |frame: &Bounds<Points>| {
            size.set(Some((frame.size.width.0, frame.size.height.0)));
        })
        .child(dynamic(move || {
            shape.get().map(|shape| match shape {
                Arrangement::Grid => dock(app, layout, titles.clone(), on_change.clone()),
                Arrangement::Panes(axis) => panes(app, axis, &titles),
            })
        }))
        .into_element()
}

/// Displays the panels of the model's tabs where the layout shows another tab of their group.
fn follow_model(app: AppState, layout: Signal<DockAreaLayout>) {
    let wanted = app.work.with(layout::wanted);
    let mut next: Option<DockAreaLayout> = None;
    for key in wanted {
        let shown = next.clone().unwrap_or_else(|| layout.get_untracked());
        if let Some(changed) = layout::show(&shown, key) {
            next = Some(changed);
        }
    }
    if let Some(next) = next {
        layout.set(next);
    }
}

fn dock(
    app: AppState,
    layout: Signal<DockAreaLayout>,
    titles: Titles,
    on_change: impl Fn(&DockAreaLayout) + 'static,
) -> AnyElement {
    let form = app.form;
    let options: Vec<DockPanelOptions> = layout::PANELS
        .iter()
        .map(|&key| {
            let mut option = DockPanelOptions::new(key);
            option.closable = Some(false);
            option
        })
        .collect();
    dock_area()
        .size_full()
        .bg(color(Token::Bg))
        .layout(move || layout.get())
        .on_layout_change(on_change)
        // The workspace already chose the grid by its measured size; the dock's own phone
        // presentation would show one group where the grid fits.
        .compact(Some(false))
        .panels(options)
        .handle_accessibility_label("Resize panels")
        .group_style(|s| s.bg(color(Token::Bg)))
        .tab_bar_style(move |s| tab_bar(s, form))
        // A terminal bands the selected tab itself; a fill of the app's would turn the standard
        // grey without bands, where the reversed selection draws its text in that grey.
        .tab_style(move |s| {
            let s = tab(s, form);
            if form == Form::Terminal {
                s
            } else {
                s.selected(|s| s.bg(color(Token::Bg)))
            }
        })
        .content_style(|s| s.bg(color(Token::Bg)))
        .handle_style(|s| {
            s.bg(color(Token::Rule))
                .group_hover(|s| s.bg(color(Token::Rule2)))
                .group_active(|s| s.bg(color(Token::Ink3)))
        })
        .drop_indicator_style(|s| {
            s.bg(color(Token::Sel))
                .opacity(0.7)
                .border_1()
                .border_color(color(Token::Rule2))
                .rounded_md()
        })
        .render_tab(move |tab: &DockTab| {
            let key = tab.panel.as_str();
            tab_label(app, title(app, &titles, key), tab.active, key)
        })
        .render_panel(move |panel: &DockPanel| frame(app, panel.id.as_str()))
        .into_element()
}

/// The fallback for a small area: the list and the inspector, each under its bar: the list's one
/// tab, the inspector's Story, Evidence and Session. A terminal splits them with a line of its
/// own, which joins the frame around Now and the lines under the tabs (`┬`, `┼`, `┴`); a border
/// there would stop short of them.
fn panes(app: AppState, axis: Axis, titles: &Titles) -> AnyElement {
    let form = app.form;
    let terminal = form == Form::Terminal;
    let side_by_side = axis == Axis::Horizontal;
    let list_tab = tab(view(), form)
        .child(tab_label(
            app,
            title(app, titles, layout::WHO),
            true,
            layout::WHO,
        ))
        .role(AccessibilityRole::Tab)
        .accessibility_state(AccessibilityState {
            selected: true,
            ..AccessibilityState::default()
        })
        .when(!terminal, |t| t.bg(color(Token::Bg)));
    let options: Vec<(NowTab, &'static str)> =
        NowTab::ALL.iter().map(|&tab| (tab, tab.label())).collect();
    let inspector_tabs = view()
        .flex_row()
        .items_center()
        .px(px(8.0))
        .child(tab_strip(
            app,
            &options,
            move || app.work.with(|state| state.now_tab),
            move |tab| act(app, work::Action::NowTab(tab)),
            "seg-inspector",
        ));
    let make_pane = |bar: AnyElement, key: &'static str, first: bool| {
        view()
            .flex_1()
            .flex_col()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .when(!terminal && first && side_by_side, |pane| pane.border_r_1())
            .when(!terminal && first && !side_by_side, |pane| {
                pane.border_b_1()
            })
            .border_color(color(Token::Rule))
            .child(tab_bar(view(), form).child(bar))
            .child(view().flex_1().min_h(px(0.0)).child(frame(app, key)))
    };
    let divider = match (terminal, side_by_side) {
        (true, true) => Some(term::vrule()),
        (true, false) => Some(term::hrule()),
        (false, _) => None,
    };
    view()
        .size_full()
        .flex_grow(1.0)
        .map(|root| {
            if side_by_side {
                root.flex_row()
            } else {
                root.flex_col()
            }
        })
        .child(make_pane(list_tab.into_element(), layout::WHO, true))
        .children(divider)
        .child(make_pane(
            inspector_tabs.into_element(),
            inspector::FOLLOW,
            false,
        ))
        .into_element()
}

/// The strip of tabs on the sunk ground (`.dockbar`). A terminal draws a border as a whole row of
/// cells, so there the strip is the tabs alone, and the panel under it starts with a line.
fn tab_bar<S: Styled>(s: S, form: Form) -> S {
    let s = s
        .flex_row()
        .flex_none()
        .items_stretch()
        .h(px(row_height(form)));
    if form == Form::Terminal {
        s
    } else {
        s.bg(color(Token::BgSunk))
            .border_b_1()
            .border_color(color(Token::Rule))
    }
}

/// One tab (`.dockbar .dt`): 10 pt in, a rule after it; the selected one on the page's ground.
fn tab<S: Styled>(s: S, form: Form) -> S {
    let s = s.flex_row().items_center().gap(px(6.0));
    if form == Form::Terminal {
        s.px(px(8.0))
    } else {
        s.px(px(10.0)).border_r_1().border_color(color(Token::Rule))
    }
}

fn tab_label(
    app: AppState,
    title: impl Fn() -> SharedString + 'static,
    active: bool,
    key: &str,
) -> AnyElement {
    let label = text(title)
        .text_size(px(text_size(app.form) * TAB_SCALE))
        .font_weight(if active {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        })
        .text_color(color(if active { Token::Ink } else { Token::Ink3 }))
        .number_of_lines(1u32);
    let test_id = format!("seg-now-{key}");
    if app.form == Form::Terminal {
        return view().test_id(&test_id).child(label).into_element();
    }
    view()
        .test_id(&test_id)
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .child(grip(active))
        .child(label)
        .into_element()
}

/// Six dots before each title: the tab is a handle that drags its panel.
fn grip(active: bool) -> AnyElement {
    let dot = || view().size(px(2.0)).rounded_full().bg(color(Token::Ink3));
    let row = move || view().flex_row().gap(px(2.0)).child(dot()).child(dot());
    view()
        .flex_col()
        .gap(px(2.0))
        .opacity(if active { 0.7 } else { 0.45 })
        .accessibility_elements_hidden(true)
        .child(row())
        .child(row())
        .child(row())
        .into_element()
}

/// A panel: its content scrolling on the page's ground, dimmed while the board is stale. In a
/// terminal it starts with the line under its tabs, which reaches the lines on either side of the
/// panel and joins them.
fn frame(app: AppState, key: &str) -> AnyElement {
    let terminal = app.form == Form::Terminal;
    let stale = stale(app);
    let content = match key {
        layout::WHO => list::list(app),
        layout::GOALS => list::goals(app),
        layout::STORY => inspector::panel(app, Some(NowTab::Story)),
        layout::EVIDENCE => inspector::panel(app, Some(NowTab::Evidence)),
        layout::SESSION => inspector::panel(app, Some(NowTab::Session)),
        inspector::FOLLOW => inspector::panel(app, None),
        layout::SINCE => context::since(app),
        layout::PARKED => context::parked(app),
        layout::LIVE => context::live(app),
        layout::GRAPH => context::graph(app),
        _ => view().into_element(),
    };
    view()
        .size_full()
        .flex_col()
        .bg(color(Token::Bg))
        .when(terminal, |panel| panel.child(term::hrule()))
        .child(
            scroll()
                .test_id(&format!("now-{key}"))
                .flex_1()
                .min_h(px(0.0))
                .child(dim_when(view(), stale).child(content)),
        )
        .into_element()
}
