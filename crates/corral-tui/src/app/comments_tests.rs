//! Real app events, persisted task files, and rendered comment threads; no live herdr.
use corral_core::{CommentAuthor, HerdrRef, Status, Store, TaskComment};
use ratatui::crossterm::event::KeyCode;

use super::mouse_tests::{click, find_any, render};
use crate::artifacts::panels_fixture::key;
use crate::board::tests::{buffer_text, seeded};
use crate::{Action, Mode, View, detail::DetailView};

#[test]
fn comments_keyboard_submission_persists_owner_context_without_changing_spec() {
    // Given the real app displaying a task, with no live transport.
    let fx = seeded();
    let original = fx.ctx.store.get("T-003").unwrap();
    let mut app = fx.app(Mode::Board);
    app.apply(Action::OpenDetail(original.id.clone()));
    // When the owner navigates, enters a comment and submits using only keys.
    for code in [KeyCode::Char('6'), KeyCode::Char('c')] {
        app.handle_key(key(code));
    }
    let message = "Keep the cache bounded; preserve Unicode: 日本語.";
    for ch in message.chars() {
        app.handle_key(key(KeyCode::Char(ch)));
    }
    app.handle_key(key(KeyCode::Enter));
    // Then a reopened store has the note, attribution, and unchanged spec.
    let reopened = Store::open(fx.ctx.store.root()).unwrap();
    let task = reopened.get(&original.id).unwrap();
    assert_eq!(task.body, original.body);
    assert_eq!(task.comments.len(), 1);
    assert_eq!(task.comments[0].author, CommentAuthor::Owner);
    assert_eq!(task.comments[0].text, message);
    assert!(render(&mut app).contains("Keep the cache bounded;"));
}

#[test]
fn comments_mouse_controls_submit_and_protect_unsaved_input() {
    // Given task detail in the real app.
    let fx = seeded();
    let mut app = fx.app(Mode::Board);
    app.apply(Action::OpenDetail("T-003".into()));
    // When the owner clicks the tab, opens input, types, and clicks Save.
    let screen = render(&mut app);
    let (x, y) = find_any(&screen, "6 Comments");
    click(&mut app, x, y);
    let screen = render(&mut app);
    let (x, y) = find_any(&screen, "[Add comment]");
    click(&mut app, x, y);
    for ch in "mouse-saved context".chars() {
        app.handle_key(key(KeyCode::Char(ch)));
    }
    render(&mut app);
    click(&mut app, 0, 0);
    let screen = render(&mut app);
    assert!(screen.contains("mouse-saved context"));
    let (x, y) = find_any(&screen, "[Save comment]");
    click(&mut app, x, y);
    // Then the durable task and the visible thread contain exactly one note.
    let task = Store::open(fx.ctx.store.root())
        .unwrap()
        .get("T-003")
        .unwrap();
    assert_eq!(task.comments.len(), 1);
    assert_eq!(task.comments[0].author, CommentAuthor::Owner);
    assert_eq!(task.comments[0].text, "mouse-saved context");
    assert!(render(&mut app).contains("mouse-saved context"));
}

#[test]
fn comments_thread_renders_both_authors_timestamps_and_multiline_text() {
    // Given owner and PM notes with fixed timestamps.
    let fx = seeded();
    fx.ctx
        .store
        .update("T-003", |t| {
            t.comments = vec![
                TaskComment {
                    author: CommentAuthor::Owner,
                    at: "2026-10-15T12:30:00Z".parse().unwrap(),
                    text: "First line\nSecond line".into(),
                },
                TaskComment {
                    author: CommentAuthor::Pm,
                    at: "2026-10-15T12:31:00Z".parse().unwrap(),
                    text: "PM readback 日本語".into(),
                },
            ];
            Ok(())
        })
        .unwrap();
    let mut view = DetailView::new(&fx.ctx, "T-003").unwrap();
    view.on_key(key(KeyCode::Char('6')), &fx.ctx);
    // When rendering at wide and compact terminal widths.
    for width in [160, 120, 80] {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 36)).unwrap();
        let mut hits = crate::HitMap::default();
        terminal
            .draw(|f| view.draw(f, f.area(), &fx.ctx, &mut hits))
            .unwrap();
        let text = buffer_text(terminal.backend().buffer());
        // Then all tabs, both attributions, time and text remain readable and clickable.
        for expected in [
            "6 Comments",
            "owner",
            "2026-10-15 12:30 UTC",
            "First line",
            "Second line",
            "pm",
            "2026-10-15 12:31 UTC",
            "PM readback 日本語",
        ] {
            assert!(
                text.contains(expected),
                "missing {expected:?} at {width}\n{text}"
            );
        }
        assert!(
            hits.iter()
                .any(|(_, h)| *h == crate::Hit::Key(key(KeyCode::Char('6'))))
        );
        println!("comments-{width}:\n{text}");
    }
}

#[test]
fn comments_word_wrapped_thread_can_scroll_to_its_final_line() {
    // Given words that each occupy a wrapped row rather than filling every column.
    let fx = seeded();
    let text = format!(
        "{}FINAL-COMMENT-LINE",
        "abcdefghijabcdefghijabcdefghij ".repeat(100)
    );
    corral_ops::comment(&fx.ctx, "T-003", &text, CommentAuthor::Owner, false).unwrap();
    let mut view = DetailView::new(&fx.ctx, "T-003").unwrap();
    view.on_key(key(KeyCode::Char('6')), &fx.ctx);

    // When the reader scrolls through the entire long thread.
    let mut screen = String::new();
    for _ in 0..15 {
        view.on_key(key(KeyCode::PageDown), &fx.ctx);
        screen = crate::artifacts::panels_fixture::render_at(&mut view, &fx.ctx, 80, 30).0;
    }

    // Then the final content is reachable rather than clamped above it.
    assert!(screen.contains("FINAL-COMMENT-LINE"), "{screen}");
}

#[test]
fn comments_empty_submission_keeps_input_open_without_persisting() {
    // Given an empty composer.
    let fx = seeded();
    let mut view = DetailView::new(&fx.ctx, "T-003").unwrap();
    view.on_key(key(KeyCode::Char('6')), &fx.ctx);
    view.on_key(key(KeyCode::Char('c')), &fx.ctx);
    // When submitting whitespace.
    view.on_key(key(KeyCode::Char(' ')), &fx.ctx);
    let action = view.on_key(key(KeyCode::Enter), &fx.ctx);
    // Then input is retained and no comment is written.
    assert!(matches!(action, Action::Toast(t) if t.tone == crate::Tone::Warn));
    assert!(view.captures_text());
    assert!(fx.ctx.store.get("T-003").unwrap().comments.is_empty());
}

#[test]
fn comments_cancel_discards_draft_without_persisting() {
    // Given a typed comment.
    let fx = seeded();
    let mut app = fx.app(Mode::Board);
    app.apply(Action::OpenDetail("T-003".into()));
    for code in [KeyCode::Char('6'), KeyCode::Char('c'), KeyCode::Char('x')] {
        app.handle_key(key(code));
    }
    // When clicking Cancel.
    let screen = render(&mut app);
    let (x, y) = find_any(&screen, "[Cancel]");
    click(&mut app, x, y);
    // Then the composer closes without saving.
    assert!(!app.stack.last().unwrap().captures_text());
    assert!(fx.ctx.store.get("T-003").unwrap().comments.is_empty());
}

#[test]
fn comments_failed_send_closes_saved_composer_and_enter_cannot_duplicate() {
    // Given a running pane with a deliberately missing, isolated transport.
    let fx = seeded();
    fx.ctx
        .store
        .update("T-004", |t| {
            t.current_run_mut().unwrap().herdr = Some(HerdrRef {
                workspace_id: "w-test".into(),
                pane_id: "p-test".into(),
                socket: None,
            });
            Ok(())
        })
        .unwrap();
    let mut view = DetailView::new(&fx.ctx, "T-004").unwrap();
    for code in [
        KeyCode::Char('6'),
        KeyCode::Char('c'),
        KeyCode::Char('x'),
        KeyCode::Tab,
    ] {
        view.on_key(key(code), &fx.ctx);
    }
    // When Save appends successfully but live delivery fails, followed by another Enter.
    let result = view.on_key(key(KeyCode::Enter), &fx.ctx);
    view.on_key(key(KeyCode::Enter), &fx.ctx);
    // Then the saved note remains, and retrying Enter cannot append it again.
    assert!(
        matches!(result, Action::Toast(t) if t.tone == crate::Tone::Bad && t.body.contains("--resend"))
    );
    assert!(!view.captures_text());
    let task = fx.ctx.store.get("T-004").unwrap();
    assert_eq!(task.comments.len(), 1);
    assert_eq!(task.comments[0].text, "x");
    assert!(task.current_run().unwrap().pending_reply.is_some());
}

#[test]
fn comments_send_control_is_hidden_outside_live_task_statuses() {
    // Given an unfinished pane retained on Review/Done task metadata.
    let fx = seeded();
    for status in [Status::Review, Status::Done] {
        fx.ctx
            .store
            .update("T-004", |t| {
                t.status = status;
                t.current_run_mut().unwrap().herdr = Some(HerdrRef {
                    workspace_id: "w-test".into(),
                    pane_id: "p-test".into(),
                    socket: None,
                });
                Ok(())
            })
            .unwrap();
        let mut view = DetailView::new(&fx.ctx, "T-004").unwrap();
        // When opening the comment composer.
        view.on_key(key(KeyCode::Char('6')), &fx.ctx);
        view.on_key(key(KeyCode::Char('c')), &fx.ctx);
        let (text, _) = crate::artifacts::panels_fixture::render_at(&mut view, &fx.ctx, 80, 30);
        // Then it offers persistence only, not live sending.
        assert!(!text.contains("Send to live agent"), "{text}");
    }
}
