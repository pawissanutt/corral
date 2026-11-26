use std::time::Duration;

use nsh_core::{
    PasteEvent, Points, Size,
    platform::{Idiom, Os},
};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{Headless, MemoryStorage};

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

#[test]
fn both_forms_open_on_home() {
    let mut app = preview(phone(), &[]);
    assert_text(&mut app, "page-title", "Home");
    drop(app);

    let mut app = preview(desktop(), &[]);
    app.wait_for("id:list-home").unwrap();
}

/// The sidebar lists the four places at rest and Activity's four pages only under Activity
/// while one of them shows; every one of them, and Settings at the foot, opens a page.
#[test]
fn desktop_sidebar_reaches_every_place_and_activity_page() {
    let mut app = preview(desktop(), &[]);
    for (slug, shows) in [
        ("home", "list-home"),
        ("tasks", "list-tasks"),
        ("sessions", "page-title"),
    ] {
        app.press(&format!("id:nav-{slug}")).unwrap();
        app.wait_for(&format!("id:{shows}")).unwrap();
        assert_eq!(app.screen().count("id:nav-usage").unwrap(), 0);
    }
    app.press("id:nav-activity").unwrap();
    for slug in ["feed", "timeline", "usage", "artifacts", "system"] {
        app.wait_for(&format!("id:nav-{slug}")).unwrap();
        app.press(&format!("id:nav-{slug}")).unwrap();
        app.wait_for("id:page-title").unwrap();
        assert!(selected(&app, &format!("nav-{slug}")), "{slug} is current");
        assert!(selected(&app, "nav-activity"));
    }
    app.press("id:nav-settings").unwrap();
    app.wait_for("id:settings-devices").unwrap();
    assert_eq!(app.screen().count("id:nav-usage").unwrap(), 0);
}

/// The phone's four tabs are the four places; Activity's root lists its four pages, each a
/// page pushed over the list.
#[test]
fn phone_tabs_reach_every_place_and_activity_page() {
    let mut app = preview(phone(), &[]);
    for (slug, title) in [("tasks", "Tasks"), ("home", "Home")] {
        app.press(&format!("id:tab-{slug}")).unwrap();
        assert_text(&mut app, "page-title", title);
    }
    app.press("id:tab-sessions").unwrap();
    app.wait_for("id:page-title").unwrap();

    app.press("id:tab-activity").unwrap();
    assert_text(&mut app, "page-title", "Activity");
    for slug in ["feed", "timeline", "usage", "artifacts", "system"] {
        app.wait_for(&format!("id:nav-{slug}")).unwrap();
        app.press(&format!("id:nav-{slug}")).unwrap();
        app.wait_for_gone("id:nav-feed", Duration::from_secs(5))
            .unwrap();
        app.back().unwrap();
        app.wait_for("id:nav-feed").unwrap();
    }
}

/// The phone has no canvas map: with a project chosen, System lists the same nodes, and the
/// picked one's edges under it, which pick the other end.
#[test]
fn phone_system_lists_the_map_and_the_picked_nodes_edges() {
    let mut app = preview(phone(), &[]);
    app.press("id:tab-activity").unwrap();
    app.press("id:nav-system").unwrap();
    app.press("id:system-pick-corral").unwrap();
    app.wait_for("id:system-tree").unwrap();
    app.press("id:node-c-crates-corral-ops").unwrap();
    app.wait_for("id:edge-line-0").unwrap();
    app.press("id:edge-line-0").unwrap();
    app.wait_for("id:inspector-title").unwrap();
}

/// Settings has no tab: the gear in the head opens it over the tab in view, and its sections
/// push their pages.
#[test]
fn phone_settings_open_from_the_gear_and_their_sections_push_their_pages() {
    let mut app = preview(phone(), &[]);
    app.press("id:tab-tasks").unwrap();
    app.press("id:nav-settings").unwrap();
    assert_text(&mut app, "page-title", "Settings");
    app.wait_for("id:settings-devices").unwrap();
    app.press("id:settings-config").unwrap();
    assert_text(&mut app, "page-title", "Configuration");
    app.back().unwrap();
    assert_text(&mut app, "page-title", "Settings");
    app.back().unwrap();
    assert_text(&mut app, "page-title", "Tasks");
}

#[test]
fn pi_hosted_tools_form_saves_and_reopens_both_routes() {
    let mut app = preview(phone(), &[]);
    app.press("id:nav-settings").unwrap();
    app.press("id:settings-config").unwrap();
    app.press("id:tab-config-3").unwrap();
    let pi = "id:seg-config-harnesses-pi";
    app.press(pi).unwrap();
    for (tool, provider, model) in [
        ("web-search", "openai-codex", "gpt-6.1-sol"),
        ("image-gen", "openai", "gpt-image-1"),
    ] {
        app.press(&format!("id:btn-pi-hosted-tools-{tool}-route"))
            .unwrap();
        app.press(&format!("id:seg-pi-hosted-tools-{tool}-{provider}"))
            .unwrap();
        app.type_text(&format!("id:field-pi-hosted-tools-{tool}-model"), model)
            .unwrap();
    }
    app.press("id:btn-pi-hosted-tools-save").unwrap();
    app.wait_for("text:Hosted tools saved.").unwrap();
    app.back().unwrap();
    app.press("id:settings-config").unwrap();
    app.press("id:tab-config-3").unwrap();
    app.press(pi).unwrap();
    for (tool, expected) in [("web-search", "gpt-6.1-sol"), ("image-gen", "gpt-image-1")] {
        let id = format!("id:field-pi-hosted-tools-{tool}-model");
        app.wait_for(&id).unwrap();
        assert_eq!(app.screen().input_text(&id).unwrap(), Some(expected));
    }
}

#[test]
fn phone_profile_edits_survive_returning_to_the_list_and_switching_tabs() {
    let mut app = preview(phone(), &[]);
    app.press("id:nav-settings").unwrap();
    app.press("id:settings-config").unwrap();
    app.press("id:tab-config-2").unwrap();
    let row = "id:seg-config-profiles-pi-luna";
    let model = "id:field-config-profiles.pi-luna.model";
    app.press(row).unwrap();
    app.type_text(model, "-custom").unwrap();
    let drafted = app.screen().input_text(model).unwrap().unwrap().to_owned();
    assert_ne!(drafted, "luna");
    // The standard Back returns to the list without asking to leave Configuration.
    app.back().unwrap();
    app.wait_for_gone(model, Duration::from_secs(5)).unwrap();
    app.wait_for(row).unwrap();
    assert_eq!(app.screen().count("id:confirm").unwrap(), 0);
    app.press("id:tab-config-4").unwrap();
    app.wait_for("id:btn-config-rule-add").unwrap();
    app.press("id:tab-config-2").unwrap();
    app.press(row).unwrap();
    assert_eq!(
        app.screen().input_text(model).unwrap(),
        Some(drafted.as_str())
    );
    // From the list, Back leaves Configuration, which asks first for the unsaved edit.
    app.back().unwrap();
    app.wait_for(row).unwrap();
    app.back().unwrap();
    app.wait_for("id:confirm").unwrap();
}

#[test]
fn typing_in_ask_does_not_navigate() {
    let mut app = preview(desktop(), &[]);
    app.press("id:nav-tasks").unwrap();
    app.wait_for("id:list-tasks").unwrap();
    app.press_key("/").unwrap();
    app.wait_for("id:ask-input").unwrap();

    let draft = "h s a n p r u f , i j k .";
    app.type_text("id:ask-input", draft).unwrap();

    app.wait_for("id:list-tasks").unwrap();
    assert_eq!(
        app.screen().input_text("id:ask-input").unwrap(),
        Some(draft)
    );
}

/// `/` opens Ask over a page, or on Home puts the focus in its docked composer; the phone's
/// floating button opens it too. Either way the field has the keys at once.
#[test]
fn ask_opens_with_the_field_focused() {
    let mut app = preview(desktop(), &[]);
    app.press_key("/").unwrap();
    app.wait_for("id:ask-input").unwrap();
    // Selector-free keystrokes cannot give an unfocused field the focus.
    for key in ["h", "e", "l", "l", "o"] {
        app.press_key(key).unwrap();
    }
    assert_eq!(
        app.screen().input_text("id:ask-input").unwrap(),
        Some("hello")
    );
    drop(app);

    let mut app = preview(phone(), &[]);
    app.press("id:ask-open").unwrap();
    app.wait_for("id:ask-input").unwrap();
    for key in ["h", "e", "l", "l", "o"] {
        app.press_key(key).unwrap();
    }
    assert_eq!(
        app.screen().input_text("id:ask-input").unwrap(),
        Some("hello")
    );
}

/// The desktop's Home has no composer of its own: `/` opens Ask in the side pane, and Escape
/// closes it so `j` walks Home's cards rather than typing into the field.
#[test]
fn home_asks_in_the_side_pane_and_escape_gives_the_keys_back() {
    let mut app = preview(desktop(), &[("corral.destination", "home")]);
    app.wait_for("id:list-home").unwrap();
    assert_eq!(app.screen().count("id:home-composer").unwrap(), 0);
    app.press_key("/").unwrap();
    app.wait_for("id:side-pane >> id:ask-input").unwrap();
    app.press_key("x").unwrap();
    assert_eq!(app.screen().input_text("id:ask-input").unwrap(), Some("x"));
    app.press_key("escape").unwrap();
    assert_eq!(app.screen().count("id:side-pane").unwrap(), 0);
    app.press_key("j").unwrap();
    assert_eq!(app.screen().count("id:side-pane").unwrap(), 0);
    app.press("id:side-pane-open-bar").unwrap();
    assert_eq!(app.screen().input_text("id:ask-input").unwrap(), Some("x"));
}

#[test]
fn palette_free_text_opens_ask_with_the_text() {
    let mut app = preview(desktop(), &[]);
    app.press("id:palette-open").unwrap();
    app.wait_for("id:palette-input").unwrap();
    let draft = "Why do violet badgers collect moonstones?";
    app.type_text("id:palette-input", draft).unwrap();
    let row = format!("text:Ask: “{draft}”");
    app.wait_for(&row).unwrap();
    app.press(&row).unwrap();

    app.wait_for("id:ask-input").unwrap();
    app.wait_for_gone("id:palette-input", Duration::from_secs(5))
        .unwrap();
    assert_eq!(
        app.screen().input_text("id:ask-input").unwrap(),
        Some(draft)
    );
    assert_eq!(app.screen().count(&row).unwrap(), 0);
}

#[test]
fn disconnected_state_shows_no_healthy_segment_and_no_count() {
    for config in [desktop(), phone()] {
        let mut app = with_storage(config, &[]);
        app.wait_for("text:not connected").unwrap();
        // The destination says why it is empty and offers the way to connect and the preview.
        app.wait_for("id:btn-connect").unwrap();
        assert!(app.screen().count("id:btn-preview").unwrap() > 0);
        let texts = app.screen().texts();
        assert!(texts.contains(&"corrald"));
        assert!(texts.contains(&"not connected"));
        for healthy in ["healthy", "reachable"] {
            assert!(!texts.contains(&healthy), "disconnected shows {healthy}");
        }
        assert_eq!(app.screen().count("id:count-home").unwrap(), 0);
    }
}

#[test]
fn sending_from_ask_appends_to_the_quick_thread_and_clears_the_draft() {
    let mut app = preview(desktop(), &[("corral.destination", "tasks")]);
    app.wait_for("id:list-tasks").unwrap();
    app.press_key("/").unwrap();
    app.wait_for("id:ask-transcript").unwrap();
    let message = "Why do violet badgers collect moonstones?";
    app.type_text("id:ask-input", message).unwrap();
    assert!(app.screen().enabled("id:btn-send").unwrap());
    app.press("id:btn-send").unwrap();
    app.wait_for(&format!("text:{message}")).unwrap();
    assert_eq!(app.screen().input_text("id:ask-input").unwrap(), Some(""));
    app.press("id:pane-close").unwrap();
    app.press_key("/").unwrap();
    app.wait_for(&format!("text:{message}")).unwrap();
}

#[test]
fn ending_ask_clears_the_thread_before_the_next_send() {
    let mut app = preview(desktop(), &[("corral.destination", "tasks")]);
    app.wait_for("id:list-tasks").unwrap();
    app.press_key("/").unwrap();
    app.wait_for("id:ask-transcript").unwrap();
    let previous = "A message in the old thread";
    app.type_text("id:ask-input", previous).unwrap();
    app.press("id:btn-send").unwrap();
    app.wait_for(&format!("text:{previous}")).unwrap();
    let thread = |app: &Headless| {
        app.screen()
            .texts()
            .iter()
            .find_map(|text| text.strip_prefix("Ask · A-").map(str::to_owned))
            .expect("a concrete assistant thread")
    };
    let original = thread(&app);
    app.press("id:btn-end-thread").unwrap();
    app.wait_for("id:confirm").unwrap();
    app.press("id:confirm-confirm").unwrap();
    app.wait_for_gone("id:btn-end-thread", Duration::from_secs(5))
        .unwrap();
    assert_eq!(app.screen().count(&format!("text:{previous}")).unwrap(), 0);
    let next = "A message in the fresh thread";
    app.type_text("id:ask-input", next).unwrap();
    app.press("id:btn-send").unwrap();
    app.wait_for(&format!("text:{next}")).unwrap();
    assert_ne!(thread(&app), original);
    assert_eq!(app.screen().count(&format!("text:{previous}")).unwrap(), 0);
}

/// A restart shows the place stored, and a slug the nine-destination app stored opens the
/// place that took its content over.
#[test]
fn destination_is_restored_from_storage() {
    for (config, phone) in [(desktop(), false), (phone(), true)] {
        for stored in ["tasks", "plans", "review"] {
            let mut app = preview(config.clone(), &[("corral.destination", stored)]);
            app.wait_for("id:list-tasks").unwrap();
            assert_text(&mut app, "page-title", "Tasks");
        }
        for stored in ["inbox", "now"] {
            let mut app = preview(config.clone(), &[("corral.destination", stored)]);
            if phone {
                assert_text(&mut app, "page-title", "Home");
            } else {
                app.wait_for("id:list-home").unwrap();
            }
        }
    }
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

/// Tasks selects its first row on arrival and shows it in the inspector; a press selects
/// another row without leaving the list, a second press opens the task's page, and Escape
/// there returns to Tasks with the row still selected.
#[test]
fn a_task_row_selects_into_the_inspector_and_opens_its_page_on_the_second_press() {
    let mut app = preview(desktop(), &[("corral.destination", "tasks")]);
    app.wait_for("id:inspector").unwrap();
    assert!(selected(&app, "task-T-8"));
    app.press("id:task-T-12").unwrap();
    app.wait_for("id:btn-open-page").unwrap();
    assert!(selected(&app, "task-T-12"));
    assert_eq!(app.screen().count("id:task-page").unwrap(), 0);
    app.press("id:task-T-12").unwrap();
    app.wait_for("id:task-page").unwrap();
    assert!(selected(&app, "nav-tasks"));
    app.press_key("escape").unwrap();
    app.wait_for("id:list-tasks").unwrap();
    assert!(selected(&app, "task-T-12"));
}

/// `j` and `k` walk the rows, Enter opens the selected one's page, and with the inspector
/// hidden (`i`) one press on a row opens its page.
#[test]
fn the_keys_walk_the_tasks_list_and_a_hidden_inspector_opens_rows_on_one_press() {
    let mut app = preview(desktop(), &[("corral.destination", "tasks")]);
    app.wait_for("id:inspector").unwrap();
    assert!(selected(&app, "task-T-8"));
    app.press_key("j").unwrap();
    app.wait_for("id:inspector").unwrap();
    assert!(!selected(&app, "task-T-8"));
    app.press_key("k").unwrap();
    assert!(selected(&app, "task-T-8"));
    app.press_key("enter").unwrap();
    app.wait_for("id:task-page").unwrap();
    app.press_key("escape").unwrap();
    app.wait_for("id:list-tasks").unwrap();
    app.press_key("i").unwrap();
    app.wait_for_gone("id:inspector", Duration::from_secs(5))
        .unwrap();
    app.press("id:task-T-12").unwrap();
    app.wait_for("id:task-page").unwrap();
}

/// The one project scope narrows every place at once: scoping to a project from the sidebar's
/// switcher leaves Tasks without the other projects' rows, and All projects brings them back.
#[test]
fn the_scope_switcher_scopes_every_place_to_one_project() {
    let mut app = preview(desktop(), &[("corral.destination", "tasks")]);
    // T-8 is the corral project's.
    app.wait_for("id:task-T-8").unwrap();
    app.press("id:scope-open").unwrap();
    app.wait_for("id:scope-all").unwrap();
    app.press("id:scope-site").unwrap();
    app.wait_for_gone("id:task-T-8", Duration::from_secs(5))
        .unwrap();
    app.press("id:nav-home").unwrap();
    app.wait_for("id:list-home").unwrap();
    assert_eq!(app.screen().count("id:task-T-8").unwrap(), 0);
    app.press("id:scope-open").unwrap();
    app.wait_for("id:scope-all").unwrap();
    app.press("id:scope-all").unwrap();
    app.wait_for("id:task-T-8").unwrap();
}

/// The successor of answering an Inbox question: the words typed in a Blocked task's reply
/// box on its page go out as its reply step, the task runs again and leaves Needs you.
#[test]
fn replying_from_a_blocked_tasks_page_sends_the_reply_and_the_task_leaves_needs_you() {
    let mut app = preview(desktop(), &[("corral.destination", "home")]);
    let waiting = count_of(&mut app, "count-home");
    app.press("id:task-T-8").unwrap();
    app.wait_for("id:field-reply").unwrap();
    assert!(!app.screen().enabled("id:btn-next-0").unwrap());
    app.type_text("id:field-reply", "Stay paused; never deliver twice.")
        .unwrap();
    app.press("id:btn-next-0").unwrap();
    app.wait_for_gone("id:field-reply", Duration::from_secs(5))
        .unwrap();
    assert_eq!(count_of(&mut app, "count-home"), waiting - 1);
}

/// The successor of the phone's question page: a Blocked task's page shows its reply box, and
/// Back returns to Home's list.
#[test]
fn phone_blocked_task_page_goes_back_to_homes_list() {
    let mut app = preview(phone(), &[]);
    app.press("id:task-T-8").unwrap();
    app.wait_for("id:field-reply").unwrap();
    app.press("id:page-back").unwrap();
    app.wait_for("id:task-T-8").unwrap();
    assert_eq!(app.screen().count("id:task-page").unwrap(), 0);
    assert_text(&mut app, "page-title", "Home");
}

/// The successor of the 80-column Inbox: at 80 × 24 Home keeps what needs you on the page, its
/// heading and its first row in view.
#[test]
fn terminal_80_columns_keeps_what_needs_you_in_view() {
    let mut app = preview(terminal_80(), &[]);
    app.wait_for("id:section-needs-you").unwrap();
    let nodes = app.screen().layout_nodes();
    for id in ["section-needs-you", "task-T-8"] {
        let node = nodes
            .iter()
            .find(|node| node.test_id.as_deref() == Some(id))
            .unwrap_or_else(|| panic!("id:{id} is on the page"));
        let bounds = &node.bounds;
        assert!(
            bounds.size.height.0 > 0.0 && bounds.origin.y.0 + bounds.size.height.0 <= 384.0,
            "id:{id} is in view: {bounds:?}"
        );
    }
}

/// The engine refuses dispatching T-12 while T-14 is not done. The sheet offers "Dispatch
/// anyway", which names what it skips and is the one confirmation: pressing it submits the same
/// dispatch with force, T-12 runs, and Activity's Feed records the forced operation.
#[test]
fn a_refused_dispatch_offers_dispatch_anyway_and_forcing_it_runs_the_task() {
    let mut app = preview(desktop(), &[("corral.destination", "now")]);
    dispatch_t12(&mut app);
    app.wait_for("id:refused").unwrap();
    app.wait_for("text:Dispatch anyway").unwrap();
    assert!(app.screen().count("id:btn-refusal-0").unwrap() > 0);
    app.wait_for("text:/^Skips unfinished dependencies/")
        .unwrap();
    app.press("id:btn-refusal-0").unwrap();
    app.wait_for_gone("id:refused", Duration::from_secs(5))
        .unwrap();
    assert_eq!(app.screen().count("id:confirm").unwrap(), 0);
    app.wait_for("text:/^Dispatch anyway T-12 · Done/").unwrap();
    app.press("id:nav-sessions").unwrap();
    app.wait_for("id:agent-T-12_R1").unwrap();
    app.press("id:nav-activity").unwrap();
    app.wait_for("text:/^dispatch · skipped unfinished dependencies/")
        .unwrap();
}

/// The keyboard's Actions menu (`.`) took no arrows, so Down three times and Enter ran its first
/// item, Edit. Down moves through the items and Enter runs the one it reached, the fourth
/// (Dispatch); End and Home reach the last and the first.
#[test]
fn arrow_keys_walk_the_actions_menu_and_enter_runs_the_item_reached() {
    let mut app = preview(desktop(), &[("corral.destination", "now")]);
    app.press("id:task-T-8").unwrap();
    app.press_key(".").unwrap();
    app.wait_for("id:verbs").unwrap();
    app.press_key("end").unwrap();
    app.press_key("home").unwrap();
    for _ in 0..3 {
        app.press_key("down").unwrap();
    }
    app.press_key("enter").unwrap();
    app.wait_for("id:btn-verb-send").unwrap();
    app.wait_for("text:Dispatch T-8").unwrap();
    assert_eq!(app.screen().count("text:Edit T-8").unwrap(), 0);
}

/// A destructive operation asks once, locally, with the focus on Cancel: Enter cancels, the
/// receipt says nothing was sent, and the task is still there.
#[test]
fn the_confirm_sheet_takes_the_focus_so_enter_cancels_it() {
    let mut app = preview(desktop(), &[("corral.destination", "now")]);
    select(&mut app, "T-12 Timeline zoom presets");
    select(&mut app, "Drop T-12");
    app.wait_for("id:confirm").unwrap();
    app.press_key("enter").unwrap();
    app.wait_for_gone("id:confirm", Duration::from_secs(5))
        .unwrap();
    app.wait_for("text:/ · cancelled, nothing was sent$/")
        .unwrap();
}

/// T-10's gate is red, so approving it from More is refused, and the refusal offers Land
/// anyway, which names what it skips and is the one confirmation: pressing it submits approve
/// with force and the task lands and leaves Review.
#[test]
fn land_anyway_over_a_red_gate_is_offered_by_the_refusal_and_lands() {
    let mut app = preview(desktop(), &[("corral.destination", "tasks")]);
    select(&mut app, "T-10 Usage page explains missing records");
    app.wait_for("id:task-page").unwrap();
    select(&mut app, "Approve and land… T-10");
    app.wait_for("id:btn-verb-send").unwrap();
    app.press("id:btn-verb-send").unwrap();
    app.wait_for("id:confirm").unwrap();
    app.press("id:confirm-confirm").unwrap();
    app.wait_for("id:refused").unwrap();
    app.wait_for("text:/the gate check/").unwrap();
    // The page's decide bar offers Land anyway too; this is the refusal's own step.
    app.press("id:btn-refusal-1").unwrap();
    app.wait_for("text:/^Land anyway T-10 · Done/").unwrap();
}

/// A review the engine marked ready shows Approve as its one next step: one press asks once,
/// and confirming lands the task.
#[test]
fn a_ready_review_lands_from_its_one_next_step() {
    let mut app = preview(desktop(), &[("corral.destination", "tasks")]);
    app.wait_for("id:next-T-11").unwrap();
    app.press("id:next-T-11").unwrap();
    app.wait_for("id:confirm").unwrap();
    app.press("id:confirm-confirm").unwrap();
    app.wait_for("text:/^Approve and land T-11 · Done/")
        .unwrap();
}

/// Approve lands onto the branch typed in Onto. A branch the repository lacks is refused, and the
/// refusal offers one Approve per existing branch; choosing one asks once and lands there.
#[test]
fn approving_onto_a_missing_branch_offers_each_existing_branch() {
    let mut app = preview(desktop(), &[("corral.destination", "tasks")]);
    select(&mut app, "T-11 Collapse the sidebar on narrow screens");
    app.wait_for("id:task-page").unwrap();
    select(&mut app, "Approve and land… T-11");
    app.wait_for("id:field-verb-0").unwrap();
    app.clear("id:field-verb-0").unwrap();
    app.type_text("id:field-verb-0", "release/1.0").unwrap();
    app.press("id:btn-verb-send").unwrap();
    app.wait_for("text:/--onto release\\/1.0/").unwrap();
    app.press("id:confirm-confirm").unwrap();
    app.wait_for("id:refused").unwrap();
    app.wait_for("text:Approve onto release/0.9").unwrap();
    app.press("id:btn-refusal-1").unwrap();
    app.wait_for("id:confirm").unwrap();
    app.press("id:confirm-confirm").unwrap();
    app.wait_for("text:/^Approve onto release\\/0.9 T-11 · Done/")
        .unwrap();
}

/// The grouping switch asks the feed for the other grouping and redraws the lanes from its
/// answer: By run draws one lane per run, which By task does not have, and a live run's lane
/// opens its session page: T-15's headless run has no Pi host, so the page is its read-only log.
#[test]
fn timeline_grouping_redraws_the_lanes_from_the_feed() {
    let mut app = preview(terminal_80(), &[("corral.destination", "timeline")]);
    app.wait_for("id:lane-T-15").unwrap();
    assert_eq!(app.screen().count("id:lane-T-15/R1").unwrap(), 0);
    app.press("id:seg-group-1").unwrap();
    app.wait_for("id:lane-T-15/R1").unwrap();
    app.press("id:lane-T-15/R1").unwrap();
    app.wait_for("id:session-log").unwrap();
}

/// A file copied in a file manager arrives as its path alone. Pasted into Ask, which takes
/// attachments, it is routed to attachment handling, not typed into the field: the sample
/// preview refuses it visibly, since only a connected corrald takes uploads.
#[test]
fn a_file_manager_path_paste_is_an_attachment_not_text() {
    let mut app = preview(desktop(), &[]);
    app.press_key("/").unwrap();
    app.wait_for("id:ask-input").unwrap();
    let mut paste = PasteEvent::default();
    paste
        .uris
        .push("file:///clipboard-fixture/meeting%20notes.txt".into());
    app.paste("id:ask-input", &paste).unwrap();
    app.wait_for("id:toast").unwrap();
    assert_eq!(app.screen().input_text("id:ask-input").unwrap(), Some(""));
}

#[test]
fn desktop_ask_does_not_cover_the_tasks_list() {
    let mut app = preview(desktop(), &[("corral.destination", "tasks")]);
    app.wait_for("id:list-tasks").unwrap();
    app.press_key("/").unwrap();
    app.wait_for("id:ask-pane").unwrap();
    let nodes = app.screen().layout_nodes();
    let bounds = |id| {
        nodes
            .iter()
            .find(|n| n.test_id.as_deref() == Some(id))
            .unwrap()
            .bounds
    };
    let list = bounds("list-tasks");
    assert!(list.origin.x + list.size.width <= bounds("ask-pane").origin.x);
}

#[test]
fn desktop_ask_does_not_block_page_keys_after_focus_leaves_the_pane() {
    let mut app = preview(desktop(), &[("corral.destination", "tasks")]);
    app.press_key("/").unwrap();
    app.wait_for("id:ask-input").unwrap();
    app.press_key("tab").unwrap();
    app.press("id:nav-tasks").unwrap();
    assert!(selected(&app, "task-T-8"));
    app.press_key("j").unwrap();
    assert!(!selected(&app, "task-T-8"));
    app.press_key("s").unwrap();
    app.wait_for("id:list-sessions").unwrap();
    assert_eq!(app.screen().count("id:pane-close").unwrap(), 1);
}

#[test]
fn build_opened_beside_stays_visible_after_navigation() {
    let mut app = preview(desktop(), &[]);
    app.press("id:nav-sessions").unwrap();
    app.press("id:agent-B-k3x9").unwrap();
    app.press("id:list-sessions >> id:more-B-k3x9").unwrap();
    app.press("id:btn-open-beside").unwrap();
    app.wait_for("id:dock-tab-B-k3x9").unwrap();
    app.press("id:nav-tasks").unwrap();
    app.wait_for("id:list-tasks").unwrap();
    app.wait_for("id:dock-transcript").unwrap();
    app.wait_for("id:dock-tab-B-k3x9").unwrap();
    app.press("id:dock-pane >> id:btn-open-page").unwrap();
    app.wait_for("id:conversation-beside").unwrap();
    app.wait_for("id:dock-transcript").unwrap();
}

#[test]
fn narrow_dock_dismisses_without_undocking_or_leaving_the_page() {
    let mut config = desktop();
    config.viewport = Size::new(Points(800.0), Points(900.0));
    let mut app = preview(config, &[]);
    app.press("id:nav-sessions").unwrap();
    app.press("id:agent-B-k3x9").unwrap();
    app.press("id:btn-dock-side").unwrap();
    app.press("id:pane-close").unwrap();
    assert_eq!(app.screen().count("id:dock-pane").unwrap(), 0);
    app.press("id:nav-tasks").unwrap();
    app.press("id:side-pane-toggle").unwrap();
    app.wait_for("id:dock-tab-B-k3x9").unwrap();
    app.press("id:dock-pane >> id:btn-open-page").unwrap();
    assert_eq!(app.screen().count("id:dock-pane").unwrap(), 0);
    assert_eq!(app.screen().count("id:list-tasks").unwrap(), 0);
    app.press("id:side-pane-toggle").unwrap();
    app.wait_for("id:dock-transcript").unwrap();
    app.press_key("escape").unwrap();
    assert_eq!(app.screen().count("id:dock-pane").unwrap(), 0);
    assert_eq!(app.screen().count("id:list-sessions").unwrap(), 0);
    app.press("id:side-pane-toggle").unwrap();
    app.wait_for("id:dock-tab-B-k3x9").unwrap();
    app.wait_for("id:dock-transcript").unwrap();
    app.press("id:dock-details-toggle").unwrap();
    app.press("id:dock-pane >> id:side-step-0").unwrap();
    app.wait_for("id:ledger").unwrap();
    assert_eq!(app.screen().count("id:dock-pane").unwrap(), 0);
}

/// A task cited in the side pane's chat opens that task's page and keeps the chat as it was:
/// beside a wide page the pane stays; over a narrow one it steps aside, then one press brings
/// it back with its draft, instead of closing unasked.
#[test]
fn a_task_cited_in_the_side_pane_opens_without_closing_the_chat() {
    for (width, narrow) in [(1440.0, false), (800.0, true)] {
        let mut config = desktop();
        config.viewport = Size::new(Points(width), Points(900.0));
        let mut app = preview(config, &[("corral.destination", "tasks")]);
        app.wait_for("id:list-tasks").unwrap();
        app.press_key("/").unwrap();
        app.wait_for("id:side-pane >> id:cite-T-14").unwrap();
        app.type_text("id:ask-input", "still here").unwrap();
        app.press("id:side-pane >> id:cite-T-14").unwrap();
        app.wait_for("id:task-page >> id:more-T-14").unwrap();
        if narrow {
            assert_eq!(app.screen().count("id:side-pane").unwrap(), 0);
            app.press("id:pane-back").unwrap();
        }
        app.wait_for("id:side-pane >> id:cite-T-14").unwrap();
        assert_eq!(
            app.screen().input_text("id:ask-input").unwrap(),
            Some("still here")
        );
        app.wait_for("id:task-page >> id:more-T-14").unwrap();
    }
}

/// A narrow pane set aside stays mounted, hidden, but lets go of the keys: Enter and Alt+Enter
/// on the page it stepped aside for do not send its hidden draft, which comes back as it was.
#[test]
fn a_pane_set_aside_does_not_send_its_hidden_draft() {
    let mut config = desktop();
    config.viewport = Size::new(Points(800.0), Points(900.0));
    let mut app = preview(config, &[("corral.destination", "tasks")]);
    app.wait_for("id:list-tasks").unwrap();
    app.press_key("/").unwrap();
    app.wait_for("id:side-pane >> id:cite-T-14").unwrap();
    app.type_text("id:ask-input", "still here").unwrap();
    app.press("id:side-pane >> id:cite-T-14").unwrap();
    app.wait_for("id:task-page >> id:more-T-14").unwrap();
    assert_eq!(app.screen().count("id:side-pane").unwrap(), 0);
    for key in ["enter", "alt+enter"] {
        app.press_key(key).unwrap();
    }
    app.press("id:pane-back").unwrap();
    app.wait_for("id:side-pane >> id:cite-T-14").unwrap();
    assert_eq!(
        app.screen().input_text("id:ask-input").unwrap(),
        Some("still here")
    );
}

/// A session's answer card in a narrow pane set aside lets go of the keys too: its words field,
/// hidden with the pane, keeps what was typed, and Enter on the page answers nothing.
#[test]
fn a_pane_set_aside_does_not_send_its_hidden_answer() {
    const FIELD: &str = "id:side-pane >> id:field-answer-q-1-0";
    let mut config = desktop();
    config.viewport = Size::new(Points(800.0), Points(900.0));
    let mut app = preview(config, &[("corral.destination", "tasks")]);
    app.wait_for("id:list-tasks").unwrap();
    app.press_key("ctrl+g").unwrap();
    app.press("id:switch-side-A-15").unwrap();
    app.wait_for("id:dock-tab-A-15").unwrap();
    app.press("id:side-pane >> id:dock-q-1").unwrap();
    app.wait_for(FIELD).unwrap();
    app.wait_for("id:side-pane >> id:cite-T-14").unwrap();
    app.type_text(FIELD, "UTC everywhere").unwrap();
    app.press("id:side-pane >> id:cite-T-14").unwrap();
    app.wait_for("id:task-page >> id:more-T-14").unwrap();
    assert_eq!(app.screen().count("id:side-pane").unwrap(), 0);
    app.press_key("enter").unwrap();
    app.press("id:pane-back").unwrap();
    app.wait_for(FIELD).unwrap();
    assert_eq!(
        app.screen().input_text(FIELD).unwrap(),
        Some("UTC everywhere")
    );
    assert_eq!(
        app.screen().count("id:field-answer-q-2-0").unwrap(),
        0,
        "the hidden answer was not sent"
    );
}

/// Escape on Configuration with a profile shown alone goes back to the profile list while Ask's
/// narrow pane is set aside: held, hidden, it does not stand over the page.
#[test]
fn escape_returns_to_the_profile_list_while_the_pane_is_aside() {
    // Narrow enough that the page, even with the pane aside, has no room for the profile list
    // beside a profile (Configuration's split wants 640 points).
    let mut config = desktop();
    config.viewport = Size::new(Points(640.0), Points(900.0));
    let mut app = preview(
        config,
        &[
            ("corral.destination", "settings"),
            ("corral.settings", "config"),
        ],
    );
    app.press("id:side-pane-toggle").unwrap();
    app.wait_for("id:side-pane >> id:cite-T-14").unwrap();
    app.press("id:side-pane >> id:cite-T-14").unwrap();
    app.wait_for("id:task-page >> id:more-T-14").unwrap();
    app.press("id:nav-settings").unwrap();
    app.wait_for("id:tab-config-2").unwrap();
    assert_eq!(app.screen().count("id:pane-back").unwrap(), 1);

    app.press("id:tab-config-2").unwrap();
    let row = "id:seg-config-profiles-pi-luna";
    let model = "id:field-config-profiles.pi-luna.model";
    app.press(row).unwrap();
    app.wait_for(model).unwrap();
    // The profile shows alone, in its list's place.
    assert_eq!(app.screen().count(row).unwrap(), 0);

    app.press_key("escape").unwrap();
    app.wait_for(row).unwrap();
    assert_eq!(app.screen().count(model).unwrap(), 0);
    assert_eq!(app.screen().count("id:tab-config-2").unwrap(), 1);
    assert_eq!(app.screen().count("id:pane-back").unwrap(), 1);
}

/// Only a citation pressed in the pane sets a narrow pane aside: one pressed in the page under
/// it opens what it cites and leaves the pane over the page as it was.
#[test]
fn a_citation_pressed_in_the_page_leaves_a_narrow_pane_over_it() {
    let mut config = desktop();
    config.viewport = Size::new(Points(1000.0), Points(900.0));
    let mut app = preview(config, &[("corral.destination", "tasks")]);
    app.wait_for("id:list-tasks").unwrap();
    app.press("id:palette-open").unwrap();
    app.wait_for("id:palette-input").unwrap();
    app.type_text("id:palette-input", "T-12 Timeline zoom presets")
        .unwrap();
    app.press("text:T-12 Timeline zoom presets").unwrap();
    app.wait_for_gone("id:palette-input", Duration::from_secs(5))
        .unwrap();
    app.wait_for("id:cite-T-2:3#acceptance").unwrap();
    app.press("id:side-pane-toggle").unwrap();
    app.wait_for("id:side-pane >> id:cite-T-14").unwrap();
    app.type_text("id:ask-input", "still here").unwrap();
    app.press("id:task-page >> id:cite-T-2:3#acceptance")
        .unwrap();
    app.wait_for("id:artifact-full").unwrap();
    app.wait_for("id:side-pane >> id:cite-T-14").unwrap();
    assert_eq!(app.screen().count("id:pane-back").unwrap(), 0);
    assert_eq!(
        app.screen().input_text("id:ask-input").unwrap(),
        Some("still here")
    );
}

/// A citation pressed in a narrow pane over Configuration with an unsaved edit asks first. The
/// pane stays over the page while the sheet asks and after Cancel (the edit kept), and steps
/// aside only once Discard has left for the cited page.
#[test]
fn a_pane_citation_from_unsaved_configuration_steps_aside_only_once_it_leaves() {
    let mut config = desktop();
    config.viewport = Size::new(Points(800.0), Points(900.0));
    let mut app = preview(
        config,
        &[
            ("corral.destination", "settings"),
            ("corral.settings", "config"),
        ],
    );
    app.press("id:tab-config-2").unwrap();
    let model = "id:field-config-profiles.pi-luna.model";
    app.press("id:seg-config-profiles-pi-luna").unwrap();
    app.type_text(model, "-custom").unwrap();
    let drafted = app.screen().input_text(model).unwrap().unwrap().to_owned();
    app.press("id:side-pane-toggle").unwrap();
    app.wait_for("id:side-pane >> id:cite-T-14").unwrap();

    app.press("id:side-pane >> id:cite-T-14").unwrap();
    app.wait_for("id:confirm").unwrap();
    assert_eq!(app.screen().count("id:side-pane").unwrap(), 1);
    assert_eq!(app.screen().count("id:pane-back").unwrap(), 0);
    app.press("id:confirm-cancel").unwrap();
    app.wait_for_gone("id:confirm", Duration::from_secs(5))
        .unwrap();
    app.wait_for("id:side-pane >> id:cite-T-14").unwrap();
    assert_eq!(app.screen().count("id:pane-back").unwrap(), 0);
    assert_eq!(app.screen().count("id:task-page").unwrap(), 0);
    assert_eq!(
        app.screen().input_text(model).unwrap(),
        Some(drafted.as_str())
    );

    app.press("id:side-pane >> id:cite-T-14").unwrap();
    app.wait_for("id:confirm").unwrap();
    app.press("id:confirm-confirm").unwrap();
    app.wait_for("id:task-page >> id:more-T-14").unwrap();
    assert_eq!(app.screen().count("id:side-pane").unwrap(), 0);
    app.press("id:pane-back").unwrap();
    app.wait_for("id:side-pane >> id:cite-T-14").unwrap();
}

#[test]
fn docked_plan_and_file_open_in_page_without_replacing_the_pane() {
    let mut app = preview(desktop(), &[]);
    app.press("id:nav-sessions").unwrap();
    app.press("id:agent-B-k3x9").unwrap();
    app.press("id:agent-B-k3x9").unwrap();
    app.press("id:btn-dock-side").unwrap();
    app.wait_for("id:dock-details-toggle").unwrap();
    app.press("id:nav-tasks").unwrap();
    app.press("id:dock-details-toggle").unwrap();
    app.press("id:dock-pane >> id:details-changes").unwrap();
    app.press("id:dock-pane >> id:details-agents").unwrap();
    app.press("id:dock-pane >> id:side-step-0").unwrap();
    app.wait_for("id:ledger").unwrap();
    app.wait_for("id:dock-transcript").unwrap();
    app.press("id:dock-pane >> id:details-changes").unwrap();
    assert_eq!(
        app.screen()
            .count("id:dock-pane >> id:side-file-0")
            .unwrap(),
        0,
        "the file list stays folded under the summary"
    );
    app.press("id:dock-pane >> id:side-files-preview").unwrap();
    app.wait_for("id:dock-pane >> id:side-file-0").unwrap();
    app.press("id:dock-pane >> id:side-file-0").unwrap();
    app.wait_for("id:session-changes").unwrap();
    app.wait_for("id:changes-file").unwrap();
    assert_eq!(app.screen().count("id:ledger").unwrap(), 0);
    app.wait_for("id:dock-tab-B-k3x9").unwrap();
    app.wait_for("id:dock-transcript").unwrap();
}

#[test]
fn dock_context_is_sent_by_the_pane_but_not_shared_with_ask() {
    let mut app = preview(desktop(), &[]);
    app.press("id:nav-sessions").unwrap();
    app.press("id:agent-B-k3x9").unwrap();
    app.press("id:agent-B-k3x9").unwrap();
    app.wait_for("id:btn-dock-side").unwrap();
    app.press("id:btn-dock-side").unwrap();
    app.wait_for("id:btn-dock-attach-page").unwrap();
    app.press("id:btn-dock-attach-page").unwrap();
    app.wait_for("id:btn-dock-remove-context").unwrap();
    app.press("id:nav-tasks").unwrap();
    app.wait_for("id:btn-dock-remove-context").unwrap();
    app.type_text("id:dock-pane >> id:field-message", "Review this context.")
        .unwrap();
    app.press("id:dock-pane >> id:btn-send").unwrap();
    let page = nsh_app_corral::model::Live::Session(nsh_app_corral::model::Id::new("B-k3x9"));
    let text = nsh_app_corral::model::assistant::with_context(
        "Review this context.",
        Some(&page.context(None)),
        None,
    );
    let context = text.lines().next().unwrap();
    app.wait_for(&format!("text:/^{context}/")).unwrap();
    app.press("id:dock-tab-ask").unwrap();
    app.wait_for("id:ask-pane >> id:ask-input").unwrap();
    assert_eq!(
        app.screen().count("id:ask-pane >> id:btn-unchip").unwrap(),
        0
    );
    app.press("id:dock-tab-B-k3x9").unwrap();
    app.wait_for("id:dock-pane >> id:btn-dock-remove-context")
        .unwrap();
}

/// Ask and the sessions kept beside the page are tabs of one pane: choosing one keeps the
/// others in the strip, Escape closes the whole pane rather than revealing a session tab in
/// Ask's place, and Ctrl+J brings it back on the session tab last chosen.
#[test]
fn ask_and_session_tabs_share_one_side_pane() {
    let mut app = preview(desktop(), &[]);
    app.press("id:nav-sessions").unwrap();
    app.wait_for("id:agent-B-k3x9").unwrap();
    app.press("id:agent-B-k3x9").unwrap();
    app.wait_for("id:list-sessions >> id:more-B-k3x9").unwrap();
    app.press("id:list-sessions >> id:more-B-k3x9").unwrap();
    app.press("id:btn-open-beside").unwrap();
    app.wait_for("id:dock-transcript").unwrap();
    app.press("id:side-pane >> id:dock-tab-ask").unwrap();
    app.wait_for("id:ask-transcript").unwrap();
    assert_eq!(app.screen().count("id:dock-transcript").unwrap(), 0);
    app.wait_for("id:dock-tab-B-k3x9").unwrap();
    app.press("id:dock-tab-B-k3x9").unwrap();
    app.wait_for("id:dock-transcript").unwrap();
    app.press("id:dock-tab-ask").unwrap();
    app.wait_for("id:ask-transcript").unwrap();
    app.press_key("escape").unwrap();
    assert_eq!(app.screen().count("id:side-pane").unwrap(), 0);
    app.press_key("ctrl+j").unwrap();
    app.wait_for("id:dock-transcript").unwrap();
    app.press("id:dock-tab-B-k3x9-close").unwrap();
    assert_eq!(app.screen().count("id:side-pane").unwrap(), 0);
}

/// The Agents switcher opens a Code session (not only a chat) on a tab of its own in the side
/// pane, leaving the page where it was.
#[test]
fn the_switcher_opens_a_code_session_in_the_side_pane() {
    let mut app = preview(desktop(), &[("corral.destination", "tasks")]);
    app.wait_for("id:list-tasks").unwrap();
    app.press_key("ctrl+g").unwrap();
    app.press("id:switch-side-B-code").unwrap();
    app.wait_for("id:dock-tab-B-code").unwrap();
    app.wait_for("id:dock-transcript").unwrap();
    app.wait_for("id:list-tasks").unwrap();
}

/// The strip's + starts a build that opens on a tab of its own in the side pane, the page
/// staying where it was.
#[test]
fn the_panes_plus_starts_a_build_on_a_tab_of_its_own() {
    let mut app = preview(desktop(), &[("corral.destination", "tasks")]);
    app.wait_for("id:list-tasks").unwrap();
    app.press_key("/").unwrap();
    app.wait_for("id:pane-new").unwrap();
    app.press("id:pane-new").unwrap();
    app.press("id:pane-new-build").unwrap();
    app.wait_for("id:field-build-name").unwrap();
    app.press("id:seg-build-project-corral").unwrap();
    app.type_text("id:field-build-name", "walk").unwrap();
    app.press("id:btn-start-build").unwrap();
    app.wait_for("id:dock-transcript").unwrap();
    app.wait_for("text:walk").unwrap();
    app.wait_for("id:list-tasks").unwrap();
}
