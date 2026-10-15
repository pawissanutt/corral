use std::path::{Path, PathBuf};

use corral_core::{HerdrRef, Store};
use corral_ops::Ctx;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, MouseButton, MouseEventKind};

use super::App;
use super::mouse_tests::{find, find_any, footer_row, mouse};
use crate::board::BoardView;
use crate::board::tests::{Fixture, buffer_text, code, key, seeded};
use crate::guide::{LeaveView, TourView, tour_seen};
use crate::nav::NavKeys;
use crate::{Action, HitMap, Mode, View};

fn render_at(app: &mut App, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| app.draw(f)).unwrap();
    buffer_text(term.backend().buffer())
}

fn render(app: &mut App) -> String {
    render_at(app, 120, 32)
}

/// A full left click (press and release), then a redraw so the next lookup sees the new screen.
fn click(app: &mut App, (x, y): (u16, u16)) -> String {
    app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y));
    app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), x, y));
    render(app)
}

fn click_text(app: &mut App, needle: &str) -> String {
    let text = render(app);
    click(app, find_any(&text, needle))
}

/// The footer wraps over the last rows; the needle is searched there, bottom up.
fn footer_click(app: &mut App, needle: &str) -> String {
    let text = render(app);
    let last = footer_row(&text);
    let row = (last.saturating_sub(4)..=last)
        .rev()
        .find(|r| text.lines().nth(*r).is_some_and(|l| l.contains(needle)))
        .unwrap_or_else(|| panic!("{needle:?} not in the footer\n{text}"));
    click(app, find(&text, row, needle))
}

/// The footer rows (it wraps at narrow widths), joined.
fn footer(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(5)..].join("\n")
}

/// A board app whose board and app both use `nav`, like `run` with a loaded herdr config.
fn app_with(fx: &Fixture, nav: &NavKeys) -> App {
    let mut app = App::with_board(fx.ctx.clone(), Mode::Board, |_| {
        Ok(fx.board().with_nav(nav.clone()))
    })
    .unwrap();
    app.nav = nav.clone();
    app
}

fn custom_nav() -> NavKeys {
    NavKeys::parse(
        "[keys]\nprefix = \"ctrl+a\"\ndetach = \"prefix+d\"\n\n\
         [[keys.command]]\nkey = \"ctrl+alt+b\"\ntype = \"plugin_action\"\ncommand = \"corral.board\"\n",
    )
}

#[test]
fn tour_shows_on_first_open_only_and_reopens_from_help() {
    // Given a workspace that never saw the tour.
    let fx = seeded();
    let mut app = fx.app(Mode::Board);
    assert!(!tour_seen(&fx.ctx));
    // When the board starts, the tour opens over it.
    app.start_tour_if_new();
    let text = render(&mut app);
    assert!(text.contains("welcome to corral · 1/6"), "{text}");
    assert!(text.contains("This is your board"), "{text}");
    // When it is skipped, it closes and is remembered in .corral/state.
    app.handle_key(code(KeyCode::Esc));
    assert_eq!(app.stack.len(), 1);
    assert!(tour_seen(&fx.ctx));
    assert!(fx.ctx.store.state_dir().join("tour-seen").is_file());
    // Then the next board open does not show it again.
    let mut again = fx.app(Mode::Board);
    again.start_tour_if_new();
    assert_eq!(again.stack.len(), 1, "tour shows once");
    assert!(!render(&mut again).contains("welcome to corral"));
    // And `?` then `t` replays it.
    again.handle_key(key('?'));
    let text = render(&mut again);
    assert!(text.contains("replay the welcome tour"), "{text}");
    again.handle_key(key('t'));
    let text = render(&mut again);
    assert!(text.contains("welcome to corral · 1/6"), "{text}");
    assert!(
        !text.contains("help · getting around"),
        "help closed\n{text}"
    );
    // And `t` on the board replays it too.
    again.handle_key(code(KeyCode::Esc));
    again.handle_key(key('t'));
    assert!(render(&mut again).contains("welcome to corral · 1/6"));
}

#[test]
fn tour_keyboard_only_walks_every_card_with_the_configured_keys() {
    let fx = seeded();
    let mut app = app_with(&fx, &custom_nav());
    app.start_tour_if_new();
    let mut seen = Vec::new();
    for step in 0..TourView::STEPS {
        let text = render(&mut app);
        assert!(
            text.contains(&format!("welcome to corral · {}/6", step + 1)),
            "{text}"
        );
        seen.push(text);
        app.handle_key(code(KeyCode::Right));
    }
    let all = seen.join("\n");
    for heading in [
        "This is your board",
        "Open an agent",
        "Get back here",
        "Needs me",
        "Review and approve",
        "Leaving and coming back",
    ] {
        assert!(all.contains(heading), "missing {heading}\n{all}");
    }
    assert!(all.contains("press Ctrl-Alt-b to come back"), "{all}");
    assert!(all.contains("press Ctrl-a, then d"), "{all}");
    assert!(!all.contains("herdr prefix"), "{all}");
    // The last Right finished the tour.
    assert_eq!(app.stack.len(), 1);
    assert!(tour_seen(&fx.ctx));
    // ← goes back a card.
    app.apply(Action::OpenTour);
    app.handle_key(code(KeyCode::Right));
    app.handle_key(code(KeyCode::Left));
    assert!(render(&mut app).contains("welcome to corral · 1/6"));
}

#[test]
fn tour_mouse_only_next_done_skip_and_reopen_from_help() {
    let fx = seeded();
    let mut app = fx.app(Mode::Board);
    app.start_tour_if_new();
    for _ in 1..TourView::STEPS {
        click_text(&mut app, "[Next ›]");
    }
    let text = render(&mut app);
    assert!(text.contains("welcome to corral · 6/6"), "{text}");
    click_text(&mut app, "[‹ Back]");
    click_text(&mut app, "[Next ›]");
    click_text(&mut app, "[Done]");
    assert_eq!(app.stack.len(), 1, "Done closes the tour");
    assert!(tour_seen(&fx.ctx));

    // Reopen with the mouse: footer `? help`, then the `t` key in the help's row.
    let text = footer_click(&mut app, "? help");
    let (x, y) = find_any(&text, "replay the welcome tour");
    let text = click(&mut app, (x - 18, y));
    assert!(text.contains("welcome to corral · 1/6"), "{text}");
    click_text(&mut app, "[Skip tour]");
    assert_eq!(app.stack.len(), 1, "Skip closes the tour");
}

#[test]
fn footer_and_help_name_the_configured_keys_in_plain_words() {
    let fx = seeded();
    // 55 columns is the board pane in an 80-column terminal next to herdr's sidebar.
    for (width, back) in [
        (55, "Ctrl-Alt-b back to board"),
        (80, "Ctrl-Alt-b back to board"),
        (120, "Ctrl-Alt-b back to board"),
    ] {
        let mut app = app_with(&fx, &custom_nav());
        let text = render_at(&mut app, width, 30);
        let footer = footer(&text);
        assert!(footer.contains(back), "{width} cols: {footer}");
        assert!(
            footer.contains("? help") && footer.contains("q leave"),
            "{width} cols: {footer}"
        );
        app.handle_key(key('?'));
        let text = render_at(&mut app, width, 30);
        for want in [
            "Ctrl-Alt-b",
            "back to this board from any pane",
            "Ctrl-a d",
            "leave; `corral up` comes back",
            "open the agent's pane",
            "close board; agents keep running",
        ] {
            assert!(text.contains(want), "{width} cols: missing {want}\n{text}");
        }
        assert!(!text.contains("prefix"), "{text}");
    }
    // Without corral's keys in the herdr config, the footer points to `corral up`.
    let mut app = app_with(&fx, &NavKeys::parse(""));
    let text = render(&mut app);
    let footer = footer(&text);
    assert!(footer.contains("corral up reopens the board"), "{footer}");
}

#[test]
fn q_explains_that_agents_keep_running_before_leaving() {
    let fx = seeded();
    let mut app = fx.app(Mode::Board);
    app.handle_key(key('q'));
    assert!(!app.quit);
    let text = render(&mut app);
    for want in [
        "close the board?",
        "2 agents keep running in the background.",
        "Come back with `corral up`, or press Ctrl-b, then k",
        "To leave the whole session, press Ctrl-b, then q.",
    ] {
        assert!(text.contains(want), "missing {want}\n{text}");
    }
    app.handle_key(code(KeyCode::Esc));
    assert!(!app.quit && app.stack.len() == 1, "esc stays");
    app.handle_key(key('q'));
    app.handle_key(key('q'));
    assert!(app.quit, "q q leaves");
}

fn empty_ctx() -> (Ctx, PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "corral-tui-empty-{}-{}",
        std::process::id(),
        jiff::Timestamp::now().as_nanosecond()
    ));
    std::fs::create_dir_all(&root).unwrap();
    (
        corral_ops::resources::FixedResourceProbe::ctx(Store::init(&root).unwrap()),
        root,
    )
}

#[test]
fn empty_board_says_what_to_do_by_key_and_by_mouse() {
    let (ctx, root) = empty_ctx();
    let mut app = App::with_board(ctx.clone(), Mode::Board, BoardView::new).unwrap();
    for width in [80, 120] {
        let text = render_at(&mut app, width, 30);
        assert!(text.contains("No tasks yet."), "{text}");
        assert!(text.contains("`corral new \"title\"`"), "{text}");
        assert!(
            text.contains("[New task]") && text.contains("[Help]"),
            "{text}"
        );
    }
    // Keyboard: n opens capture.
    app.handle_key(key('n'));
    assert!(render(&mut app).contains("what's the idea?"));
    app.handle_key(code(KeyCode::Esc));
    // Mouse: the buttons press n and ?.
    let text = click_text(&mut app, "[New task]");
    assert!(text.contains("what's the idea?"), "{text}");
    app.handle_key(code(KeyCode::Esc));
    let text = click_text(&mut app, "[Help]");
    assert!(text.contains("help · getting around"), "{text}");
    let _ = std::fs::remove_dir_all(root);
}

/// A fake herdr that logs its argv and answers `pane get` with a pane in workspace w4.
#[cfg(unix)]
fn fake_herdr(dir: &Path) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let log = dir.join("herdr-calls.log");
    let bin = dir.join("fake-herdr");
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\nif [ \"$1 $2\" = 'pane get' ]; then\n  \
             echo '{{\"id\":\"x\",\"result\":{{\"pane\":{{\"pane_id\":\"w4:p1\",\"workspace_id\":\"w4\",\"tab_id\":\"w4:t1\",\"agent_status\":\"working\"}}}}}}'\n\
             else\n  echo '{{\"id\":\"x\",\"result\":{{}}}}'\nfi\n",
            log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    (bin, log)
}

#[cfg(unix)]
fn agent_fixture() -> (Fixture, PathBuf) {
    let mut fx = seeded();
    let (bin, log) = fake_herdr(fx.ctx.store.root());
    fx.ctx.herdr = corral_herdr::Herdr::with_bin(bin, None);
    fx.ctx
        .store
        .update("T-004", |t| {
            t.current_run_mut().unwrap().herdr = Some(HerdrRef {
                workspace_id: "w4".into(),
                pane_id: "w4:p1".into(),
                socket: None,
            });
            Ok(())
        })
        .unwrap();
    (fx, log)
}

fn focused_agent(log: &Path) -> bool {
    std::fs::read_to_string(log).is_ok_and(|calls| calls.contains("workspace focus w4"))
}

#[cfg(unix)]
#[test]
fn keyboard_only_open_agent_and_back_to_board() {
    let (fx, log) = agent_fixture();
    let mut app = fx.app(Mode::Board);
    app.start_tour_if_new();
    app.handle_key(code(KeyCode::Esc));
    // Running is the third column; `a` opens the agent's pane.
    app.handle_key(key('l'));
    app.handle_key(key('l'));
    let text = render(&mut app);
    let footer = footer(&text);
    assert!(footer.contains("a open agent"), "{footer}");
    app.handle_key(key('a'));
    assert!(focused_agent(&log), "a focuses the agent's workspace");
    // ⏎ opens the card, `a` there opens the agent too, esc is back on the board.
    std::fs::remove_file(&log).unwrap();
    app.handle_key(code(KeyCode::Enter));
    assert_eq!(app.stack.len(), 2);
    app.handle_key(key('a'));
    assert!(focused_agent(&log));
    app.handle_key(code(KeyCode::Esc));
    assert_eq!(app.stack.len(), 1, "esc is back on the board");
    assert!(render(&mut app).contains("Ctrl-b k back to board"));
}

#[cfg(unix)]
#[test]
fn mouse_only_open_agent_and_back_to_board() {
    let (fx, log) = agent_fixture();
    let mut app = fx.app(Mode::Board);
    app.start_tour_if_new();
    click_text(&mut app, "[Skip tour]");
    // Click the Running card, then the footer's `a open agent`.
    click_text(&mut app, "T-004");
    footer_click(&mut app, "a open agent");
    assert!(
        focused_agent(&log),
        "clicking `a open agent` focuses the agent"
    );
    // A second click on the selected card opens it; `‹ back` returns to the board.
    std::fs::remove_file(&log).unwrap();
    click_text(&mut app, "T-004");
    assert_eq!(app.stack.len(), 2, "second click opens the card");
    footer_click(&mut app, "a jump");
    assert!(focused_agent(&log), "detail `a jump` focuses the agent");
    click_text(&mut app, "‹ back");
    assert_eq!(app.stack.len(), 1, "‹ back returns to the board");
}

/// Renders one guide popup alone in a `w` x `h` pane and returns its text as single-spaced words.
fn popup_words(view: &mut dyn View, ctx: &Ctx, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let mut hits = HitMap::default();
    term.draw(|f| view.draw(f, f.area(), ctx, &mut hits))
        .unwrap();
    let text = buffer_text(term.backend().buffer());
    let words: Vec<&str> = text
        .lines()
        .flat_map(|l| l.trim().trim_matches('│').split_whitespace())
        .collect();
    words.join(" ")
}

#[test]
fn leave_popup_shows_every_instruction_in_a_narrow_empty_board_pane() {
    // Given no running agents, custom keys, and the 55-column board pane of an 80-column terminal.
    let (ctx, root) = empty_ctx();
    for height in [30, 12, 9] {
        let mut leave = LeaveView::new(0, custom_nav());
        let text = popup_words(&mut leave, &ctx, 55, height);
        // Then every sentence and both buttons are complete, even without room for blank lines.
        for want in [
            "Nothing is running right now; your tasks are saved.",
            "Come back with `corral up`, or press Ctrl-Alt-b inside the session.",
            "To leave the whole session, press Ctrl-a, then d.",
            "[Close board] [Stay]",
        ] {
            assert!(text.contains(want), "55x{height}: missing {want:?}\n{text}");
        }
    }
    // And the last tour card keeps its whole leaving text there too.
    let mut tour = TourView::new(custom_nav());
    tour.step = TourView::STEPS - 1;
    for height in [30, 12] {
        let text = popup_words(&mut tour, &ctx, 55, height);
        for want in [
            "To leave the whole session, press Ctrl-a, then d.",
            "Press ? for help or t to see this tour again.",
            "[‹ Back] [Done]",
        ] {
            assert!(
                text.contains(want),
                "tour 55x{height}: missing {want:?}\n{text}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn narrow_empty_board_leave_buttons_work_by_click_and_by_key() {
    let (ctx, root) = empty_ctx();
    let mut app = App::with_nav(ctx, Mode::Board, custom_nav()).unwrap();
    let narrow = |app: &mut App| render_at(app, 55, 30);
    app.handle_key(key('q'));
    let text = narrow(&mut app);
    assert!(text.contains("press Ctrl-a, then d."), "{text}");
    let at = |text: &str, needle: &str| find_any(text, needle);
    let stay = at(&text, "[Stay]");
    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        stay.0,
        stay.1,
    ));
    assert!(
        !app.quit && !narrow(&mut app).contains("close the board?"),
        "Stay keeps the board"
    );
    app.handle_key(key('q'));
    let text = narrow(&mut app);
    let close = at(&text, "[Close board]");
    app.handle_mouse(mouse(
        MouseEventKind::Down(MouseButton::Left),
        close.0,
        close.1,
    ));
    assert!(app.quit, "Close board closes it");
    let _ = std::fs::remove_dir_all(root);
}
