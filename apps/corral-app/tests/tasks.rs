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

#[test]
fn a_done_task_opened_from_the_closed_list_shows_its_stored_patch() {
    let mut app = preview("tasks");
    app.wait_for("id:section-closed").unwrap();
    app.press("id:section-closed").unwrap();
    app.wait_for("id:task-T-2").unwrap();
    app.press("id:task-T-2").unwrap();
    app.press("id:task-T-2").unwrap();
    app.wait_for("id:task-page").unwrap();
    app.press("id:tab-task-1").unwrap();
    app.press("id:btn-expand-notes-md").unwrap();
    app.wait_for("text:/submitted locally; no repository was changed/")
        .unwrap();
}

/// The redesign dropped Ask about this from every task surface: the task page offers it again,
/// and Ask opens with the task attached as its context.
#[test]
fn ask_about_this_opens_ask_with_the_task_attached() {
    let mut app = preview("tasks");
    select(&mut app, "T-12 Timeline zoom presets");
    app.wait_for("id:task-page").unwrap();
    app.press("id:btn-ask-about").unwrap();
    app.wait_for("id:ask-transcript").unwrap();
    app.wait_for("id:btn-unchip").unwrap();
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

/// A new task's tags go out exactly as chosen: the form keeps the pending label on Save,
/// refuses a leading blank instead of trimming it, and supports exact filtering by that label.
#[test]
fn a_new_task_keeps_its_tags_exactly_and_the_filter_finds_it() {
    let mut app = preview("tasks");
    app.wait_for("id:btn-new-task").unwrap();
    app.press("id:btn-new-task").unwrap();
    app.wait_for("id:field-verb-0").unwrap();
    app.type_text("id:field-verb-0", "Pin zoom presets")
        .unwrap();
    app.type_text("id:field-verb-8", "Release blocker").unwrap();
    app.press("id:btn-tag-add").unwrap();
    app.wait_for("id:btn-tag-remove-0").unwrap();
    app.type_text("id:field-verb-8", "test").unwrap();
    app.press("id:seg-tags-testing").unwrap();
    app.wait_for("id:btn-tag-remove-1").unwrap();
    app.type_text("id:field-verb-8", " padded").unwrap();
    app.press("id:btn-verb-send").unwrap();
    app.wait_for("id:tag-problem").unwrap();
    app.clear("id:field-verb-8").unwrap();
    app.type_text("id:field-verb-8", "needs design").unwrap();
    app.press("id:btn-verb-send").unwrap();
    app.wait_for_gone("id:verb", Duration::from_secs(5))
        .unwrap();
    app.type_text("id:field-tasks", r#"tag:"needs design""#)
        .unwrap();
    app.wait_for("text:Pin zoom presets").unwrap();
    app.wait_for_gone("id:task-T-9", Duration::from_secs(5))
        .unwrap();
}

/// Changes › Impact draws T-11's change on the System page's map at the task's head; its
/// touched module opens the package's level with the module selected, and Open in System opens
/// Activity › System at that level with the module selected.
#[test]
fn a_touched_module_opens_in_system_at_its_package_level() {
    let mut app = preview("tasks");
    select(&mut app, "T-11 Collapse the sidebar on narrow screens");
    app.wait_for("id:task-page").unwrap();
    app.press("id:tab-task-1").unwrap();
    app.press("id:seg-changes-1").unwrap();
    app.wait_for("id:node-c-apps-corral-app").unwrap();
    app.press("id:impact-item-row-1").unwrap();
    app.wait_for("id:node-m-apps-corral-app-model").unwrap();
    app.press("id:btn-open-in-system").unwrap();
    app.wait_for("id:crumb-1").unwrap();
    app.wait_for("id:inspector-title").unwrap();
    app.wait_for("text:/^model$/").unwrap();
}
