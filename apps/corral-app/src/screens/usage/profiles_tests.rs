//! The profile table at the narrow desktop width where a long profile name used to run into the
//! column beside it.

use std::time::Duration;

use nsh_core::platform::{Idiom, Os};
use nsh_core::{Points, Size};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{Headless, LayoutNode, MemoryStorage};

/// The synthetic feed's longest profile name, wider than the profile column at 900 points.
const PROFILE: &str = "fallback · pi-sol";
const TABLE: &str = "usage-profiles";

fn text_of(node: &LayoutNode) -> Option<&str> {
    node.text.as_ref().map(|text| text.text.text.as_str())
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
fn long_profile_name_stays_out_of_the_next_column_at_900_points() {
    let mut config = EngineConfig::new(Root::of::<crate::App>());
    config.viewport = Size::new(Points(900.0), Points(600.0));
    config.environment.os = Os::Linux;
    config.environment.idiom = Idiom::Desktop;
    config.storage = Some(MemoryStorage::new(
        [("corral.destination", "usage")].map(|(key, value)| (key.to_owned(), value.to_owned())),
    ));
    let mut app = Headless::start(config).unwrap();
    app.wait_for("id:ask-open").unwrap();
    app.press("id:palette-open").unwrap();
    app.wait_for("id:palette-input").unwrap();
    app.type_text("id:palette-input", "Data: fixture").unwrap();
    app.press("text:Data: fixture").unwrap();
    app.wait_for_gone("id:palette-input", Duration::from_secs(5))
        .unwrap();
    app.wait_for(&format!("id:{TABLE}")).unwrap();
    let nodes = app.screen().layout_nodes();
    let profile = nodes
        .iter()
        .find(|node| text_of(node) == Some(PROFILE) && inside(&nodes, node, TABLE))
        .expect("the long profile name is in the profile table");
    let (left, top, bottom) = (
        profile.bounds.origin.x.0,
        profile.bounds.origin.y.0,
        profile.bounds.origin.y.0 + profile.bounds.size.height.0,
    );
    let next = nodes
        .iter()
        .filter(|node| text_of(node).is_some() && inside(&nodes, node, TABLE))
        .filter(|node| {
            let y = node.bounds.origin.y.0;
            node.bounds.origin.x.0 > left && y < bottom && y + node.bounds.size.height.0 > top
        })
        .map(|node| node.bounds.origin.x.0)
        .fold(f32::INFINITY, f32::min);
    assert!(next.is_finite(), "the profile's row has a figure after it");
    let profile_right = left + profile.bounds.size.width.0;
    assert!(
        profile_right <= next,
        "profile name ends at {profile_right}, past the next column at {next}"
    );
}
