//! Real app input and rendering, without herdr or background workers.

use corral_core::{Accent, Status};
use ratatui::crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{Terminal, backend::TestBackend};

use crate::board::tests::{buffer_text, code, seeded};
use crate::board::{CardExtra, card_lines};
use crate::{Hit, Mode, theme};

#[test]
fn remapped_board_shortcuts_do_not_intercept_text_when_capture_is_open() {
    // Given capture bound to z rather than n.
    let mut fx = seeded();
    fx.ctx
        .store
        .config
        .ui
        .keys
        .insert("capture".into(), "z".into());
    let mut app = fx.app(Mode::Board);
    // When the old binding, new binding, and text are sent through the app.
    app.handle_key(code(KeyCode::Char('n')));
    assert!(!app.stack[0].captures_text());
    app.handle_key(code(KeyCode::Char('z')));
    assert!(app.stack[0].captures_text());
    app.handle_key(code(KeyCode::Char('n')));
    // Then capture has received literal text, not another board action.
    assert!(app.stack[0].has_unsaved_text());
}

#[test]
fn remapped_footer_shows_physical_key_and_clicks_canonical_action() {
    // Given capture bound to z and a rendered board.
    let mut fx = seeded();
    fx.ctx
        .store
        .config
        .ui
        .keys
        .insert("capture".into(), "z".into());
    let mut app = fx.app(Mode::Board);
    let mut term = Terminal::new(TestBackend::new(120, 36)).unwrap();
    term.draw(|frame| app.draw(frame)).unwrap();
    assert!(buffer_text(term.backend().buffer()).contains("z new"));
    let point = (0..36).flat_map(|y| (0..120).map(move |x| (x,y)))
        .find(|(x,y)| matches!(app.hits.at(*x,*y), Some(Hit::Key(k)) if k.code == KeyCode::Char('n')))
        .unwrap();
    // When the same footer action is activated by mouse.
    app.handle_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: point.0,
        row: point.1,
        modifiers: KeyModifiers::NONE,
    });
    // Then capture opens even though n is no longer a physical shortcut.
    assert!(app.stack[0].captures_text());
}

#[test]
fn display_preferences_change_rendered_board_when_loaded() {
    for (width, height) in [(80, 24), (160, 45)] {
        // Given a compact, magenta board showing only Backlog and Ready without WIP.
        let mut fx = seeded();
        fx.ctx.store.config.ui.columns = vec![Status::Backlog, Status::Ready];
        fx.ctx.store.config.ui.compact_cards = true;
        fx.ctx.store.config.ui.show_wip = false;
        fx.ctx.store.config.ui.accent = Accent::Magenta;
        let mut app = fx.app(Mode::Board);
        // When the actual app frame is drawn at narrow and wide terminal sizes.
        let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
        term.draw(|frame| app.draw(frame)).unwrap();
        let buffer = term.backend().buffer();
        let text = buffer_text(buffer);
        // Then hidden columns/WIP are absent and accent cells have changed.
        assert!(!text.contains("Running") && !text.contains("Needs me") && !text.contains("Done"));
        assert!(!text.contains("slots"));
        assert!(text.contains("Backlog") && text.contains("Ready"));
        assert!(!buffer.content.iter().any(|cell| cell.fg == theme::CYAN));
        assert!(buffer.content.iter().any(|cell| cell.fg == theme::MAGENTA));
        let tasks = fx.ctx.store.list().unwrap();
        let lines = card_lines(
            &tasks[0],
            &tasks,
            &fx.ctx.store.config,
            60,
            false,
            fx.now.get(),
            CardExtra::default(),
        );
        assert_eq!(lines.len(), 1);
    }
}

#[test]
fn remapped_leave_opens_exit_prompt_when_physical_key_is_pressed() {
    // Given leave bound to x.
    let mut fx = seeded();
    fx.ctx
        .store
        .config
        .ui
        .keys
        .insert("leave".into(), "x".into());
    let mut app = fx.app(Mode::Board);
    // When the physical leave shortcut is pressed.
    app.handle_key(code(KeyCode::Char('x')));
    // Then the normal leave flow runs, rather than bypassing active-run confirmation.
    assert_eq!(app.stack.len(), 2);
    assert!(!app.quit);
}

#[test]
fn preferences_help_keeps_terminal_engine_chords_when_board_keys_are_remapped() {
    // Given board navigation and leave keys that differ from herdr's prefix bindings.
    let mut fx = seeded();
    fx.ctx.store.config.ui.keys.insert("up".into(), "u".into());
    fx.ctx
        .store
        .config
        .ui
        .keys
        .insert("leave".into(), "x".into());
    let mut app = fx.app(Mode::Board);
    // When Help is opened through the real app input path.
    app.handle_key(code(KeyCode::Char('?')));
    let mut term = Terminal::new(TestBackend::new(120, 36)).unwrap();
    term.draw(|frame| app.draw(frame)).unwrap();
    let screen = buffer_text(term.backend().buffer());
    // Then board labels use their bindings but terminal-engine labels stay literal.
    assert!(screen.contains("h j u l / arrows"));
    assert!(screen.contains("Ctrl-b k") && screen.contains("Ctrl-b q"));
    assert!(!screen.contains("Ctrl-b u") && !screen.contains("Ctrl-b x"));
}
