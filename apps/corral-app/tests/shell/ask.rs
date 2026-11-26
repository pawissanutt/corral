//! Ask: opening it with the keys in its field, its draft, its thread and its attachments, and
//! where it stands beside a desktop page.

use nsh_core::PasteEvent;

use super::*;

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
