//! Headless tests of Tasks and the task page, over the explicit sample preview.

use std::time::Duration;

use nsh_core::{
    Points, Size,
    platform::{Idiom, Os},
};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{Headless, MemoryStorage};

fn desktop() -> EngineConfig {
    let mut config = EngineConfig::new(Root::of::<nsh_app_corral::App>());
    config.viewport = Size::new(Points(1440.0), Points(900.0));
    config.environment.os = Os::Linux;
    config.environment.idiom = Idiom::Desktop;
    config
}

/// Starts the app on `destination` and chooses the sample preview from the palette, as a person
/// does after every launch.
fn preview(destination: &str) -> Headless {
    let mut config = desktop();
    config.storage = Some(MemoryStorage::new([(
        "corral.destination".to_owned(),
        destination.to_owned(),
    )]));
    let mut app = Headless::start(config).unwrap();
    app.wait_for("id:nav-settings").unwrap();
    select(&mut app, "Data: fixture");
    app
}

/// Runs a palette row: opens a sample task's page by its id and title.
fn select(app: &mut Headless, row: &str) {
    app.press("id:palette-open").unwrap();
    app.wait_for("id:palette-input").unwrap();
    app.type_text("id:palette-input", row).unwrap();
    app.press(&format!("text:{row}")).unwrap();
    app.wait_for_gone("id:palette-input", Duration::from_secs(5))
        .unwrap();
}

/// T-13's reviewer stays with the task across rounds; its run on the task page opens the
/// reviewer's session page.
#[test]
fn the_reviewer_session_opens_from_the_task_page() {
    let mut app = preview("tasks");
    select(&mut app, "T-13 Changes list marks forced events");
    app.wait_for("id:task-page").unwrap();
    app.press("id:btn-open-reviewer").unwrap();
    app.wait_for("id:btn-session-actions").unwrap();
    // R2 is T-13's reviewer run in the sample; its worker is R1.
    app.wait_for("text:/T-13 R2/").unwrap();
}

/// T-10's gate is red, so the decide bar in its band offers Land anyway: one press opens the
/// one local question, which names what it skips; confirming submits approve with force and
/// the task lands.
#[test]
fn land_anyway_from_the_decide_bar_asks_once_and_lands() {
    let mut app = preview("tasks");
    select(&mut app, "T-10 Usage page explains missing records");
    app.wait_for("id:task-band").unwrap();
    app.press("id:btn-land-anyway").unwrap();
    app.wait_for("id:confirm").unwrap();
    app.wait_for("text:/the gate check/").unwrap();
    app.press("id:confirm-confirm").unwrap();
    app.wait_for("text:/^Land anyway T-10 · Done/").unwrap();
}

/// A child listed under its unfolded plan is its own row, keyed apart from the child's row in
/// its status group; opening it must open the child, not a task named by that key.
#[test]
fn a_child_under_its_unfolded_plan_opens_the_childs_page() {
    let mut app = preview("tasks");
    app.wait_for("id:btn-unfold-T-1").unwrap();
    app.press("id:btn-unfold-T-1").unwrap();
    app.wait_for("id:task-T-12-in-T-1").unwrap();
    app.press("id:task-T-12-in-T-1").unwrap();
    app.press("id:task-T-12-in-T-1").unwrap();
    app.wait_for("id:task-page").unwrap();
    app.wait_for("text:Timeline zoom presets").unwrap();
}

/// Done and Dropped are never on the open list: opening their fold lists the closed tasks
/// corrald serves page by page (the sample's T-7 was dropped), and a row opens its page.
#[test]
fn the_done_and_dropped_fold_lists_the_served_closed_tasks() {
    let mut app = preview("tasks");
    app.wait_for("id:section-closed").unwrap();
    app.press("id:section-closed").unwrap();
    app.wait_for("id:task-T-7").unwrap();
    app.press("id:task-T-7").unwrap();
    app.press("id:task-T-7").unwrap();
    app.wait_for("id:task-page").unwrap();
    app.wait_for("text:Device authority editor").unwrap();
}

/// A comment sent from History is an engine operation on the task, and the task's events,
/// read again as the task changes, list it.
#[test]
fn a_comment_from_history_joins_the_tasks_events() {
    let mut app = preview("tasks");
    select(&mut app, "T-12 Timeline zoom presets");
    app.wait_for("id:task-page").unwrap();
    app.press("id:tab-task-2").unwrap();
    app.wait_for("id:field-comment").unwrap();
    app.type_text("id:field-comment", "Presets look right")
        .unwrap();
    app.press("id:btn-comment-send").unwrap();
    app.wait_for("text:/Presets look right/").unwrap();
}
