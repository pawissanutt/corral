use corral_core::{Density, Status};
use ratatui::{backend::TestBackend, crossterm::event::KeyCode};

use super::card_lines;
use super::tests::{buffer_text, code, key, seeded};
use crate::app::mouse_tests::{click, find_any, render};
use crate::detail::DetailView;
use crate::{Action, HitMap, Mode, View};

const TRACKER: &str = "T-002";
const CHILDREN: [&str; 4] = ["T-003", "T-004", "T-006", "T-007"];

fn tracker() -> super::tests::Fixture {
    let fx = seeded();
    fx.ctx
        .store
        .update(TRACKER, |t| {
            t.title = "Imported widget plan".into();
            t.tracker = true;
            t.children = CHILDREN.map(str::to_string).to_vec();
            Ok(())
        })
        .unwrap();
    for (id, status) in
        CHILDREN
            .into_iter()
            .zip([Status::Ready, Status::Backlog, Status::Done, Status::Done])
    {
        fx.ctx
            .store
            .update(id, |t| {
                t.parent = Some(TRACKER.into());
                t.status = status;
                t.runs.clear();
                Ok(())
            })
            .unwrap();
    }
    fx
}

#[test]
fn tracker_card_counts_done_children_in_every_density_and_status() {
    // Given a tracker with four children, exactly two Done.
    let mut fx = tracker();
    for density in [Density::Comfortable, Density::Rich, Density::Compact] {
        fx.ctx.store.config.board.card.density = density;
        for status in [Status::Backlog, Status::Ready, Status::Done] {
            let task = fx
                .ctx
                .store
                .update(TRACKER, |t| {
                    t.status = status;
                    Ok(())
                })
                .unwrap();
            for width in [12, 24, 80] {
                // When the real card renderer draws it, even with stale run metadata.
                let lines = card_lines(
                    &task,
                    &fx.ctx.store.list().unwrap(),
                    &fx.ctx.store.config,
                    width,
                    true,
                    fx.now.get(),
                    super::CardExtra {
                        stage: Some("gate running"),
                        wait: Some("eligible"),
                        ..super::CardExtra::default()
                    },
                );
                let text = lines
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n");
                // Then progress replaces run, route, stage and configured cost fields.
                assert!(
                    text.contains("2/4 done"),
                    "{density:?} {status:?} {width}\n{text}"
                );
                assert!(
                    !text.contains('$')
                        && !text.contains("decomposed")
                        && !text.contains("gate running")
                );
                assert!(!text.contains("eligible") && !text.contains("⇢"));
                assert!(lines.iter().all(|line| line.width() <= width + 2));
            }
        }
    }
}

#[test]
fn tracker_card_does_not_count_dropped_or_missing_children_as_done() {
    // Given one dropped and one missing child in the four-child tracker.
    let fx = tracker();
    fx.ctx
        .store
        .update("T-006", |t| {
            t.status = Status::Dropped;
            Ok(())
        })
        .unwrap();
    let mut task = fx.ctx.store.get(TRACKER).unwrap();
    task.children[0] = "T-999".into();
    // When progress is resolved from the current task inventory.
    let text = super::cards::tracker_progress(&task, &fx.ctx.store.list().unwrap());
    // Then only the remaining Done child counts.
    assert_eq!(text, "1/4 done");
}

#[test]
fn tracker_dispatch_is_absent_from_hints_and_menu_and_refused_by_key() {
    // Given a tracker in each status that normally offers dispatch.
    let fx = tracker();
    for status in [Status::Idea, Status::Backlog, Status::Ready] {
        fx.ctx
            .store
            .update(TRACKER, |t| {
                t.status = status;
                Ok(())
            })
            .unwrap();
        let mut board = fx.board();
        board.col = board
            .columns()
            .iter()
            .position(|s| {
                *s == if status == Status::Idea {
                    Status::Backlog
                } else {
                    status
                }
            })
            .unwrap();
        board.row = board
            .column_tasks(board.columns()[board.col])
            .iter()
            .position(|t| t.id == TRACKER)
            .unwrap();
        assert_eq!(board.selected_task().unwrap().id, TRACKER);
        assert!(!board.hints().iter().any(|(key, _)| *key == "d"));
        board.on_key(key('.'), &fx.ctx);
        assert!(
            !board
                .menu
                .as_ref()
                .unwrap()
                .items
                .iter()
                .any(|(key, _, _)| *key == KeyCode::Char('d')),
            "tracker menu must not offer dispatch"
        );
        board.on_key(code(KeyCode::Esc), &fx.ctx);
        // When dispatch is pressed directly.
        let result = board.on_key(key('d'), &fx.ctx);
        // Then it is refused without opening a dispatch sheet or creating a run.
        assert!(matches!(result, Action::Toast(t) if t.title == "Not dispatchable"));
        assert!(board.overlay.is_none());
        assert!(fx.ctx.store.get(TRACKER).unwrap().runs.is_empty());
        assert_eq!(
            board.on_key(code(KeyCode::Enter), &fx.ctx),
            Action::OpenDetail(TRACKER.into())
        );
    }
}

#[test]
fn tracker_detail_lists_children_without_a_plan_record_and_navigates_by_keyboard() {
    // Given an imported tracker without any PlanRecord.
    let fx = tracker();
    assert!(fx.ctx.store.get(TRACKER).unwrap().plans.is_empty());
    for width in [80, 160] {
        let mut view = DetailView::new(&fx.ctx, TRACKER).unwrap();
        view.on_key(key('1'), &fx.ctx);
        view.on_key(key('8'), &fx.ctx);
        let mut term = ratatui::Terminal::new(TestBackend::new(width, 30)).unwrap();
        term.draw(|f| view.draw(f, f.area(), &fx.ctx, &mut HitMap::default()))
            .unwrap();
        let text = buffer_text(term.backend().buffer());
        assert!(
            text.contains("7 Timeline") && text.contains("8 Plan"),
            "{text}"
        );
        assert!(text.contains("Child tasks (4)"), "{text}");
        for (id, status) in CHILDREN
            .into_iter()
            .zip(["Ready", "Backlog", "Done", "Done"])
        {
            let row = text
                .lines()
                .find(|line| line.contains(&format!("▸ {id}")))
                .unwrap();
            assert!(row.contains(status), "{row}");
        }
        assert!(!text.contains("Revisions") && !text.contains("[Approve plan]"));
        // When the second child is selected and opened.
        view.on_key(key('j'), &fx.ctx);
        // Then navigation targets that child.
        assert_eq!(
            view.on_key(code(KeyCode::Enter), &fx.ctx),
            Action::OpenDetail(CHILDREN[1].into())
        );
    }
}

#[test]
fn tracker_child_and_parent_links_work_through_the_app_by_mouse_and_keyboard() {
    // Given the actual app displaying an import tracker's children.
    let fx = tracker();
    let mut app = fx.app(Mode::Board);
    app.apply(Action::OpenDetail(TRACKER.into()));
    app.handle_key(key('8'));
    let text = render(&mut app);
    // When a child row is clicked.
    let (x, y) = find_any(&text, &format!("▸ {}", CHILDREN[0]));
    click(&mut app, x, y);
    app.handle_key(key('7'));
    let text = render(&mut app);
    // Then its detail shows a clickable parent backlink.
    assert!(text.contains(&format!("parent {TRACKER} [P]")), "{text}");
    let (x, y) = find_any(&text, &format!("parent {TRACKER}"));
    click(&mut app, x, y);
    app.handle_key(key('8'));
    assert!(render(&mut app).contains("Child tasks (4)"));
    app.handle_key(code(KeyCode::Enter));
    app.handle_key(key('P'));
    app.handle_key(key('8'));
    assert!(render(&mut app).contains("Child tasks (4)"));
}
