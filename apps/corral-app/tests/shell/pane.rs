//! The desktop side pane: Ask's tab and the sessions kept beside the page, what they open in
//! the page, and how a narrow pane steps aside, keeps its drafts and lets go of the keys.

use super::*;

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

/// A full pane still opens the just-started session through Configuration's leave guard.
#[test]
fn full_pane_start_from_dirty_configuration_opens_the_exact_session() {
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
    app.press("id:seg-config-profiles-pi-luna").unwrap();
    let model = "id:field-config-profiles.pi-luna.model";
    app.type_text(model, "-custom").unwrap();
    let drafted = app.screen().input_text(model).unwrap().unwrap().to_owned();
    app.press("id:side-pane-toggle").unwrap();
    for n in 0..7 {
        if n == 6 {
            app.press("id:dock-tab-ask").unwrap();
        }
        app.wait_for("id:pane-new").unwrap();
        app.press("id:pane-new").unwrap();
        app.press("id:pane-new-build").unwrap();
        app.wait_for("id:field-build-name").unwrap();
        app.press("id:seg-build-project-corral").unwrap();
        app.type_text("id:field-build-name", &format!("full-test-{n}"))
            .unwrap();
        app.press("id:btn-start-build").unwrap();
        if n < 6 {
            app.wait_for("id:dock-transcript").unwrap();
        }
    }
    app.wait_for("id:confirm").unwrap();
    assert_eq!(
        app.screen().input_text(model).unwrap(),
        Some(drafted.as_str())
    );
    app.press("id:confirm-confirm").unwrap();
    app.wait_for("text:full-test-6").unwrap();
    app.wait_for("id:btn-session-actions").unwrap();
    assert_eq!(app.screen().count("id:list-sessions").unwrap(), 0);
    app.press("id:nav-sessions").unwrap();
    app.wait_for("id:list-sessions >> text:full-test-6")
        .unwrap();
    assert_eq!(
        app.screen()
            .count("id:list-sessions >> text:full-test-6")
            .unwrap(),
        1
    );
}

/// Both commands start a session on a tab, leaving the page where it was.
#[test]
fn the_panes_plus_starts_a_build_on_a_tab_of_its_own() {
    for command in ["id:pane-new-build", "id:pane-new-code"] {
        let mut app = preview(desktop(), &[("corral.destination", "tasks")]);
        app.wait_for("id:list-tasks").unwrap();
        app.press_key("/").unwrap();
        app.wait_for("id:pane-new").unwrap();
        app.press("id:pane-new").unwrap();
        app.press(command).unwrap();
        app.wait_for("id:field-build-name").unwrap();
        app.press("id:seg-build-project-corral").unwrap();
        app.type_text("id:field-build-name", "walk").unwrap();
        app.press("id:btn-start-build").unwrap();
        app.wait_for("id:dock-transcript").unwrap();
        app.wait_for("text:walk").unwrap();
        app.wait_for("id:list-tasks").unwrap();
    }
}
