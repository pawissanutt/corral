//! The profile table at the narrow desktop width where a long profile name used to run into the
//! window figure beside it.

use std::time::Duration;

use nsh_core::platform::{Idiom, Os};
use nsh_core::{Points, Size};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{Headless, LayoutNode, MemoryStorage};

const PROFILE: &str = "assistant · quick thread A-7";
const WINDOW: &str = "2%";

fn text_of(node: &LayoutNode) -> Option<&str> {
    node.text.as_ref().map(|text| text.text.text.as_str())
}

#[test]
fn long_profile_name_stays_out_of_the_window_column_at_900_points() {
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
    app.wait_for("id:usage-profiles").unwrap();
    let nodes = app.screen().layout_nodes();
    let profile = nodes
        .iter()
        .find(|node| text_of(node) == Some(PROFILE))
        .expect("the long profile name is on the page");
    let (top, bottom) = (
        profile.bounds.origin.y.0,
        profile.bounds.origin.y.0 + profile.bounds.size.height.0,
    );
    let window = nodes
        .iter()
        .filter(|node| text_of(node) == Some(WINDOW))
        .find(|node| {
            let y = node.bounds.origin.y.0;
            y < bottom && y + node.bounds.size.height.0 > top
        })
        .expect("the profile's window figure is on its row");
    let profile_right = profile.bounds.origin.x.0 + profile.bounds.size.width.0;
    assert!(
        profile_right <= window.bounds.origin.x.0,
        "profile name ends at {profile_right}, past the window figure at {}",
        window.bounds.origin.x.0
    );
}
