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

/// The sidebar lists the places at rest, and Activity's and Insight's pages only under their
/// place while one of them shows; every one of them, and Settings at the foot, opens a page.
/// System is Insight's four pages, never an Activity page.
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
    for slug in ["feed", "timeline", "usage", "artifacts"] {
        app.wait_for(&format!("id:nav-{slug}")).unwrap();
        app.press(&format!("id:nav-{slug}")).unwrap();
        app.wait_for("id:page-title").unwrap();
        assert!(selected(&app, &format!("nav-{slug}")), "{slug} is current");
        assert!(selected(&app, "nav-activity"));
    }
    assert_eq!(app.screen().count("id:nav-system").unwrap(), 0);
    assert_eq!(app.screen().count("id:nav-structure").unwrap(), 0);
    app.press("id:nav-insight").unwrap();
    for (slug, title) in [
        ("structure", "Structure"),
        ("runtime", "Runtime"),
        ("data", "Data"),
        ("lenses", "Lenses"),
    ] {
        app.wait_for(&format!("id:nav-{slug}")).unwrap();
        app.press(&format!("id:nav-{slug}")).unwrap();
        assert_text(&mut app, "page-title", title);
        assert!(selected(&app, &format!("nav-{slug}")), "{slug} is current");
        assert!(selected(&app, "nav-insight"));
        assert!(!selected(&app, "nav-activity"));
        assert_eq!(app.screen().count("id:nav-usage").unwrap(), 0);
    }
    app.press("id:nav-settings").unwrap();
    app.wait_for("id:settings-devices").unwrap();
    assert_eq!(app.screen().count("id:nav-usage").unwrap(), 0);
}

/// The phone's tabs are the places; Activity's and Insight's roots list their pages, each a
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
    for slug in ["feed", "timeline", "usage", "artifacts"] {
        app.wait_for(&format!("id:nav-{slug}")).unwrap();
        app.press(&format!("id:nav-{slug}")).unwrap();
        app.wait_for_gone("id:nav-feed", Duration::from_secs(5))
            .unwrap();
        app.back().unwrap();
        app.wait_for("id:nav-feed").unwrap();
    }
    assert_eq!(app.screen().count("id:nav-system").unwrap(), 0);

    app.press("id:tab-insight").unwrap();
    assert_text(&mut app, "page-title", "Insight");
    for (slug, title) in [
        ("structure", "Structure"),
        ("runtime", "Runtime"),
        ("data", "Data"),
        ("lenses", "Lenses"),
    ] {
        app.wait_for(&format!("id:nav-{slug}")).unwrap();
        app.press(&format!("id:nav-{slug}")).unwrap();
        app.wait_for_gone("id:nav-structure", Duration::from_secs(5))
            .unwrap();
        app.wait_for(&format!("text:/^{title}$/")).unwrap();
        app.back().unwrap();
        app.wait_for("id:nav-structure").unwrap();
    }
}

/// The phone's Insight list had no address, so the entry under Runtime kept Settings': the
/// browser's Back opened Settings with a new entry and Forward could not bring Runtime back.
#[test]
fn phone_browser_back_and_forward_walk_the_insight_list_and_its_page() {
    let mut config = phone();
    config.storage = Some(MemoryStorage::new(std::iter::empty()));
    let mut app = Headless::start_at_page(config, "https://corral.test/").unwrap();
    app.wait_for("id:nav-settings").unwrap();
    app.press("id:btn-search").unwrap();
    app.type_text("id:palette-input", "Data: fixture").unwrap();
    app.press("text:Data: fixture").unwrap();
    app.wait_for_gone("id:palette-input", Duration::from_secs(5))
        .unwrap();
    app.press("id:nav-settings").unwrap();
    app.wait_for("id:settings-devices").unwrap();

    let url = |app: &mut Headless| app.browser().url().map(str::to_owned);
    app.press("id:tab-insight").unwrap();
    app.wait_for("id:insight-list").unwrap();
    assert_eq!(
        url(&mut app).as_deref(),
        Some("https://corral.test/insight")
    );
    app.press("id:nav-runtime").unwrap();
    app.wait_for_gone("id:nav-structure", Duration::from_secs(5))
        .unwrap();
    assert_eq!(
        url(&mut app).as_deref(),
        Some("https://corral.test/insight/runtime")
    );
    let entries = app.browser().entries().len();

    app.browser().back().unwrap();
    app.wait_for("id:nav-structure").unwrap();
    assert_eq!(
        url(&mut app).as_deref(),
        Some("https://corral.test/insight")
    );
    assert_eq!(app.screen().count("id:settings-devices").unwrap(), 0);
    assert_eq!(app.browser().entries().len(), entries);

    app.browser().forward().unwrap();
    app.wait_for_gone("id:nav-structure", Duration::from_secs(5))
        .unwrap();
    app.wait_for("text:/^Runtime$/").unwrap();
    assert_eq!(
        url(&mut app).as_deref(),
        Some("https://corral.test/insight/runtime")
    );
    assert_eq!(app.browser().entries().len(), entries);
}

/// The phone has no canvas map: with a project chosen, System lists the same nodes, and the
/// picked one's edges under it, which pick the other end.
#[test]
fn phone_system_lists_the_map_and_the_picked_nodes_edges() {
    let mut app = preview(phone(), &[]);
    app.press("id:tab-insight").unwrap();
    app.press("id:nav-structure").unwrap();
    app.press("id:system-pick-corral").unwrap();
    app.wait_for("id:system-tree").unwrap();
    app.press("id:node-c-crates-corral-facts").unwrap();
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

/// The folded terminal sidebar showed Insight only as its key `m`: its four pages, and which
/// one shows, were not in it. They follow the key while Insight shows, the current one marked.
#[test]
fn the_folded_terminal_sidebar_lists_insights_pages_and_marks_the_current_one() {
    let mut app = preview(terminal_80(), &[]);
    app.wait_for("id:nav-insight").unwrap();
    // Folded: the place is its key, without its name.
    assert_eq!(app.screen().count("text:/^Insight$/").unwrap(), 0);
    assert_eq!(app.screen().count("id:nav-runtime").unwrap(), 0);
    app.press("id:nav-insight").unwrap();
    app.wait_for("id:nav-runtime").unwrap();
    assert!(selected(&app, "nav-structure"));
    app.press("id:nav-runtime").unwrap();
    app.wait_for("text:/^Runtime$/").unwrap();
    assert!(selected(&app, "nav-runtime"));
    assert!(!selected(&app, "nav-structure"));
    assert!(selected(&app, "nav-insight"));
    for page in ["data", "lenses"] {
        app.wait_for(&format!("id:nav-{page}")).unwrap();
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
    // The sample follows the real clock, so which of the plan's lanes precede T-11 varies.
    let mut tabs = 0;
    while !focused(&app, "lane-T-11") {
        assert!(tabs < 10, "Tab from the plan header reached no lane-T-11");
        app.press_key("tab").unwrap();
        tabs += 1;
    }
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

/// Configuration's ten tabs are wider than a phone: swiping the strip brings Skills, the last,
/// into view, so a finger can reach it.
#[test]
fn phone_config_tabs_scroll_to_skills() {
    let config = phone();
    let width = config.viewport.width.0;
    let mut app = preview(config, &[]);
    app.press("id:nav-settings").unwrap();
    app.press("id:settings-config").unwrap();
    app.wait_for("id:tab-config-9").unwrap();
    let right = |app: &mut Headless| {
        let nodes = app.screen().layout_nodes();
        let tab = nodes
            .iter()
            .find(|node| node.test_id.as_deref() == Some("tab-config-9"))
            .expect("the Skills tab");
        tab.bounds.origin.x.0 + tab.bounds.size.width.0
    };
    assert!(right(&mut app) > width, "the strip outgrows the phone");
    app.scroll("id:tab-config-0", Points(2000.0)).unwrap();
    assert!(right(&mut app) <= width, "Skills scrolls into view");
}

#[test]
fn skills_fixture_keeps_markdown_drafts_and_switches_read_only_builtins() {
    let mut app = preview(phone(), &[]);
    app.press("id:nav-settings").unwrap();
    app.press("id:settings-config").unwrap();
    app.press("id:tab-config-9").unwrap();
    app.press("id:btn-pi-skill-0").unwrap();
    app.wait_for("id:field-pi-skill-markdown").unwrap();
    assert_eq!(app.screen().count("id:btn-pi-skill-save").unwrap(), 0);
    app.press("id:btn-pi-skill-toggle").unwrap();
    app.wait_for("text:Enable in workspace").unwrap();
    app.press("id:btn-pi-skill-back").unwrap();

    app.press("id:btn-pi-skill-9").unwrap();
    assert_text(
        &mut app,
        "pi-skill-path",
        "/preview/configured/skills/notes/SKILL.md",
    );
    let field = "id:field-pi-skill-markdown";
    app.type_text(field, "Draft note").unwrap();
    let draft = app.screen().input_text(field).unwrap().unwrap().to_owned();
    assert!(draft.contains("Draft note"));
    app.press("id:tab-config-0").unwrap();
    app.press("id:tab-config-9").unwrap();
    app.wait_for(field).unwrap();
    assert_eq!(
        app.screen().input_text(field).unwrap(),
        Some(draft.as_str())
    );
    app.press("id:btn-pi-skill-save").unwrap();
    app.press("id:btn-pi-skill-back").unwrap();
    app.press("id:btn-pi-skill-9").unwrap();
    app.wait_for(field).unwrap();
    assert_eq!(
        app.screen().input_text(field).unwrap(),
        Some(draft.as_str())
    );
}
