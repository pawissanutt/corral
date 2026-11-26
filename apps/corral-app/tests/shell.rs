//! The shell's flows, headless, one test binary (`cargo test --test shell`) split by area under
//! `shell/`: the places and their pages, Ask, a task's operations, the side pane, and the
//! citations its chat draws inline. The modules sit under `shell/` by `#[path]`, so Cargo does
//! not take them for test binaries of their own.

use std::time::Duration;

use nsh_core::{
    Points, Size,
    platform::{Idiom, Os},
};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{Headless, MemoryStorage};

#[path = "shell/ask.rs"]
mod ask;
#[path = "shell/links.rs"]
mod links;
#[path = "shell/pane.rs"]
mod pane;
#[path = "shell/places.rs"]
mod places;
#[path = "shell/work.rs"]
mod work;

/// A terminal 80 columns wide and 24 rows high, in 8 × 16 pt Cells.
fn terminal_80() -> EngineConfig {
    let mut config = phone();
    config.viewport = Size::new(Points(640.0), Points(384.0));
    config.environment.os = Os::Linux;
    config.environment.idiom = Idiom::Terminal;
    config.environment.cell = Some(Size::new(Points(8.0), Points(16.0)));
    config
}
fn desktop() -> EngineConfig {
    let mut config = phone();
    config.viewport = Size::new(Points(1440.0), Points(900.0));
    config.environment.os = Os::Linux;
    config.environment.idiom = Idiom::Desktop;
    config
}

/// A desktop `width` points wide and `height` high: the side pane sits beside a wide page and
/// over a narrow one.
fn desktop_sized(width: f32, height: f32) -> EngineConfig {
    let mut config = desktop();
    config.viewport = Size::new(Points(width), Points(height));
    config
}

/// A phone browser. The default engine is iOS, where the tabs are the system `TabBar`, which the
/// headless driver cannot press; on the web the shell draws the same tabs itself.
fn phone() -> EngineConfig {
    let mut config = EngineConfig::new(Root::of::<nsh_app_corral::App>());
    config.environment.os = Os::Web;
    config.environment.idiom = Idiom::Phone;
    config
}

/// Starts the app with `entries` in Storage. Without a daemon that is the disconnected state:
/// no launch restores the fixture preview.
fn with_storage(config: EngineConfig, entries: &[(&str, &str)]) -> Headless {
    let mut config = config;
    config.storage = Some(MemoryStorage::new(
        entries
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string())),
    ));
    let mut app = Headless::start(config).unwrap();
    app.wait_for("id:nav-settings").unwrap();
    app
}

/// Starts the app as [`with_storage`] does, then chooses the sample preview from the palette,
/// as a person does after every launch: the synthetic feed through the live screens.
fn preview(config: EngineConfig, entries: &[(&str, &str)]) -> Headless {
    let opener = if config.environment.idiom == Idiom::Phone {
        "id:btn-search"
    } else {
        "id:palette-open"
    };
    let mut app = with_storage(config, entries);
    app.press(opener).unwrap();
    app.wait_for("id:palette-input").unwrap();
    app.type_text("id:palette-input", "Data: fixture").unwrap();
    app.press("text:Data: fixture").unwrap();
    app.wait_for_gone("id:palette-input", Duration::from_secs(5))
        .unwrap();
    app
}

fn assert_text(app: &mut Headless, id: &str, expected: &str) {
    app.wait_for(&format!("id:{id}")).unwrap();
    // A global text match would also match the unchanged sidebar label.
    let nodes = app.screen().layout_nodes();
    let texts: Vec<&str> = nodes
        .iter()
        .filter(|node| node.test_id.as_deref() == Some(id))
        .filter_map(|node| node.text.as_ref().map(|text| text.text.text.as_str()))
        .collect();
    assert_eq!(texts, [expected], "text of id:{id}");
}

/// Runs a palette row: opens a sample task's page by its id, as a person who knows it does,
/// or runs one of the operations the palette lists for the task in view.
fn select(app: &mut Headless, row: &str) {
    app.press("id:palette-open").unwrap();
    app.wait_for("id:palette-input").unwrap();
    app.type_text("id:palette-input", row).unwrap();
    app.press(&format!("text:{row}")).unwrap();
    app.wait_for_gone("id:palette-input", Duration::from_secs(5))
        .unwrap();
}

/// Opens the page of the sample's T-12, which waits on T-14, and runs Dispatch from its More
/// menu, which the palette also lists for the task in view.
fn dispatch_t12(app: &mut Headless) {
    select(app, "T-12 Timeline zoom presets");
    app.wait_for("id:task-page").unwrap();
    select(app, "Dispatch… T-12");
    app.wait_for("id:btn-verb-send").unwrap();
    app.press("id:btn-verb-send").unwrap();
}

fn selected(app: &Headless, id: &str) -> bool {
    app.screen()
        .accessibility_state(&format!("id:{id}"))
        .is_ok_and(|state| state.selected)
}

/// The number a count shows.
fn count_of(app: &mut Headless, id: &str) -> u32 {
    app.wait_for(&format!("id:{id}")).unwrap();
    app.screen()
        .layout_nodes()
        .iter()
        .filter(|node| node.test_id.as_deref() == Some(id))
        .find_map(|node| node.text.as_ref()?.text.text.parse().ok())
        .unwrap_or_else(|| panic!("id:{id} shows a number"))
}
