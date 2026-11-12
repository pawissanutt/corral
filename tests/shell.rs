use std::time::Duration;

use nsh_core::{
    Points, Size,
    platform::{Idiom, Os},
};
use nsh_engine::{EngineConfig, Root};
use nsh_testing::{Headless, MemoryStorage};

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

#[test]
fn phone_opens_on_inbox_and_desktop_on_now() {
    let mut app = with_storage(phone(), &[]);
    assert_text(&mut app, "page-title", "Inbox");
    drop(app);

    let mut app = with_storage(desktop(), &[]);
    app.wait_for("text:Who acts next").unwrap();
}

#[test]
fn desktop_sidebar_reaches_every_destination() {
    let mut app = with_storage(desktop(), &[]);
    for (slug, title) in [
        ("inbox", "Inbox"),
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
            app.wait_for("text:Who acts next").unwrap();
        } else {
            assert_text(&mut app, "page-title", title);
        }
    }
}

#[test]
fn phone_tabs_and_more_reach_every_destination() {
    let mut app = with_storage(phone(), &[]);
    for (slug, title) in [
        ("inbox", "Inbox"),
        ("now", "Now"),
        ("agents", "Agents"),
        ("plans", "Plans"),
    ] {
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
    let mut app = with_storage(phone(), &[]);
    app.press("id:tab-more").unwrap();
    app.press("id:nav-settings").unwrap();
    assert_text(&mut app, "page-title", "Settings");
    app.wait_for("id:settings-devices").unwrap();

    for (slug, title) in [
        ("assistant", "Assistant"),
        ("autopilot", "Autopilot & grants"),
    ] {
        app.press(&format!("id:settings-{slug}")).unwrap();
        assert_text(&mut app, "page-title", title);
        app.back().unwrap();
        assert_text(&mut app, "page-title", "Settings");
        app.wait_for("id:settings-devices").unwrap();
    }
}

#[test]
fn typing_in_ask_does_not_navigate() {
    let mut app = with_storage(desktop(), &[]);
    app.press("id:nav-inbox").unwrap();
    assert_text(&mut app, "page-title", "Inbox");
    app.press("id:ask-open").unwrap();
    app.wait_for("id:ask-input").unwrap();

    let draft = "n i p a r t u . , p";
    app.type_text("id:ask-input", draft).unwrap();

    assert_text(&mut app, "page-title", "Inbox");
    assert_eq!(
        app.screen().input_text("id:ask-input").unwrap(),
        Some(draft)
    );
}

#[test]
fn ask_opens_with_the_field_focused() {
    let mut app = with_storage(desktop(), &[]);
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

    let mut app = with_storage(desktop(), &[]);
    app.press("id:palette-open").unwrap();
    app.wait_for("id:palette-input").unwrap();
    let draft = "Why do violet badgers collect moonstones?";
    app.type_text("id:palette-input", draft).unwrap();
    let row = format!("text:Ask: {draft}");
    app.wait_for(&row).unwrap();
    app.press(&row).unwrap();
    app.wait_for("id:ask-input").unwrap();
    app.wait_for_gone("id:palette-input", Duration::from_secs(5))
        .unwrap();
    for key in ["space", "m", "o", "r", "e"] {
        app.press_key(key).unwrap();
    }
    let expected = format!("{draft} more");
    assert_eq!(
        app.screen().input_text("id:ask-input").unwrap(),
        Some(expected.as_str())
    );
    drop(app);

    let mut app = with_storage(phone(), &[]);
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
    let mut app = with_storage(desktop(), &[]);
    app.press("id:palette-open").unwrap();
    app.wait_for("id:palette-input").unwrap();
    let draft = "Why do violet badgers collect moonstones?";
    app.type_text("id:palette-input", draft).unwrap();
    let row = format!("text:Ask: {draft}");
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
        let mut app = with_storage(config, &[("corral.data", "disconnected")]);
        app.wait_for("text:corrald not connected").unwrap();
        app.wait_for("text:No connection to corrald.").unwrap();
        let texts = app.screen().texts();
        assert!(texts.contains(&"corrald not connected"));
        for healthy in ["corrald healthy", "herdr-core ok", "PM live"] {
            assert!(!texts.contains(&healthy), "disconnected shows {healthy}");
        }
        assert_eq!(app.screen().count("id:count-inbox").unwrap(), 0);
        assert!(texts.contains(&"No connection to corrald."));
    }
}

#[test]
fn send_is_disabled_with_its_reason() {
    let mut app = with_storage(desktop(), &[]);
    app.press("id:ask-open").unwrap();
    app.wait_for("id:ask-send").unwrap();
    assert!(!app.screen().enabled("id:ask-send").unwrap());
    assert_text(
        &mut app,
        "ask-reason",
        "Not connected to corrald. Nothing can be sent from this build.",
    );
}

#[test]
fn destination_is_restored_from_storage() {
    for (config, home) in [(desktop(), "Now"), (phone(), "Inbox")] {
        let mut app = with_storage(config.clone(), &[("corral.destination", "plans")]);
        assert_text(&mut app, "page-title", "Plans");
        drop(app);

        let mut app = with_storage(config, &[("corral.destination", "not-a-destination")]);
        if home == "Now" {
            app.wait_for("text:Who acts next").unwrap();
        } else {
            assert_text(&mut app, "page-title", home);
        }
    }
}
