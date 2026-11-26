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
    select_from(app, "id:palette-open", row);
}

/// [`select`] from the control that opens the palette on this form (the phone's search).
fn select_from(app: &mut Headless, opener: &str, row: &str) {
    app.press(opener).unwrap();
    app.wait_for("id:palette-input").unwrap();
    app.type_text("id:palette-input", row).unwrap();
    app.press(&format!("text:{row}")).unwrap();
    app.wait_for_gone("id:palette-input", Duration::from_secs(5))
        .unwrap();
}

/// On a phone a citation is one tap away: T-12's body cites a section of T-2's brief, which
/// opens in the artifact viewer cut to that section as corrald serves it; the History comment
/// citing an artifact T-2 does not have opens nothing and says "not found".
#[test]
fn a_phone_tap_on_a_citation_opens_its_section_or_says_not_found() {
    let mut config = EngineConfig::new(Root::of::<nsh_app_corral::App>());
    config.environment.os = Os::Web;
    config.environment.idiom = Idiom::Phone;
    config.storage = Some(MemoryStorage::new([(
        "corral.destination".to_owned(),
        "tasks".to_owned(),
    )]));
    let mut app = Headless::start(config).unwrap();
    app.wait_for("id:nav-settings").unwrap();
    select_from(&mut app, "id:btn-search", "Data: fixture");
    select_from(&mut app, "id:btn-search", "T-12 Timeline zoom presets");
    app.wait_for("id:task-cited").unwrap();
    app.press("id:cite-T-2:3#acceptance").unwrap();
    app.wait_for("id:artifact-full").unwrap();
    app.wait_for("text:/Only the cited section “acceptance” shows/")
        .unwrap();
    // The served section's own words show, and none of the brief around it.
    app.wait_for("text:/covered by one test at the lowest layer/")
        .unwrap();
    assert!(
        app.wait_for_gone("text:Goal", Duration::from_secs(1))
            .is_ok()
    );
    app.press("id:tab-tasks").unwrap();
    select_from(&mut app, "id:btn-search", "T-12 Timeline zoom presets");
    app.press("id:tab-task-2").unwrap();
    app.press("id:cite-T-2:99").unwrap();
    app.wait_for("text:/T-2:99: not found/").unwrap();
    assert!(
        app.wait_for_gone("id:artifact-full", Duration::from_secs(1))
            .is_ok()
    );
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

/// Only a child in its plan's own status section shows under the plan. T-1 is in Backlog and
/// none of its children are, so its row has no caret, and its Ready child T-12 is reached from
/// the Children section of T-1's page, which opens the child's own page.
#[test]
fn a_child_in_another_status_opens_from_its_plans_page() {
    let mut app = preview("tasks");
    app.wait_for("id:task-T-1").unwrap();
    assert!(
        app.wait_for_gone("id:btn-unfold-T-1", Duration::from_secs(1))
            .is_ok()
    );
    select(&mut app, "T-1 Simple engine app");
    app.wait_for("id:task-page").unwrap();
    app.press("id:open-task-T-12").unwrap();
    // T-12 has no children, so its page has no link to itself: the plan's page is gone.
    app.wait_for_gone("id:open-task-T-12", Duration::from_secs(5))
        .unwrap();
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
    app.wait_for("id:node-m-apps-corral-app-shell").unwrap();
    app.press("id:btn-open-in-system").unwrap();
    app.wait_for("id:crumb-1").unwrap();
    app.wait_for("id:inspector-title").unwrap();
    app.wait_for("text:/^shell$/").unwrap();
}

#[test]
fn escape_in_impact_clears_the_pick_and_goes_up_before_leaving_the_task_page() {
    let mut app = preview("tasks");
    let task = "T-11 Collapse the sidebar on narrow screens";
    select(&mut app, task);
    app.press("id:tab-task-1").unwrap();
    app.press("id:seg-changes-1").unwrap();
    app.press("id:impact-item-row-1").unwrap();
    app.wait_for("id:impact-crumb-1").unwrap();
    app.wait_for("id:node-m-apps-corral-app-shell").unwrap();

    app.press("id:btn-back").unwrap();
    select(&mut app, task);
    app.press("id:tab-task-1").unwrap();
    app.wait_for("id:impact-crumb-1").unwrap();
    app.wait_for("id:btn-open-in-system").unwrap();

    let gone = Duration::from_secs(5);
    app.press_key("escape").unwrap();
    app.wait_for_gone("id:inspector", gone).unwrap();
    app.wait_for("id:impact-crumb-1").unwrap();

    app.press_key("escape").unwrap();
    app.wait_for_gone("id:impact-crumb-1", gone).unwrap();
    app.wait_for("id:node-c-apps-corral-app").unwrap();
    app.wait_for("id:inspector").unwrap();

    app.press_key("escape").unwrap();
    app.wait_for_gone("id:inspector", gone).unwrap();
    app.wait_for("id:task-impact").unwrap();

    app.press_key("escape").unwrap();
    app.wait_for_gone("id:task-page", gone).unwrap();
}

/// Whether the Tasks table shows the column of `key`: its header is laid out and visible.
fn column_shown(screen: &nsh_testing::Screen, key: &str) -> bool {
    let id = format!("col-{key}");
    screen.layout_nodes().iter().any(|node| {
        node.test_id.as_deref() == Some(id.as_str())
            && !node.hidden
            && node.bounds.size.width.0 > 0.0
    })
}

/// The Columns control's choice was lost on reopening, or a column it hid kept its room: the
/// menu stays open while columns are toggled, the table follows at once, the choice is kept on
/// this device and read back at the next launch, and Restore brings the default back.
#[test]
fn the_columns_control_keeps_its_choice_on_this_device_until_restored() {
    let storage = MemoryStorage::new([("corral.destination".to_owned(), "tasks".to_owned())]);
    let launch = |storage: &std::sync::Arc<MemoryStorage>| {
        let mut config = desktop();
        config.storage = Some(storage.clone());
        let mut app = Headless::start(config).unwrap();
        app.wait_for("id:nav-settings").unwrap();
        select(&mut app, "Data: fixture");
        app.wait_for("id:col-title").unwrap();
        app
    };
    let mut app = launch(&storage);
    assert!(column_shown(app.screen(), "priority"));
    assert!(!column_shown(app.screen(), "cost"));
    app.press("id:btn-columns").unwrap();
    app.wait_for("id:seg-column-priority").unwrap();
    app.press("id:seg-column-priority").unwrap();
    app.press("id:seg-column-age").unwrap();
    app.wait_until("the table follows", Duration::from_secs(5), |screen| {
        !column_shown(screen, "priority") && !column_shown(screen, "age")
    })
    .unwrap();
    assert_eq!(
        storage
            .entries()
            .get("corral.tasks.columns")
            .map(String::as_str),
        Some("now,pipeline,size,difficulty,project")
    );
    drop(app);

    let mut app = launch(&storage);
    assert!(!column_shown(app.screen(), "priority"));
    assert!(!column_shown(app.screen(), "age"));
    app.press("id:btn-columns").unwrap();
    app.wait_for("id:btn-columns-reset").unwrap();
    app.press("id:btn-columns-reset").unwrap();
    app.wait_until("the default is back", Duration::from_secs(5), |screen| {
        column_shown(screen, "priority") && column_shown(screen, "age")
    })
    .unwrap();
}
