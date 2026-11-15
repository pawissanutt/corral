//! TT-11: a plan the keyboard selects is scrolled into the list's view, on a window too short to
//! show every plan.

use std::time::Duration;

use nsh_core::platform::{Idiom, Os};
use nsh_core::{Points, Size};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{Headless, LayoutNode, MemoryStorage};

/// The synthetic feed's last plan (the approved T-385, after T-885, the executing plans and the
/// 64-member preview tracker S-900), below the fold of a 360 pt window.
const LAST: &str = "plan-T-385";
const LIST: &str = "plans-list";

fn bounds_of<'a>(nodes: &'a [LayoutNode], id: &str) -> &'a LayoutNode {
    nodes
        .iter()
        .find(|node| node.test_id.as_deref() == Some(id))
        .unwrap_or_else(|| panic!("id:{id} is on the page"))
}

/// Whether `node` sits inside the view whose test id is `id`.
fn inside(nodes: &[LayoutNode], node: &LayoutNode, id: &str) -> bool {
    let mut parent = node.parent;
    while let Some(view) = nodes.iter().find(|view| view.id == parent) {
        if view.test_id.as_deref() == Some(id) {
            return true;
        }
        parent = view.parent;
    }
    false
}

#[test]
fn a_plan_selected_with_j_scrolls_into_the_list() {
    let mut config = EngineConfig::new(Root::of::<crate::App>());
    config.viewport = Size::new(Points(1440.0), Points(360.0));
    config.environment.os = Os::Linux;
    config.environment.idiom = Idiom::Desktop;
    config.storage = Some(MemoryStorage::new(
        [("corral.destination", "plans")].map(|(key, value)| (key.to_owned(), value.to_owned())),
    ));
    let mut app = Headless::start(config).unwrap();
    app.wait_for("id:ask-open").unwrap();
    app.press("id:palette-open").unwrap();
    app.wait_for("id:palette-input").unwrap();
    app.type_text("id:palette-input", "Data: fixture").unwrap();
    app.press("text:Data: fixture").unwrap();
    app.wait_for_gone("id:palette-input", Duration::from_secs(5))
        .unwrap();
    // The first plan takes the press and with it the focus, so j and k are the list's keys.
    app.press("id:plan-T-885").unwrap();
    // j walks the list in its drawn order; however many plans the feed serves, the last is at
    // most one press per row away.
    let nodes = app.screen().layout_nodes();
    let rows = nodes
        .iter()
        .filter(|node| {
            node.test_id
                .as_deref()
                .is_some_and(|id| id.starts_with("plan-"))
                && inside(&nodes, node, LIST)
        })
        .count();
    let target = format!("id:{LAST}");
    for _ in 0..rows {
        if app.screen().selected(&target).unwrap() {
            break;
        }
        app.press_key("j").unwrap();
    }
    assert!(
        app.screen().selected(&target).unwrap(),
        "j through {rows} plans never selected {LAST}"
    );
    let nodes = app.screen().layout_nodes();
    let list = bounds_of(&nodes, LIST);
    let row = bounds_of(&nodes, LAST);
    let (top, bottom) = (
        list.bounds.origin.y.0,
        list.bounds.origin.y.0 + list.bounds.size.height.0,
    );
    let (row_top, row_bottom) = (
        row.bounds.origin.y.0,
        row.bounds.origin.y.0 + row.bounds.size.height.0,
    );
    assert!(
        row_top >= top - 0.5 && row_bottom <= bottom + 0.5,
        "the selected plan spans {row_top}–{row_bottom}, outside the list's view {top}–{bottom}"
    );
}
