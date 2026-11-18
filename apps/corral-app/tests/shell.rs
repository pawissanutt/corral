use std::time::Duration;

use nsh_core::{
    Points, Size,
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
    app.wait_for("id:ask-open").unwrap();
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

/// Selects a sample task through the palette, as a person who knows its id does.
fn select(app: &mut Headless, row: &str) {
    app.press("id:palette-open").unwrap();
    app.wait_for("id:palette-input").unwrap();
    app.type_text("id:palette-input", row).unwrap();
    app.press(&format!("text:{row}")).unwrap();
    app.wait_for_gone("id:palette-input", Duration::from_secs(5))
        .unwrap();
}

/// Selects the sample's T-12, which waits on T-14, and runs Dispatch from its Actions menu,
/// which the palette also lists for the selected task.
fn dispatch_t12(app: &mut Headless) {
    select(app, "T-12 Timeline zoom presets");
    select(app, "Dispatch… T-12");
    app.wait_for("id:btn-verb-send").unwrap();
    app.press("id:btn-verb-send").unwrap();
}

#[test]
fn both_forms_open_on_now() {
    let mut app = preview(phone(), &[]);
    assert_text(&mut app, "page-title", "Now");
    drop(app);

    let mut app = preview(desktop(), &[]);
    app.wait_for("id:now-main").unwrap();
}

#[test]
fn desktop_sidebar_reaches_every_destination() {
    let mut app = preview(desktop(), &[]);
    for (slug, title) in [
        ("now", "Now"),
        ("changes", "Changes"),
        ("plans", "Plans"),
        ("agents", "Agents"),
        ("review", "Review"),
        ("timeline", "Timeline"),
        ("usage", "Usage"),
        ("settings", "Settings"),
    ] {
        app.press(&format!("id:nav-{slug}")).unwrap();
        if slug == "now" {
            app.wait_for("id:now-main").unwrap();
        } else if slug == "settings" {
            app.wait_for("id:page-title").unwrap();
        } else {
            assert_text(&mut app, "page-title", title);
        }
    }
}

#[test]
fn phone_tabs_and_more_reach_every_destination() {
    let mut app = preview(phone(), &[]);
    for (slug, title) in [("now", "Now"), ("agents", "Agents"), ("plans", "Plans")] {
        app.press(&format!("id:tab-{slug}")).unwrap();
        assert_text(&mut app, "page-title", title);
    }

    app.press("id:tab-more").unwrap();
    for (slug, title) in [
        ("changes", "Changes"),
        ("review", "Review"),
        ("timeline", "Timeline"),
        ("usage", "Usage"),
        ("settings", "Settings"),
    ] {
        app.wait_for(&format!("id:nav-{slug}")).unwrap();
        app.press(&format!("id:nav-{slug}")).unwrap();
        assert_text(&mut app, "page-title", title);
        app.back().unwrap();
        app.wait_for("id:nav-changes").unwrap();
    }
}

#[test]
fn phone_settings_sections_push_their_pages() {
    let mut app = preview(phone(), &[]);
    app.press("id:tab-more").unwrap();
    app.press("id:nav-settings").unwrap();
    assert_text(&mut app, "page-title", "Settings");
    app.wait_for("id:settings-devices").unwrap();
    app.press("id:settings-config").unwrap();
    assert_text(&mut app, "page-title", "Configuration");
    app.back().unwrap();
    assert_text(&mut app, "page-title", "Settings");
}

#[test]
fn typing_in_ask_does_not_navigate() {
    let mut app = preview(desktop(), &[]);
    app.press("id:nav-plans").unwrap();
    app.wait_for("id:plans-list").unwrap();
    app.press("id:ask-open").unwrap();
    app.wait_for("id:ask-input").unwrap();

    let draft = "n i p a r t u . , p";
    app.type_text("id:ask-input", draft).unwrap();

    app.wait_for("id:plans-list").unwrap();
    assert_eq!(
        app.screen().input_text("id:ask-input").unwrap(),
        Some(draft)
    );
}

#[test]
fn ask_opens_with_the_field_focused() {
    let mut app = preview(desktop(), &[]);
    app.press("id:ask-open").unwrap();
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
        assert_eq!(app.screen().count("id:count-now").unwrap(), 0);
    }
}

#[test]
fn sending_from_ask_appends_to_the_quick_thread_and_clears_the_draft() {
    let mut app = preview(desktop(), &[]);
    app.press("id:ask-open").unwrap();
    app.wait_for("id:thread-quick-items").unwrap();
    let message = "Why do violet badgers collect moonstones?";
    app.type_text("id:ask-input", message).unwrap();
    assert!(app.screen().enabled("id:btn-send").unwrap());
    app.press("id:btn-send").unwrap();
    app.wait_for(&format!("text:{message}")).unwrap();
    assert_eq!(app.screen().input_text("id:ask-input").unwrap(), Some(""));
    app.press("id:ask-close").unwrap();
    app.press("id:ask-open").unwrap();
    app.wait_for(&format!("text:{message}")).unwrap();
}

#[test]
fn destination_is_restored_from_storage() {
    for (config, phone) in [(desktop(), false), (phone(), true)] {
        let mut app = preview(config.clone(), &[("corral.destination", "plans")]);
        app.wait_for("id:plans-list").unwrap();
        assert_text(&mut app, "page-title", "Plans");
        drop(app);

        // The retired Inbox opens its new home, Now.
        let mut app = preview(config, &[("corral.destination", "inbox")]);
        if phone {
            assert_text(&mut app, "page-title", "Now");
        } else {
            app.wait_for("id:now-main").unwrap();
        }
    }
}

fn selected(app: &Headless, id: &str) -> bool {
    app.screen()
        .accessibility_state(&format!("id:{id}"))
        .is_ok_and(|state| state.selected)
}

/// Now opens at what needs you: the sample's first blocked task is selected beside the list
/// with its reply box, and opening Now again comes back to it from another selection.
#[test]
fn now_opens_at_needs_you_each_time_it_is_opened() {
    let mut app = preview(desktop(), &[("corral.destination", "now")]);
    app.wait_for("id:field-reply").unwrap();
    assert!(selected(&app, "task-T-8"));
    select(&mut app, "T-12 Timeline zoom presets");
    app.wait_for_gone("id:field-reply", Duration::from_secs(5))
        .unwrap();
    app.press("id:nav-plans").unwrap();
    app.wait_for("id:plans-list").unwrap();
    app.press("id:nav-now").unwrap();
    app.wait_for("id:field-reply").unwrap();
    assert!(selected(&app, "task-T-8"));
}

/// The successor of answering an Inbox question: the words typed in a Blocked card's reply box
/// go out as its reply step, the task runs again and leaves Needs you.
#[test]
fn replying_from_a_blocked_cards_box_sends_the_reply_and_the_task_leaves_needs_you() {
    let mut app = preview(desktop(), &[("corral.destination", "now")]);
    app.wait_for("id:field-reply").unwrap();
    assert_text(&mut app, "count-now", "3");
    assert!(!app.screen().enabled("id:btn-next-0").unwrap());
    app.type_text("id:field-reply", "Stay paused; never deliver twice.")
        .unwrap();
    app.press("id:btn-next-0").unwrap();
    app.wait_for_gone("id:field-reply", Duration::from_secs(5))
        .unwrap();
    assert_text(&mut app, "count-now", "2");
}

/// The successor of the phone's question page: a Blocked task's page shows its reply box, and
/// Back returns to Now's list.
#[test]
fn phone_blocked_task_page_goes_back_to_nows_list() {
    let mut app = preview(phone(), &[]);
    app.press("id:task-T-8").unwrap();
    app.wait_for("id:field-reply").unwrap();
    app.press("id:page-back").unwrap();
    app.wait_for("id:task-T-8").unwrap();
    assert_eq!(app.screen().count("id:field-reply").unwrap(), 0);
    assert_text(&mut app, "page-title", "Now");
}

/// The successor of the 80-column Inbox: at 80 × 24 Now keeps what needs you on the page, its
/// heading and its first row in view, rather than hiding the list behind the side panel.
#[test]
fn terminal_80_columns_keeps_what_needs_you_in_view() {
    let mut app = preview(terminal_80(), &[]);
    app.wait_for("id:section-you").unwrap();
    let nodes = app.screen().layout_nodes();
    for id in ["section-you", "task-T-8"] {
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
/// dispatch with force, T-12 runs, and Changes records the forced operation.
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
    app.press("id:nav-agents").unwrap();
    app.wait_for("id:agent-T-12_R1").unwrap();
    app.press("id:nav-changes").unwrap();
    app.wait_for("text:/forced dispatch/").unwrap();
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

/// T-10's gate is red, so its decide bar offers Land anyway: one press opens the one local
/// question, which names what it skips; confirming submits approve with force and the task lands
/// and leaves Review.
#[test]
fn land_anyway_over_a_red_gate_asks_once_and_lands() {
    let mut app = preview(desktop(), &[("corral.destination", "review")]);
    app.press("id:task-T-10").unwrap();
    app.press("id:btn-land-anyway").unwrap();
    app.wait_for("id:confirm").unwrap();
    app.wait_for("text:/the gate check/").unwrap();
    app.press("id:confirm-confirm").unwrap();
    app.wait_for("text:/^Land anyway T-10 · Done/").unwrap();
    app.wait_for_gone("id:task-T-10", Duration::from_secs(5))
        .unwrap();
}

/// Approve lands onto the branch typed in Onto. A branch the repository lacks is refused, and the
/// refusal offers one Approve per existing branch; choosing one asks once and lands there.
#[test]
fn approving_onto_a_missing_branch_offers_each_existing_branch() {
    let mut app = preview(desktop(), &[("corral.destination", "review")]);
    app.press("id:task-T-11").unwrap();
    app.wait_for("id:field-onto").unwrap();
    app.clear("id:field-onto").unwrap();
    app.type_text("id:field-onto", "release/1.0").unwrap();
    app.press("id:btn-approve").unwrap();
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

/// Escape closes the roster's panel first: the page stays Agents rather than navigating back.
#[test]
fn escape_clears_the_roster_selection_before_leaving_agents() {
    let mut app = preview(desktop(), &[("corral.destination", "agents")]);
    app.press("id:agent-T-14_R1").unwrap();
    app.wait_for("id:seg-agent-panel-summary").unwrap();
    app.press_key("escape").unwrap();
    app.wait_for_gone("id:seg-agent-panel-summary", Duration::from_secs(5))
        .unwrap();
    assert!(
        app.screen()
            .accessibility_state("id:nav-agents")
            .unwrap()
            .selected
    );
}

/// A run row's Session menu terminates its session: Terminate asks once, confirming sends it,
/// and the run leaves the roster.
#[test]
fn terminate_from_a_session_row_asks_once_and_ends_the_run() {
    let mut app = preview(desktop(), &[("corral.destination", "agents")]);
    app.wait_for("id:agent-T-14_R1").unwrap();
    app.right_click("id:agent-T-14_R1").unwrap();
    app.press("id:btn-terminate").unwrap();
    app.wait_for("id:confirm").unwrap();
    app.press("id:confirm-confirm").unwrap();
    app.wait_for("text:/Terminated/").unwrap();
    app.wait_for_gone("id:agent-T-14_R1", Duration::from_secs(5))
        .unwrap();
    assert_eq!(app.screen().count("id:confirm").unwrap(), 0);
}

/// Pause on a session without a pi host is refused, and the refusal sheet says corrald's reason.
#[test]
fn pause_on_a_session_without_a_pi_host_shows_the_refusal_reason() {
    let mut app = preview(desktop(), &[("corral.destination", "agents")]);
    app.wait_for("id:agent-T-14_R1").unwrap();
    app.right_click("id:agent-T-14_R1").unwrap();
    app.press("id:btn-pause").unwrap();
    app.wait_for("id:refused").unwrap();
    app.wait_for("text:/no Pi socket/").unwrap();
}

/// Review's live reviewer opens its session page: the reviewer run's terminal.
#[test]
fn the_reviewer_session_opens_from_review() {
    let mut app = preview(desktop(), &[("corral.destination", "review")]);
    app.press("id:task-T-13").unwrap();
    app.press("id:btn-open-reviewer").unwrap();
    app.wait_for("id:session-terminal").unwrap();
    app.wait_for("id:btn-session-menu").unwrap();
    app.wait_for("text:/T-13-R2/").unwrap();
}

/// The grouping switch asks the feed for the other grouping and redraws the lanes from its
/// answer: By run draws one lane per run, which By task does not have, and a live run's lane
/// opens its session terminal.
#[test]
fn timeline_grouping_redraws_the_lanes_from_the_feed() {
    let mut app = preview(terminal_80(), &[("corral.destination", "timeline")]);
    app.wait_for("id:lane-T-14").unwrap();
    assert_eq!(app.screen().count("id:lane-T-14/R1").unwrap(), 0);
    app.press("id:seg-group-1").unwrap();
    app.wait_for("id:lane-T-14/R1").unwrap();
    app.press("id:lane-T-14/R1").unwrap();
    app.wait_for("id:session-terminal").unwrap();
}
