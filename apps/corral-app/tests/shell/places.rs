//! The places and their pages: the sidebar and the phone's tabs reach each, Settings and its
//! sections, the stored destination, the disconnected state, the scope and the terminal's room.

use super::*;

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

/// The grouping switch asks the feed for the other grouping and redraws the lanes from its
/// answer: By run draws one lane per run, which By task does not have, and a live run's lane
/// opens its session page: T-15's headless run has no Pi host, so the page is its read-only log;
/// the first press selects the lane.
#[test]
fn timeline_grouping_redraws_the_lanes_from_the_feed() {
    let mut app = preview(terminal_80(), &[("corral.destination", "timeline")]);
    app.wait_for("id:lane-T-15").unwrap();
    assert_eq!(app.screen().count("id:lane-T-15/R1").unwrap(), 0);
    app.press("id:seg-group-1").unwrap();
    app.wait_for("id:lane-T-15/R1").unwrap();
    // The first press selects the lane and shows its inspector; the second opens it.
    app.press("id:lane-T-15/R1").unwrap();
    app.wait_for("id:inspector").unwrap();
    app.press("id:lane-T-15/R1").unwrap();
    app.wait_for("id:session-log").unwrap();
}

/// A redraw that replaced the Timeline's buttons would drop the keyboard focus: the Enter that
/// folds a plan or selects a lane must leave the focus on it, so the next Enter unfolds the
/// plan and opens the lane.
#[test]
fn timeline_keyboard_focus_survives_folding_a_plan_and_selecting_a_lane() {
    let mut config = terminal_80();
    config.viewport = Size::new(Points(1120.0), Points(704.0));
    let mut app = preview(config, &[("corral.destination", "timeline")]);
    app.wait_for("id:lane-T-11").unwrap();
    // A row leads with the focus mark while the keyboard is on it.
    let focused =
        |app: &Headless, id: &str| app.screen().count(&format!("id:{id} >> text:›")).unwrap() > 0;
    // The first plan's header, which holds T-11.
    let mut tabs = 0;
    while !focused(&app, "plan-T-1") {
        assert!(tabs < 40, "Tab reached no plan header");
        app.press_key("tab").unwrap();
        tabs += 1;
    }
    app.press_key("enter").unwrap();
    app.wait_for_gone("id:lane-T-11", Duration::from_secs(5))
        .unwrap();
    assert!(focused(&app, "plan-T-1"), "folding keeps the focus");
    app.press_key("enter").unwrap();
    app.wait_for("id:lane-T-11").unwrap();
    app.press_key("tab").unwrap();
    assert!(focused(&app, "lane-T-11"));
    app.press_key("enter").unwrap();
    app.wait_for("id:inspector").unwrap();
    assert!(
        selected(&app, "lane-T-11"),
        "Enter selects the focused lane"
    );
    assert!(focused(&app, "lane-T-11"), "selecting keeps the focus");
    app.press_key("enter").unwrap();
    app.wait_for("id:task-page").unwrap();
}
