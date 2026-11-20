//! Sessions and the session workbench, headless: the list grouped by state beside its
//! inspector, and the session page with its Details panel, dock, composer and controls.

use std::time::{Duration, Instant};

use nsh_core::{
    PasteEvent, PastedFile, Points, Size,
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

/// A phone browser, whose tabs the shell draws itself.
fn phone() -> EngineConfig {
    let mut config = EngineConfig::new(Root::of::<nsh_app_corral::App>());
    config.environment.os = Os::Web;
    config.environment.idiom = Idiom::Phone;
    config
}

/// Starts on Sessions and chooses the sample preview from the palette.
fn sessions(config: EngineConfig) -> Headless {
    let opener = if config.environment.idiom == Idiom::Phone {
        "id:btn-search"
    } else {
        "id:palette-open"
    };
    let mut config = config;
    config.storage = Some(MemoryStorage::new([(
        "corral.destination".to_owned(),
        "sessions".to_owned(),
    )]));
    let mut app = Headless::start(config).unwrap();
    app.wait_for("id:nav-settings").unwrap();
    app.press(opener).unwrap();
    app.wait_for("id:palette-input").unwrap();
    app.type_text("id:palette-input", "Data: fixture").unwrap();
    app.press("text:Data: fixture").unwrap();
    app.wait_for_gone("id:palette-input", Duration::from_secs(5))
        .unwrap();
    app
}

/// Whether a view with test id `id` is in the tree and not hidden, on screen or scrolled away.
fn in_tree(app: &Headless, id: &str) -> bool {
    app.screen()
        .layout_nodes()
        .iter()
        .any(|node| node.test_id.as_deref() == Some(id) && !node.hidden)
}

/// Waits, as long as `wait_for` does, for [`in_tree`].
fn wait_in_tree(app: &mut Headless, id: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !in_tree(app, id) {
        assert!(Instant::now() < deadline, "id:{id} never joined the tree");
        app.next_change(Duration::from_millis(100)).unwrap();
    }
}

/// Opens session `id`'s page from its row: one press without the inspector, a second with it
/// (the first selects).
fn open(app: &mut Headless, id: &str) {
    let row = format!("id:agent-{id}");
    app.wait_for(&row).unwrap();
    app.press(&row).unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    while !in_tree(app, "page-back") && Instant::now() < deadline {
        app.next_change(Duration::from_millis(100)).unwrap();
    }
    if !in_tree(app, "page-back") {
        app.press(&row).unwrap();
    }
    app.wait_for("id:page-back").unwrap();
}

/// Shows Details where it is closed (a narrow page, the phone), then presses the section's Open.
fn open_section(app: &mut Headless, section: &str) {
    if !in_tree(app, "session-details") {
        app.press("id:btn-details").unwrap();
        wait_in_tree(app, "session-details");
    }
    app.press(&format!("id:btn-details-{section}")).unwrap();
}

/// A stopped host listed among the live ones buried what waits on the owner: Blocked leads,
/// Gone stays folded until its header opens it, and a row selects into the inspector before a
/// second press opens its page.
#[test]
fn sessions_group_by_state_and_gone_folds_until_opened() {
    let mut app = sessions(desktop());
    app.wait_for("id:list-sessions").unwrap();
    app.wait_for("id:section-blocked").unwrap();
    app.wait_for("id:section-gone").unwrap();
    assert!(!in_tree(&app, "agent-B-k2a1"), "a Gone build is folded");
    app.press("id:section-gone").unwrap();
    wait_in_tree(&mut app, "agent-B-k2a1");
    app.wait_for("id:inspector").unwrap();
    app.press("id:agent-T-14_R1").unwrap();
    app.wait_for("id:btn-open-page").unwrap();
    assert!(!in_tree(&app, "page-back"), "the first press only selects");
    app.press("id:agent-T-14_R1").unwrap();
    app.wait_for("id:page-back").unwrap();
}

/// The inspector leads with what a blocked session asks and answers it in place: the sample
/// build's two questions are answered from the inspector, without opening its page.
#[test]
fn the_inspector_answers_a_blocked_session_in_place() {
    let mut app = sessions(desktop());
    // The sample's first Blocked row is a chat, selected on arrival: one press selects the build.
    app.wait_for("id:agent-B-k3x9").unwrap();
    app.press("id:agent-B-k3x9").unwrap();
    app.wait_for("id:inspect-question").unwrap();
    app.wait_for("id:dock-q-b1").unwrap();
    app.press("id:dock-q-b1").unwrap();
    app.press("id:opt-q-b1-0").unwrap();
    app.wait_for("id:question-1").unwrap();
    app.press("id:opt-q-b1-1").unwrap();
    app.press("id:btn-send-answers").unwrap();
    app.wait_for_gone("id:btn-send-answers", Duration::from_secs(5))
        .unwrap();
    assert!(
        !in_tree(&app, "page-back"),
        "answered without leaving the list"
    );
}

/// Details sits beside the conversation on a wide page and is open there; at 80 columns it is
/// closed, Alt+B opens it in the conversation's place, and a view opened from it takes that
/// place again.
#[test]
fn details_open_beside_a_wide_page_and_by_key_at_80_columns() {
    let mut app = sessions(desktop());
    open(&mut app, "B-k3x9");
    wait_in_tree(&mut app, "session-details");
    app.wait_for("id:session-transcript").unwrap();
    drop(app);

    // The sample build docks three choices, a question and its queue, which leave its
    // conversation no room in this small grid: it is looked for in the tree.
    let mut app = sessions(terminal_80());
    open(&mut app, "B-k3x9");
    wait_in_tree(&mut app, "session-transcript");
    assert!(!in_tree(&app, "session-details"), "folded at 80 columns");
    app.press_key("alt+b").unwrap();
    wait_in_tree(&mut app, "session-details");
    app.press("id:btn-details-changes").unwrap();
    wait_in_tree(&mut app, "session-changes");
    app.wait_for_gone("id:session-details", Duration::from_secs(5))
        .unwrap();
    assert!(
        !in_tree(&app, "session-details"),
        "the view took Details' place"
    );
}

/// Escape steps back before it leaves: on a session page it returns a view (Agents) to the
/// conversation, and the page and the Sessions place stay.
#[test]
fn escape_returns_a_session_view_to_the_conversation_before_leaving_the_page() {
    let mut app = sessions(desktop());
    open(&mut app, "B-k3x9");
    open_section(&mut app, "agents");
    app.wait_for("id:session-agents").unwrap();
    app.press_key("escape").unwrap();
    app.wait_for_gone("id:session-agents", Duration::from_secs(5))
        .unwrap();
    app.wait_for("id:session-transcript").unwrap();
    app.wait_for("id:page-back").unwrap();
}

/// A run row's More menu terminates its session: Terminate asks once, confirming sends it, and
/// the run leaves the live groups.
#[test]
fn terminate_from_a_session_row_asks_once_and_ends_the_run() {
    let mut app = sessions(desktop());
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

/// A menu the pointer opened lost the arrows to the list under it, which moved its selection:
/// they go into the menu (End to its last item, Task ›, then Up past Reply…), and Enter runs the
/// item reached, Terminate.
#[test]
fn arrow_keys_walk_a_menu_the_pointer_opened() {
    let mut app = sessions(desktop());
    app.wait_for("id:agent-T-14_R1").unwrap();
    app.right_click("id:agent-T-14_R1").unwrap();
    app.wait_for("id:btn-terminate").unwrap();
    for key in ["end", "up", "up"] {
        app.press_key(key).unwrap();
    }
    app.press_key("enter").unwrap();
    app.wait_for("id:confirm").unwrap();
    app.wait_for("text:/Terminate/").unwrap();
}

/// Pause on a session without a pi host is refused, and the refusal sheet says corrald's reason.
#[test]
fn pause_on_a_session_without_a_pi_host_shows_the_refusal_reason() {
    let mut app = sessions(desktop());
    app.wait_for("id:agent-T-15_R1").unwrap();
    app.right_click("id:agent-T-15_R1").unwrap();
    app.press("id:btn-pause").unwrap();
    app.wait_for("id:refused").unwrap();
    app.wait_for("text:/no Pi socket/").unwrap();
}

/// In a terminal, a control chosen in the session page's ⋯ menu changes the run's state under
/// the menu, and the menu's trigger must still be there and open the menu again; then Ctrl+K
/// opens the palette and Escape closes it with the sample preview still shown.
#[test]
fn the_keyboard_stays_with_the_app_after_a_session_menu_control_in_a_terminal() {
    let mut app = sessions(terminal_80());
    open(&mut app, "T-14_R1");
    app.press("id:btn-session-actions").unwrap();
    app.wait_for("id:act-cancel-turn").unwrap();
    app.press("id:act-cancel-turn").unwrap();
    app.wait_for_gone("id:act-cancel-turn", Duration::from_secs(5))
        .unwrap();
    app.press("id:btn-session-actions").unwrap();
    app.wait_for("id:act-terminate").unwrap();
    app.press_key("escape").unwrap();
    app.wait_for_gone("id:act-terminate", Duration::from_secs(5))
        .unwrap();
    app.press_key("ctrl+k").unwrap();
    app.wait_for("id:palette-input").unwrap();
    app.press_key("escape").unwrap();
    app.wait_for_gone("id:palette-input", Duration::from_secs(5))
        .unwrap();
    app.wait_for("id:btn-session-actions").unwrap();
    assert_eq!(app.screen().count("text:/not reachable/").unwrap(), 0);
}

/// A rewind leaves a gap in the live stream, so the session page reads the transcript again:
/// after rewinding T-14's worker to turn 3, the words of turn 3 are no longer on the page.
#[test]
fn rewinding_a_session_reloads_its_transcript() {
    let mut app = sessions(desktop());
    open(&mut app, "T-14_R1");
    app.wait_for("text:/A run that crossed midnight/").unwrap();
    app.press("id:btn-session-actions").unwrap();
    app.press("id:act-rewind").unwrap();
    app.press("id:btn-rewind-turn-3").unwrap();
    app.wait_for("id:confirm").unwrap();
    app.press("id:confirm-confirm").unwrap();
    app.wait_for_gone("text:/A run that crossed midnight/", Duration::from_secs(5))
        .unwrap();
}

/// Sessions › New › New build: the sheet starts the build in the chosen project under the typed
/// name, closes, and the new build's session page opens.
#[test]
fn starting_a_build_opens_its_session_page() {
    let mut app = sessions(desktop());
    app.press("id:btn-new").unwrap();
    app.press("id:btn-new-build").unwrap();
    app.wait_for("id:field-build-name").unwrap();
    app.press("id:seg-build-project-corral").unwrap();
    app.type_text("id:field-build-name", "walk").unwrap();
    app.press("id:btn-start-build").unwrap();
    app.wait_for_gone("id:field-build-name", Duration::from_secs(5))
        .unwrap();
    app.wait_for("id:btn-session-actions").unwrap();
    app.wait_for("text:walk").unwrap();
}

/// The sample build waits on a two-question card: its answer card takes one answer per
/// question and sends them once both are answered. Two follow-ups queued while it works join
/// the two the sample queued; they are reordered and one is taken back into the draft (the
/// queue folds while cards wait, so it is unfolded first). `/fin` runs Finish…, which docks the
/// exit choice; Land asks once on this device, then for the reason, and the exit walks rebase,
/// gate and land to its outcome.
#[test]
fn a_build_session_is_answered_and_landed() {
    let mut app = sessions(desktop());
    open(&mut app, "B-k3x9");

    app.wait_for("id:dock-q-b1").unwrap();
    app.press("id:dock-q-b1").unwrap();
    app.wait_for("id:opt-q-b1-0").unwrap();
    app.press("id:opt-q-b1-0").unwrap();
    app.wait_for("id:question-1").unwrap();
    app.press("id:opt-q-b1-1").unwrap();
    app.wait_for("id:review-1").unwrap();
    app.press("id:btn-send-answers").unwrap();
    app.wait_for_gone("id:btn-send-answers", Duration::from_secs(5))
        .unwrap();

    app.wait_for("id:btn-queue").unwrap();
    for text in ["first follow-up", "second follow-up"] {
        app.clear("id:field-message").unwrap();
        app.type_text("id:field-message", text).unwrap();
        app.press("id:btn-queue").unwrap();
    }
    if !in_tree(&app, "queue-0") {
        app.press("id:btn-queue-toggle").unwrap();
        wait_in_tree(&mut app, "queue-0");
    }
    wait_in_tree(&mut app, "queue-3");
    app.press("id:btn-queue-down-2").unwrap();
    app.wait_for("id:btn-queue-edit-2").unwrap();
    app.press("id:btn-queue-edit-2").unwrap();
    app.wait_for_gone("id:queue-3", Duration::from_secs(5))
        .unwrap();
    assert_eq!(
        app.screen().input_text("id:field-message").unwrap(),
        Some("second follow-up")
    );

    app.clear("id:field-message").unwrap();
    app.type_text("id:field-message", "/fin").unwrap();
    app.wait_for("id:cmd-finish").unwrap();
    app.submit("id:field-message").unwrap();
    app.wait_for("id:opt-exit-B-k3x9-1").unwrap();
    app.press("id:opt-exit-B-k3x9-1").unwrap();
    app.wait_for("id:btn-card-confirm").unwrap();
    app.press("id:btn-card-confirm").unwrap();
    app.wait_for("id:field-message").unwrap();
    app.type_text("id:field-message", "The open finding is accepted for now.")
        .unwrap();
    app.submit("id:field-message").unwrap();
    app.wait_for_within("text:/^✓ Landed/", Duration::from_secs(30))
        .unwrap();
}

/// A build page whose host declares attachments offers them and hears an image pasted into its
/// field: the sample host takes none now, so the paste is refused visibly, not dropped.
#[test]
fn an_image_pasted_into_a_build_page_reaches_its_attachments() {
    let mut app = sessions(desktop());
    open(&mut app, "B-k3x9");
    app.wait_for("id:field-message").unwrap();
    app.wait_for("id:btn-attach-0").unwrap();
    let png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0];
    let mut paste = PasteEvent::default();
    paste.files.push(PastedFile::image(png));
    app.paste("id:field-message", &paste).unwrap();
    app.wait_for("text:/^Not attached/").unwrap();
}

/// A stopped build's row offers Remove…; its sheet shows prune's dry run before anything goes,
/// and confirming removes the build from the list.
#[test]
fn a_stopped_build_is_removed_after_its_dry_run() {
    let mut app = sessions(desktop());
    app.press("id:section-gone").unwrap();
    app.wait_for("id:btn-remove-B-k2a1").unwrap();
    app.press("id:btn-remove-B-k2a1").unwrap();
    app.wait_for("id:remove-lines").unwrap();
    app.wait_for("text:/would remove worktree/").unwrap();
    app.press("id:btn-remove-confirm").unwrap();
    app.wait_for_gone("id:agent-B-k2a1", Duration::from_secs(5))
        .unwrap();
}

/// The sample build serves every part a host serves, so each Details section opens its view on
/// what the sample holds on every form; a part the preview did not answer would leave it
/// reading forever. Agents lists the three children and both jobs, a child opens on its detail
/// and its conversation, a job on its output; Changes lists the three files and the first opens
/// on its two hunks; Board lists its tasks and proposal and opens the build record's sections;
/// the ⋯ menu lists the host's actions and opens Usage on one row per agent and the total;
/// Details' Plan lists the plan's steps. List rows are looked for in the tree, not on screen.
/// What the host serves does not depend on the form; the 80-column grid opens a view from
/// Details in `details_open_beside_a_wide_page_and_by_key_at_80_columns`.
#[test]
fn every_details_section_opens_in_the_sample_preview_on_phone_and_desktop() {
    for config in [phone(), desktop()] {
        let mut app = sessions(config);
        open(&mut app, "B-k3x9");
        wait_in_tree(&mut app, "session-transcript");
        wait_in_tree(&mut app, "item-0");

        open_section(&mut app, "agents");
        wait_in_tree(&mut app, "session-agents");
        for child in ["migration", "docs", "review"] {
            wait_in_tree(&mut app, &format!("agent-B-k3x9-child-{child}"));
        }
        for job in ["j1-1", "j1-2"] {
            wait_in_tree(&mut app, &format!("job-{job}"));
        }
        app.press("id:agent-B-k3x9-child-migration").unwrap();
        wait_in_tree(&mut app, "session-agent-detail");
        app.press("id:btn-child-open").unwrap();
        wait_in_tree(&mut app, "session-conversation");
        wait_in_tree(&mut app, "item-0");
        app.press("id:btn-agents-back").unwrap();
        app.wait_for_gone("id:session-conversation", Duration::from_secs(5))
            .unwrap();
        wait_in_tree(&mut app, "session-agents");
        app.press("id:job-j1-1").unwrap();
        wait_in_tree(&mut app, "session-job-detail");
        app.press("id:btn-job-open").unwrap();
        wait_in_tree(&mut app, "session-job-output");
        wait_in_tree(&mut app, "btn-job-older");
        app.press("id:btn-agents-back").unwrap();
        app.wait_for_gone("id:session-job-output", Duration::from_secs(5))
            .unwrap();
        wait_in_tree(&mut app, "session-agents");

        open_section(&mut app, "changes");
        wait_in_tree(&mut app, "session-changes");
        for file in 0..3 {
            wait_in_tree(&mut app, &format!("change-file-{file}"));
        }
        app.press("id:change-file-0").unwrap();
        wait_in_tree(&mut app, "changes-file");
        wait_in_tree(&mut app, "hunk-1");

        open_section(&mut app, "board");
        wait_in_tree(&mut app, "session-board");
        wait_in_tree(&mut app, "task-T-004");
        wait_in_tree(&mut app, "board-plan");
        app.press("id:btn-board-record").unwrap();
        wait_in_tree(&mut app, "session-record");
        for section in ["gate", "reviews", "decisions", "timeline"] {
            wait_in_tree(&mut app, &format!("record-{section}"));
        }

        app.press("id:btn-session-actions").unwrap();
        wait_in_tree(&mut app, "session-actions");
        for row in [
            "model",
            "compact",
            "export",
            "board-refresh",
            "build-record",
            "plan-ledger",
        ] {
            wait_in_tree(&mut app, &format!("act-{row}"));
        }
        app.press("id:act-view-usage").unwrap();
        app.wait_for_gone("id:session-actions", Duration::from_secs(5))
            .unwrap();
        wait_in_tree(&mut app, "session-usage");
        for row in ["pane", "migration", "docs", "review"] {
            wait_in_tree(&mut app, &format!("usage-row-{row}"));
        }
        wait_in_tree(&mut app, "usage-total");

        if !in_tree(&app, "session-details") {
            app.press("id:btn-details").unwrap();
        }
        for step in 0..5 {
            wait_in_tree(&mut app, &format!("side-step-{step}"));
        }
    }
}
