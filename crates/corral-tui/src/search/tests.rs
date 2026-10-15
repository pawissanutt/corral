use corral_core::Status;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};

use super::{SearchView, Z_LIST, Z_ROW};
use crate::app::mouse_tests::mouse;
use crate::board::tests::{Fixture, buffer_text, code, key, seeded};
use crate::{Action, Hit, HitMap, MouseCtx, View};

fn filter(view: &mut SearchView, fx: &Fixture, text: &str) {
    view.on_key(
        KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
        &fx.ctx,
    );
    for c in text.chars() {
        view.on_key(key(c), &fx.ctx);
    }
}

fn draw(view: &mut SearchView, fx: &Fixture, width: u16, height: u16) -> (String, HitMap) {
    let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
    let mut hits = HitMap::default();
    term.draw(|f| view.draw(f, f.area(), &fx.ctx, &mut hits))
        .unwrap();
    (buffer_text(term.backend().buffer()), hits)
}

#[test]
fn search_includes_hidden_statuses_and_uses_shared_ranking_live() {
    let fx = seeded();
    let dropped = fx
        .ctx
        .store
        .create("Dropped cache", None, Status::Dropped, "")
        .unwrap();
    let mut view = SearchView::new(&fx.ctx).unwrap();
    assert_eq!(view.matches.len(), 8);
    for status in [
        Status::Idea,
        Status::Backlog,
        Status::Ready,
        Status::Running,
        Status::NeedsMe,
        Status::Review,
        Status::Done,
        Status::Dropped,
    ] {
        assert!(
            view.matches
                .iter()
                .any(|&i| view.data.tasks[i].status == status)
        );
    }
    filter(&mut view, &fx, "cache");
    let query = corral_core::query::Query::parse("cache", jiff::Timestamp::now()).unwrap();
    assert_eq!(view.matches, view.data.ranked(&query));
    assert!(view.matches.len() >= 3);
    filter(&mut view, &fx, &dropped.id);
    assert_eq!(view.selected().unwrap().id, dropped.id);
    assert_eq!(
        view.on_key(code(KeyCode::Enter), &fx.ctx),
        Action::OpenDetail(dropped.id)
    );

    // Updates to disk are reflected through the same refresh used after periodic sync.
    filter(&mut view, &fx, "newly indexed");
    assert!(view.matches.is_empty());
    let added = fx
        .ctx
        .store
        .create("Newly indexed", None, Status::Backlog, "")
        .unwrap();
    view.refresh(&fx.ctx);
    assert_eq!(view.selected().unwrap().id, added.id);
    fx.ctx
        .store
        .update(&added.id, |task| {
            task.title = "Renamed".into();
            Ok(())
        })
        .unwrap();
    view.refresh(&fx.ctx);
    assert!(view.matches.is_empty());
}

#[test]
fn search_finds_and_negates_comment_only_text() {
    // Given an idea and a finished task with durable comment-only matches.
    let fx = seeded();
    let mut expected = Vec::new();
    for status in [Status::Idea, Status::Done] {
        let task = fx
            .ctx
            .store
            .create("Unrelated task", None, status, "")
            .unwrap();
        corral_ops::comment(
            &fx.ctx,
            &task.id,
            "zqxcommentonly047",
            corral_core::CommentAuthor::Owner,
            false,
        )
        .unwrap();
        expected.push(task.id);
    }
    let mut view = SearchView::new(&fx.ctx).unwrap();

    // When the live query matches only the comments.
    filter(&mut view, &fx, "zqxcommentonly047");
    let mut ids: Vec<_> = view
        .matches
        .iter()
        .map(|&i| view.data.tasks[i].id.clone())
        .collect();
    ids.sort();

    // Then both tasks are found, and negating the comment removes both.
    assert_eq!(ids, expected);
    filter(&mut view, &fx, "-zqxcommentonly047");
    assert!(
        view.matches
            .iter()
            .all(|&i| !expected.contains(&view.data.tasks[i].id))
    );
}

#[test]
fn malformed_search_is_inline_and_recovers_without_stale_selection() {
    let fx = seeded();
    let mut view = SearchView::new(&fx.ctx).unwrap();
    filter(&mut view, &fx, "\"unterminated");
    let (text, _) = draw(&mut view, &fx, 100, 20);
    assert!(view.error.is_some(), "{text}");
    assert!(text.contains(view.error.as_ref().unwrap()), "{text}");
    assert!(view.matches.is_empty());
    assert_eq!(view.on_key(code(KeyCode::Enter), &fx.ctx), Action::None);
    filter(&mut view, &fx, "T-004");
    assert!(view.error.is_none());
    assert_eq!(
        view.on_key(code(KeyCode::Enter), &fx.ctx),
        Action::OpenDetail("T-004".into())
    );
    assert_eq!(view.on_key(code(KeyCode::Esc), &fx.ctx), Action::Back);
}

#[test]
fn search_renders_and_maps_visible_rows_at_narrow_and_wide_sizes() {
    let fx = seeded();
    let mut view = SearchView::new(&fx.ctx).unwrap();
    for (width, height) in [(160, 45), (80, 24), (40, 12)] {
        view.on_key(code(KeyCode::PageDown), &fx.ctx);
        let (text, hits) = draw(&mut view, &fx, width, height);
        println!("Search TestBackend {width}x{height}:\n{text}");
        let selected = view.selected().unwrap();
        assert!(text.contains(&selected.id), "{width}x{height}\n{text}");
        let rows: Vec<_> = hits
            .iter()
            .filter_map(|(r, h)| match h {
                Hit::Item(Z_ROW, n) => Some((r, *n)),
                _ => None,
            })
            .collect();
        assert!(!rows.is_empty());
        assert_eq!(rows[0].1, view.offset);
        for (r, n) in rows {
            assert!(r.right() <= width && r.bottom() <= height);
            assert_eq!(hits.at(r.x, r.y), Some(&Hit::Item(Z_ROW, n)));
            let task = &view.data.tasks[view.matches[n]];
            assert!(
                text.lines()
                    .nth(usize::from(r.y))
                    .unwrap()
                    .contains(&task.id),
                "{text}"
            );
        }
        assert!(hits.iter().any(|(_, h)| *h == Hit::Key(code(KeyCode::Esc))));
    }
}

#[test]
fn wheel_click_and_double_click_open_the_pointer_task() {
    let fx = seeded();
    let mut view = SearchView::new(&fx.ctx).unwrap();
    let event = mouse(MouseEventKind::ScrollDown, 2, 6);
    let context = MouseCtx {
        ctx: &fx.ctx,
        hit: Some(Hit::Item(Z_LIST, 0)),
        double: false,
    };
    view.on_mouse(event, &context);
    assert_eq!(view.sel, 1);
    view.on_key(code(KeyCode::Up), &fx.ctx);
    assert_eq!(view.sel, 0);
    let target = view.data.tasks[view.matches[2]].id.clone();
    let click = mouse(MouseEventKind::Down(MouseButton::Left), 2, 8);
    let context = MouseCtx {
        ctx: &fx.ctx,
        hit: Some(Hit::Item(Z_ROW, 2)),
        double: false,
    };
    assert_eq!(view.on_mouse(click, &context), Action::None);
    assert_eq!(view.on_mouse(click, &context), Action::OpenDetail(target));
    let target = view.data.tasks[view.matches[3]].id.clone();
    let context = MouseCtx {
        ctx: &fx.ctx,
        hit: Some(Hit::Item(Z_ROW, 3)),
        double: true,
    };
    assert_eq!(view.on_mouse(click, &context), Action::OpenDetail(target));
}
