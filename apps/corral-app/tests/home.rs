use std::time::Duration;

use nsh_core::{
    Points, Size,
    platform::{Idiom, Os},
};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{Headless, MemoryStorage};

fn phone() -> EngineConfig {
    let mut config = EngineConfig::new(Root::of::<nsh_app_corral::App>());
    config.environment.os = Os::Web;
    config.environment.idiom = Idiom::Phone;
    config
}

fn desktop() -> EngineConfig {
    let mut config = phone();
    config.viewport = Size::new(Points(1440.0), Points(900.0));
    config.environment.os = Os::Linux;
    config.environment.idiom = Idiom::Desktop;
    config
}

/// Starts the app on Home and chooses the sample preview from the palette.
fn preview(config: EngineConfig) -> Headless {
    preview_kept(config, device())
}

/// This device's storage, opening on Home.
fn device() -> std::sync::Arc<MemoryStorage> {
    MemoryStorage::new([("corral.destination".to_owned(), "home".to_owned())])
}

/// [`preview`] with this device's storage in `storage`.
fn preview_kept(config: EngineConfig, storage: std::sync::Arc<MemoryStorage>) -> Headless {
    let opener = if config.environment.idiom == Idiom::Phone {
        "id:btn-search"
    } else {
        "id:palette-open"
    };
    let mut config = config;
    config.storage = Some(storage);
    let mut app = Headless::start(config).unwrap();
    app.wait_for("id:nav-settings").unwrap();
    app.press(opener).unwrap();
    app.wait_for("id:palette-input").unwrap();
    app.type_text("id:palette-input", "Data: fixture").unwrap();
    app.press("text:Data: fixture").unwrap();
    app.wait_for_gone("id:palette-input", Duration::from_secs(5))
        .unwrap();
    app.wait_for("id:list-home").unwrap();
    app
}

fn selected(app: &Headless, id: &str) -> bool {
    app.screen()
        .accessibility_state(&format!("id:{id}"))
        .is_ok_and(|state| state.selected)
}

/// The cards of Needs you in order, by test id.
fn cards(app: &Headless) -> Vec<String> {
    app.screen()
        .layout_nodes()
        .iter()
        .filter_map(|node| node.test_id.as_deref().map(str::to_owned))
        .filter(|id| id.starts_with("task-") || id.starts_with("agent-"))
        .collect()
}

/// On the dashboard `j` walked past the last card into rows it draws without a selection, and
/// Enter opened a page the owner could not see chosen: the keys open the next card, and Enter
/// opens the open card's page.
#[test]
fn j_opens_the_next_card_and_enter_opens_its_page() {
    let mut app = preview(desktop());
    app.wait_for("id:home-dashboard").unwrap();
    let shown = cards(&app);
    let first = shown.first().expect("a card waits on the owner").clone();
    assert!(selected(&app, &first), "{first} opens on arrival");
    app.press_key("j").unwrap();
    let second = cards(&app)
        .into_iter()
        .find(|id| selected(&app, id))
        .expect("a card is open");
    assert_ne!(second, first, "j opens the next card");
    app.press_key("enter").unwrap();
    let page = if second.starts_with("task-") {
        "id:task-page"
    } else {
        "id:session-transcript"
    };
    app.wait_for(page).unwrap();
}

/// Every number links to the page holding its detail: Ready opens Tasks.
#[test]
fn a_tile_opens_the_page_holding_its_detail() {
    let mut app = preview(desktop());
    app.press("id:stat-ready").unwrap();
    app.wait_for("id:list-tasks").unwrap();
}

/// A project row on Home scopes every page to it, so Home shows that project's facts in place
/// of the project list.
#[test]
fn a_project_row_scopes_every_page() {
    let mut app = preview(desktop());
    app.press("id:project-site").unwrap();
    app.wait_for("id:panel-project").unwrap();
    assert_eq!(app.screen().count("id:panel-projects").unwrap(), 0);
}

/// On the phone a Blocked task whose next step is a reply is answered in Home's list: the
/// words go out as the reply step and the task leaves Needs you, without opening its page.
#[test]
fn the_phone_answers_a_reply_in_homes_list() {
    let mut app = preview(phone());
    app.wait_for("id:field-reply").unwrap();
    let waiting = app.screen().count("id:field-reply").unwrap();
    app.type_text("id:field-reply", "Stay paused; never deliver twice.")
        .unwrap();
    app.press("id:btn-next-0").unwrap();
    app.wait_until(
        "the answered task leaves the list's answers",
        Duration::from_secs(5),
        |screen| screen.count("id:field-reply").unwrap_or(0) < waiting,
    )
    .unwrap();
    assert_eq!(app.screen().count("id:task-page").unwrap(), 0);
}

fn bounds(app: &Headless, id: &str) -> nsh_core::geometry::Bounds<Points> {
    app.screen()
        .layout_nodes()
        .into_iter()
        .find(|node| node.test_id.as_deref() == Some(id))
        .unwrap_or_else(|| panic!("{id} is on the screen"))
        .bounds
}

/// The board's widgets in the order they show.
fn widgets(app: &Headless) -> Vec<String> {
    app.screen()
        .layout_nodes()
        .iter()
        .filter_map(|node| node.test_id.as_deref())
        .filter_map(|id| id.strip_prefix("board-"))
        .filter(|id| {
            !["edit-", "grip-", "resize-", "tray", "empty"]
                .iter()
                .any(|part| id.starts_with(part))
        })
        .map(str::to_owned)
        .collect()
}

/// Customize arranges the desktop board by dragging and by its buttons, and keeps the
/// arrangement in this device's storage: a widget dragged onto another's left half lands
/// before it, a dragged edge widens it a column, and a removed widget comes back from the tray.
#[test]
fn customize_arranges_the_board_and_keeps_it_on_this_device() {
    let storage = device();
    let mut app = preview_kept(desktop(), storage.clone());
    app.wait_for("id:board-stat-needs").unwrap();
    assert_eq!(
        widgets(&app).first().map(String::as_str),
        Some("stat-needs")
    );
    app.press("id:btn-board-edit").unwrap();
    app.wait_for("id:board-tray").unwrap();

    let grip = bounds(&app, "board-grip-stat-cost").center();
    let target = bounds(&app, "board-stat-needs");
    let to_x = target.origin.x.0 + target.size.width.0 * 0.25;
    let to_y = target.origin.y.0 + target.size.height.0 * 0.5;
    app.drag(
        "id:board-grip-stat-cost",
        nsh_core::geometry::point(Points(to_x - grip.x.0), Points(to_y - grip.y.0)),
    )
    .unwrap();
    assert_eq!(&widgets(&app)[..2], ["stat-cost", "stat-needs"]);

    let cell = bounds(&app, "board-stat-cost").size.width.0;
    app.drag(
        "id:board-resize-stat-cost",
        nsh_core::geometry::point(Points(cell * 0.5 + 4.0), Points(0.0)),
    )
    .unwrap();
    let kept = storage
        .entries()
        .get("corral.home.board")
        .cloned()
        .unwrap_or_default();
    assert!(
        kept.starts_with("v1 stat-cost:3 stat-needs:2"),
        "kept on this device: {kept}"
    );

    app.press("id:btn-board-remove-burnup").unwrap();
    app.wait_for_gone("id:board-burnup", Duration::from_secs(5))
        .unwrap();
    app.press("id:btn-board-add-burnup").unwrap();
    app.wait_for("id:board-burnup").unwrap();
    assert_eq!(widgets(&app).last().map(String::as_str), Some("burnup"));

    app.press("id:btn-board-done").unwrap();
    app.wait_for_gone("id:board-tray", Duration::from_secs(5))
        .unwrap();
    assert_eq!(app.screen().count("id:board-edit-burnup").unwrap(), 0);
}
