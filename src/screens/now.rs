//! The Now workspace: a dock of tab groups the user arranges, whose layout is app data kept in Storage.
//! Too small for its 2x2 grid, it shows the list and the inspector as two panes instead.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use nsh_base::dock::{
    DockAreaLayout, DockLayout, DockNode, DockPanel, DockPanelOptions, DockRegion, DockTab,
};
use nsh_base::dock_area;
use nsh_core::JsonValue;
use nsh_core::element::{cell_size, from_json};
use nsh_core::geometry::Axis;
use nsh_core::prelude::*;
use nsh_core::storage::Storage;

use crate::model::Form;
use crate::state::AppState;
use crate::ui::term;
use crate::ui::tokens::{Token, color, row_height, text_size};

const LAYOUT_KEY: &str = "corral.now.layout";
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

/// Each panel's stable key, as the layout and Storage name it, and its tab title.
const PANELS: [(&str, &str); 9] = [
    ("who", "Who acts next"),
    ("goals", "Goals"),
    ("story", "Story"),
    ("evidence", "Evidence"),
    ("session", "Session"),
    ("since", "Since your last visit"),
    ("parked", "Parked"),
    ("live", "Live"),
    ("graph", "Plan graph"),
];
/// The two panes shown when the grid does not fit.
const LIST: &str = "who";
const INSPECTOR: &str = "story";

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

/// The Now workspace. Wide enough (measure, do not guess): a DockArea, 2x2, tab groups
/// "Who acts next" + "Goals"; "Story" + "Evidence" + "Session"; "Since your last visit" + "Parked";
/// "Live" + "Plan graph"; each panel shows the body line. The layout is app data: stored under
/// Storage key "corral.now.layout" once changes settle, restored on start, and reset to the default
/// when app.now_reset changes. Too small for 2x2 (terminal grids, narrow windows): the same panels as
/// two panes, list and inspector.
pub fn workspace(app: AppState) -> AnyElement {
    let layout = signal(stored_layout().unwrap_or_else(default_layout));
    // Plain Rust rather than a signal: the cleanup below must still read it while the page goes.
    let unsaved: Rc<RefCell<Option<DockAreaLayout>>> = Rc::default();
    let pending = stored(None::<Spawned<()>>);
    let size = signal(None::<(f32, f32)>);
    let minimum = grid_minimum();
    let shape = computed(move || {
        size.get()
            .map(|(width, height)| arrangement(width, height, minimum))
    });

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
                layout.set(default_layout());
            }
        });
    }
    {
        // Leaving Now ends the save timer with the page; what it would have written goes now.
        let unsaved = unsaved.clone();
        cleanup(move || {
            if let Some(last) = unsaved.borrow_mut().take() {
                save(&last);
            }
        });
    }
    let on_change = move |next: &DockAreaLayout| {
        layout.set(next.clone());
        *unsaved.borrow_mut() = Some(next.clone());
        let unsaved = unsaved.clone();
        let task = set_timeout(SAVE_DELAY, move || {
            if let Some(last) = unsaved.borrow_mut().take() {
                save(&last);
            }
        });
        if let Some(old) = pending.replace(Some(task)) {
            old.cancel();
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
                Arrangement::Grid => dock(app, layout, on_change.clone()),
                Arrangement::Panes(axis) => panes(app, axis),
            })
        }))
        .into_element()
}

fn dock(
    app: AppState,
    layout: Signal<DockAreaLayout>,
    on_change: impl Fn(&DockAreaLayout) + 'static,
) -> AnyElement {
    let form = app.form;
    let options: Vec<DockPanelOptions> = PANELS
        .iter()
        .map(|&(key, _)| {
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
                s.selected(|s| s.bg(color(Token::Surface)))
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
        .render_tab(move |tab: &DockTab| tab_label(app, title(tab.panel.as_ref()), tab.active))
        .render_panel(move |_: &DockPanel| body(app))
        .into_element()
}

/// The fallback for a small area: the list and the inspector, each under a bar of one selected tab.
/// A terminal splits them with a line of its own, which joins the frame around Now and the lines
/// under the tabs (`┬`, `┼`, `┴`); a border there would stop short of them.
fn panes(app: AppState, axis: Axis) -> AnyElement {
    let form = app.form;
    let terminal = form == Form::Terminal;
    let side_by_side = axis == Axis::Horizontal;
    let make_pane = |key: &'static str, first: bool| {
        let selected_tab = tab(view(), form).child(tab_label(app, title(key), true));
        let selected_tab = if terminal {
            selected_tab
                .role(AccessibilityRole::Tab)
                .accessibility_state(AccessibilityState {
                    selected: true,
                    ..AccessibilityState::default()
                })
        } else {
            selected_tab.bg(color(Token::Surface))
        };
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
            .child(tab_bar(view(), form).child(selected_tab))
            .child(view().flex_1().min_h(px(0.0)).child(body(app)))
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
        .child(make_pane(LIST, true))
        .children(divider)
        .child(make_pane(INSPECTOR, false))
        .into_element()
}

/// The strip of tabs on the sunk ground. A terminal draws a border as a whole row of cells, so there
/// the strip is the tabs alone, and the panel under it starts with a line.
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

fn tab<S: Styled>(s: S, form: Form) -> S {
    let s = s.flex_row().items_center().gap(px(6.0));
    if form == Form::Terminal {
        s.px(px(8.0))
    } else {
        s.px(px(10.0)).border_r_1().border_color(color(Token::Rule))
    }
}

fn tab_label(app: AppState, title: &'static str, active: bool) -> AnyElement {
    let label = text(title)
        .text_size(px(text_size(app.form) * TAB_SCALE))
        .font_weight(if active {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::NORMAL
        })
        .text_color(color(if active { Token::Ink } else { Token::Ink3 }))
        .number_of_lines(1);
    if app.form == Form::Terminal {
        return label.into_element();
    }
    view()
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

/// The panel's content. In a terminal it starts with the line under its tabs, which reaches the
/// lines on either side of the panel and joins them, then a blank row.
fn body(app: AppState) -> AnyElement {
    let form = app.form;
    let terminal = form == Form::Terminal;
    let (pad_x, pad_y) = if terminal { (8.0, 0.0) } else { (16.0, 14.0) };
    view()
        .size_full()
        .bg(color(Token::Bg))
        .when(terminal, |panel| {
            panel.child(term::hrule()).child(term::blank())
        })
        .child(
            view().px(px(pad_x)).pt(px(pad_y)).child(
                text(move || app.snapshot().body_line())
                    .text_size(px(text_size(form)))
                    .text_color(color(Token::Ink3)),
            ),
        )
        .into_element()
}

fn title(key: &str) -> &'static str {
    PANELS
        .iter()
        .find(|&&(panel, _)| panel == key)
        .map_or("", |&(_, title)| title)
}

fn tabs(keys: &[&'static str]) -> DockLayout {
    keys.iter()
        .fold(DockLayout::tabs(), |group, key| group.panel(*key))
}

fn default_layout() -> DockAreaLayout {
    DockAreaLayout::new(
        DockLayout::h_split()
            .child(
                DockLayout::v_split()
                    .child(tabs(&["who", "goals"]), None)
                    .child(tabs(&["since", "parked"]), None),
                None,
            )
            .child(
                DockLayout::v_split()
                    .child(tabs(&["story", "evidence", "session"]), None)
                    .child(tabs(&["live", "graph"]), None),
                None,
            ),
    )
}

/// The stored layout, when it parses and holds exactly the workspace's panels; a layout from another
/// version of the panel set would hide a panel for good.
fn stored_layout() -> Option<DockAreaLayout> {
    let json: JsonValue = Storage::get_item(LAYOUT_KEY)?.parse().ok()?;
    let layout: DockAreaLayout = from_json(&json).ok()?;
    holds_every_panel(&layout).then_some(layout)
}

fn holds_every_panel(layout: &DockAreaLayout) -> bool {
    let mut held = Vec::new();
    collect_panels(&layout.center, &mut held);
    for region in [&layout.left, &layout.right, &layout.bottom]
        .into_iter()
        .flatten()
    {
        collect_panels(&region.layout, &mut held);
    }
    held.sort_unstable();
    let mut known: Vec<&str> = PANELS.iter().map(|&(key, _)| key).collect();
    known.sort_unstable();
    held == known
}

fn collect_panels<'a>(node: &'a DockNode, out: &mut Vec<&'a str>) {
    match node {
        DockNode::Split { children, .. } => {
            for child in children {
                collect_panels(child, out);
            }
        }
        DockNode::Tabs { panels, .. } => out.extend(panels.iter().map(|panel| panel.as_ref())),
    }
}

fn save(layout: &DockAreaLayout) {
    // A full Storage keeps the last layout that fit; the workspace itself is unaffected.
    let _ = Storage::set_item(LAYOUT_KEY, layout_json(layout).to_string());
}

/// The layout as the JSON `DockArea` takes from JS, which `from_json` reads back. Node ids are left
/// out: they are this process's, and the area assigns fresh ones when it installs the layout. (The
/// crate's persisted format, `DockAreaState`, is serde-only and the app has no JSON serializer.)
fn layout_json(layout: &DockAreaLayout) -> JsonValue {
    let mut fields = vec![("center", node_json(&layout.center))];
    for (name, region) in [
        ("left", &layout.left),
        ("right", &layout.right),
        ("bottom", &layout.bottom),
    ] {
        if let Some(region) = region {
            fields.push((name, region_json(region)));
        }
    }
    fields.into_iter().collect()
}

fn region_json(region: &DockRegion) -> JsonValue {
    let mut fields = vec![("layout", node_json(&region.layout))];
    if let Some(size) = region.size {
        fields.push(("size", JsonValue::from(size.0)));
    }
    if let Some(open) = region.open {
        fields.push(("open", JsonValue::from(open)));
    }
    if let Some(collapsible) = region.collapsible {
        fields.push(("collapsible", JsonValue::from(collapsible)));
    }
    fields.into_iter().collect()
}

fn node_json(node: &DockNode) -> JsonValue {
    match node {
        DockNode::Split {
            axis,
            children,
            sizes,
            ..
        } => [
            ("type", JsonValue::from("split")),
            (
                "axis",
                JsonValue::from(match axis {
                    Axis::Horizontal => "horizontal",
                    Axis::Vertical => "vertical",
                }),
            ),
            ("children", children.iter().map(node_json).collect()),
            (
                "sizes",
                sizes.iter().map(|size| size.map(|size| size.0)).collect(),
            ),
        ]
        .into_iter()
        .collect(),
        DockNode::Tabs {
            panels,
            active_index,
            ..
        } => [
            ("type", JsonValue::from("tabs")),
            (
                "panels",
                panels.iter().map(|panel| panel.to_string()).collect(),
            ),
            ("activeIndex", JsonValue::from(*active_index)),
        ]
        .into_iter()
        .collect(),
    }
}
