//! The Now dock's layout as data: its panels' keys, the default 2x2 arrangement, the copy kept in
//! Storage, and the reads and edits the workspace makes to keep the displayed tabs in step with
//! the model's tab state.

use nsh_base::dock::{DockAreaLayout, DockLayout, DockNode, DockRegion};
use nsh_core::JsonValue;
use nsh_core::element::from_json;
use nsh_core::geometry::Axis;
use nsh_core::storage::Storage;

use crate::model::work::{Action, CtxTab, LowTab, NowTab, State};

const LAYOUT_KEY: &str = "corral.now.layout";

/// Each panel's stable key, as the layout and Storage name it.
pub(super) const WHO: &str = "who";
pub(super) const GOALS: &str = "goals";
pub(super) const STORY: &str = "story";
pub(super) const EVIDENCE: &str = "evidence";
pub(super) const SESSION: &str = "session";
pub(super) const SINCE: &str = "since";
pub(super) const PARKED: &str = "parked";
pub(super) const LIVE: &str = "live";
pub(super) const GRAPH: &str = "graph";
pub(super) const PANELS: [&str; 9] = [
    WHO, GOALS, STORY, EVIDENCE, SESSION, SINCE, PARKED, LIVE, GRAPH,
];

/// The panel that shows an inspector tab.
pub(super) const fn inspector_panel(tab: NowTab) -> &'static str {
    match tab {
        NowTab::Story => STORY,
        NowTab::Evidence => EVIDENCE,
        NowTab::Session => SESSION,
    }
}

pub(super) const fn lower_panel(tab: LowTab) -> &'static str {
    match tab {
        LowTab::Since => SINCE,
        LowTab::Parked => PARKED,
    }
}

pub(super) const fn context_panel(tab: CtxTab) -> &'static str {
    match tab {
        CtxTab::Live => LIVE,
        CtxTab::PlanGraph => GRAPH,
    }
}

/// The panels the model's tab state says are shown: its inspector, lower and context tabs.
pub(super) fn wanted(state: &State) -> [&'static str; 3] {
    [
        inspector_panel(state.now_tab),
        lower_panel(state.low_tab),
        context_panel(state.ctx_tab),
    ]
}

/// What displaying a panel tells the model: its tab, or the Goals notice (the mock's Goals tab
/// is a control whose press explains the filter). The list itself carries no tab state.
pub(super) fn panel_action(key: &str) -> Option<Action> {
    Some(match key {
        GOALS => Action::Goals,
        STORY => Action::NowTab(NowTab::Story),
        EVIDENCE => Action::NowTab(NowTab::Evidence),
        SESSION => Action::NowTab(NowTab::Session),
        SINCE => Action::LowTab(LowTab::Since),
        PARKED => Action::LowTab(LowTab::Parked),
        LIVE => Action::CtxTab(CtxTab::Live),
        GRAPH => Action::CtxTab(CtxTab::PlanGraph),
        _ => return None,
    })
}

fn tabs(keys: &[&'static str]) -> DockLayout {
    keys.iter()
        .fold(DockLayout::tabs(), |group, key| group.panel(*key))
}

/// The mock's 2x2: the list and Goals over Since and Parked on the left; the inspector over Live
/// and the plan graph on the right.
pub(super) fn default_layout() -> DockAreaLayout {
    DockAreaLayout::new(
        DockLayout::h_split()
            .child(
                DockLayout::v_split()
                    .child(tabs(&[WHO, GOALS]), None)
                    .child(tabs(&[SINCE, PARKED]), None),
                None,
            )
            .child(
                DockLayout::v_split()
                    .child(tabs(&[STORY, EVIDENCE, SESSION]), None)
                    .child(tabs(&[LIVE, GRAPH]), None),
                None,
            ),
    )
}

fn regions(layout: &DockAreaLayout) -> impl Iterator<Item = &DockNode> {
    std::iter::once(&layout.center).chain(
        [&layout.left, &layout.right, &layout.bottom]
            .into_iter()
            .flatten()
            .map(|region| &region.layout),
    )
}

/// Every group's displayed panel.
pub(super) fn displayed(layout: &DockAreaLayout) -> Vec<&str> {
    fn walk<'a>(node: &'a DockNode, out: &mut Vec<&'a str>) {
        match node {
            DockNode::Split { children, .. } => {
                for child in children {
                    walk(child, out);
                }
            }
            DockNode::Tabs {
                panels,
                active_index,
                ..
            } => out.extend(panels.get(*active_index).map(|panel| panel.as_str())),
        }
    }
    let mut out = Vec::new();
    for node in regions(layout) {
        walk(node, &mut out);
    }
    out
}

/// The layout with `key` displayed in its group, or `None` when it already is (or is absent).
pub(super) fn show(layout: &DockAreaLayout, key: &str) -> Option<DockAreaLayout> {
    fn select(node: &mut DockNode, key: &str) -> Option<bool> {
        match node {
            DockNode::Split { children, .. } => children.iter_mut().find_map(|c| select(c, key)),
            DockNode::Tabs {
                panels,
                active_index,
                ..
            } => {
                let index = panels.iter().position(|panel| panel.as_str() == key)?;
                let changed = *active_index != index;
                *active_index = index;
                Some(changed)
            }
        }
    }
    let mut next = layout.clone();
    let changed = select(&mut next.center, key).or_else(|| {
        [&mut next.left, &mut next.right, &mut next.bottom]
            .into_iter()
            .flatten()
            .find_map(|region| select(&mut region.layout, key))
    });
    (changed == Some(true)).then_some(next)
}

/// The stored layout, when it parses and holds exactly the workspace's panels; a layout from another
/// version of the panel set would hide a panel for good.
pub(super) fn stored_layout() -> Option<DockAreaLayout> {
    let json: JsonValue = Storage::get_item(LAYOUT_KEY)?.parse().ok()?;
    let layout: DockAreaLayout = from_json(&json).ok()?;
    holds_every_panel(&layout).then_some(layout)
}

fn holds_every_panel(layout: &DockAreaLayout) -> bool {
    fn collect<'a>(node: &'a DockNode, out: &mut Vec<&'a str>) {
        match node {
            DockNode::Split { children, .. } => {
                for child in children {
                    collect(child, out);
                }
            }
            DockNode::Tabs { panels, .. } => out.extend(panels.iter().map(|panel| panel.as_str())),
        }
    }
    let mut held = Vec::new();
    for node in regions(layout) {
        collect(node, &mut held);
    }
    held.sort_unstable();
    let mut known = PANELS.to_vec();
    known.sort_unstable();
    held == known
}

pub(super) fn save(layout: &DockAreaLayout) {
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
