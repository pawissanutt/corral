use corral_core::Status;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::KeyCode;

use ratatui::Terminal;

use super::mouse_tests::{click, find_any, render};

use super::App;
use crate::artifacts::panels_fixture::fixture;
use crate::board::tests::{buffer_text, code, key, seeded};
use crate::{Action, Hit, Mode, Toast, Tone};

use crate::hit;

#[test]
fn every_footer_action_remains_clickable_at_eighty_columns() {
    // Given a board whose hints exceed a single terminal row.
    let fx = seeded();
    let mut app = fx.app(Mode::Board);
    let mut term = Terminal::new(TestBackend::new(80, 24)).unwrap();
    let hints = app.stack[0].hints();

    // When it is drawn at a normal small terminal size.
    term.draw(|f| app.draw(f)).unwrap();
    let text = buffer_text(term.backend().buffer());

    // Then every action has a visible hit, including Help and Quit at the end.
    for (label, _) in hints {
        for (_, _, key) in hit::hint_keys(label) {
            assert!(
                app.hits
                    .iter()
                    .any(|(r, h)| { *h == Hit::Key(key) && app.hits.at(r.x, r.y) == Some(h) }),
                "{label} has no visible hit\n{text}"
            );
        }
    }
    let (x, y) = find_any(&text, "? keys");
    click(&mut app, x, y);
    assert!(render(&mut app).contains("move selection"));
}

#[test]
fn toast_function_keys_activate_each_visible_toast_like_clicks() {
    // Given a plain toast and a linked task toast.
    let fx = seeded();
    let mut app = fx.app(Mode::Board);
    app.toast(Toast::new(Tone::Info, "older", "plain"));
    app.toast(Toast::new(Tone::Info, "middle", "plain"));
    app.toast(Toast::new(Tone::Attention, "newest", "task").about("T-005"));

    // When function keys activate oldest, middle, then newest without touching the mouse.
    app.handle_key(code(KeyCode::F(10)));
    assert_eq!(
        app.toasts
            .iter()
            .map(|(t, _)| t.title.as_str())
            .collect::<Vec<_>>(),
        ["middle", "newest"]
    );
    app.handle_key(code(KeyCode::F(9)));
    app.handle_key(code(KeyCode::F(8)));

    // Then plain toasts are dismissed and the linked task opens.
    assert!(app.toasts.is_empty());
    assert_eq!(app.stack.len(), 2);
    assert!(render(&mut app).contains("T-005"));
}

#[test]
fn capture_status_pills_have_keyboard_selection_before_save() {
    // Given capture's default Idea status.
    let fx = seeded();
    let mut app = fx.app(Mode::Capture(fx.ctx.store.root().into()));
    for c in "keyboard status".chars() {
        app.handle_key(key(c));
    }

    // When arrows select both pills, then save the Task choice.
    app.handle_key(code(KeyCode::Right));
    assert!(render(&mut app).contains("[task]"));
    app.handle_key(code(KeyCode::Left));
    assert!(render(&mut app).contains("[idea]"));
    app.handle_key(code(KeyCode::Right));
    app.handle_key(code(KeyCode::Enter));

    // Then Enter respects the selection and persists a Backlog task.
    assert_eq!(fx.ctx.store.get("T-008").unwrap().status, Status::Backlog);
    assert!(app.quit);
}

#[test]
fn review_file_keys_and_mouse_rows_jump_to_the_same_diffs() {
    // Given two diffs long enough that jumping changes the visible content.
    let fx = fixture();
    let worktree = fx.ctx.store.get(&fx.id).unwrap().runs[0].worktree.clone();
    let big: String = (1..=100).map(|i| format!("line {i}\n")).collect();
    std::fs::write(worktree.join("big.txt"), big).unwrap();
    let info = corral_ops::review_info(&fx.ctx, &fx.id).unwrap();
    let mut by_key = App::new(fx.ctx.clone(), Mode::Board).unwrap();
    by_key.apply(Action::OpenReview(fx.id.clone()));

    // When next-file keys visit each file, mouse row jumps produce identical frames.
    for file in &info.stat.files {
        by_key.handle_key(key(']'));
        let mut by_mouse = App::new(fx.ctx.clone(), Mode::Board).unwrap();
        by_mouse.apply(Action::OpenReview(fx.id.clone()));
        let (x, y) = find_any(&render(&mut by_mouse), &file.path);
        click(&mut by_mouse, x, y);
        assert_eq!(render(&mut by_key), render(&mut by_mouse));
    }

    // Then previous-file reverses the last jump and All artifacts is mouse-accessible.
    by_key.handle_key(key('['));
    let first = render(&mut by_key);
    let (_, y) = find_any(&first, "Files 2");
    let top = first.lines().nth(usize::from(y) + 1).unwrap();
    assert!(top.contains(&format!("diff --git a/{}", info.stat.files[0].path)));
    let (x, y) = find_any(&first, "[All artifacts]");
    click(&mut by_key, x, y);
    assert_eq!(by_key.stack.len(), 3);
}
