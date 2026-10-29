use corral_core::{Density, Status};
use jiff::SignedDuration;
use ratatui::{Terminal, backend::TestBackend};
use serde_json::json;
use unicode_width::UnicodeWidthStr;

use super::BoardSnapshot;
use super::tests::{Fixture, buffer_text, key, run, seeded};
use crate::{Action, Hit, HitMap, View};

/// Set CORRAL_T550_EVIDENCE to export the six real BoardView frames for xterm rendering.
fn capture(name: &str, buffer: &ratatui::buffer::Buffer) {
    let Some(directory) = std::env::var_os("CORRAL_T550_EVIDENCE") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join(format!("{name}.txt")), buffer_text(buffer)).unwrap();
    let lines = (0..buffer.area.height)
        .map(|y| {
            let mut skip = 0;
            let mut spans = Vec::new();
            for x in 0..buffer.area.width {
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                let cell = &buffer[(x, y)];
                skip = cell.symbol().width().saturating_sub(1);
                spans.push(ratatui::text::Span::styled(cell.symbol(), cell.style()));
            }
            ratatui::text::Line::from(spans)
        })
        .collect::<Vec<_>>();
    let ansi = crate::render::to_ansi(&lines).replace('\n', "\r\n");
    std::fs::write(directory.join(format!("{name}.ansi")), ansi).unwrap();
}

pub(crate) fn parked() -> Fixture {
    let fx = seeded();
    let at = fx.now.get() - SignedDuration::from_secs(60);
    fx.ctx
        .store
        .update("T-004", |task| {
            task.current_run_mut().unwrap().obligations.waits = vec![
                serde_json::from_value(json!({
                    "request": {
                        "version": 1, "id": "W1", "run": "R1",
                        "condition": "Dependency lands", "owner": "worker", "source": "worker",
                        "handle": {
                            "kind": "channel", "channel_id": "task:T-002:landed",
                            "capability": "wait_v1"
                        }
                    },
                    "registered_at": at,
                    "observation": {"at": at, "state": "pending", "detail": ""}
                }))
                .unwrap(),
            ];
            Ok(())
        })
        .unwrap();
    let worker = fx
        .ctx
        .store
        .create("Ordinary worker", None, Status::Ready, "")
        .unwrap();
    fx.ctx
        .store
        .update(&worker.id, |task| {
            task.status = Status::Running;
            task.runs.push(run("R1", "omo"));
            Ok(())
        })
        .unwrap();
    fx
}

#[test]
fn parked_membership_preserves_actions_slots_and_done_history() {
    // Given a native parked run, ordinary worker and fixed-clock board snapshot.
    let fx = parked();
    let mut board = fx.board();
    let usage = board
        .index
        .usage(&board.tasks, &fx.ctx.store.config.limits)
        .clone();

    // When navigation selects the derived Blocked column's parked card.
    board.selected = Some("T-004".into());
    board.restore_selection();

    // Then selection and membership change without changing the canonical action contract.
    assert_eq!(board.selected_task().unwrap().status, Status::Running);
    assert_eq!(board.column_tasks(Status::NeedsMe).len(), 3);
    assert_eq!(board.column_tasks(Status::Running).len(), 1);
    assert_eq!(
        board.on_key(key('a'), &fx.ctx),
        Action::Jump("T-004".into())
    );
    assert!(matches!(board.on_key(key('r'), &fx.ctx), Action::Toast(_)));
    assert!(board.reply.is_none());
    assert_eq!(fx.ctx.store.get("T-004").unwrap().status, Status::Running);
    assert_eq!(usage.tasks, 2, "native parking releases its worker slot");
    assert_eq!(board.done_cards().0[0].id, "T-007");
}

#[test]
fn refresh_returns_resumed_worker_to_running_and_keeps_selection() {
    // Given a selected card in derived Blocked membership.
    let fx = parked();
    let mut board = fx.board();
    board.selected = Some("T-004".into());
    board.restore_selection();
    let before = board
        .index
        .usage(&board.tasks, &fx.ctx.store.config.limits)
        .total();
    fx.ctx
        .store
        .update("T-004", |task| {
            task.current_run_mut().unwrap().obligations.waits.clear();
            Ok(())
        })
        .unwrap();
    let snapshot = BoardSnapshot::load_at(&fx.ctx, fx.now.get()).unwrap();

    // When the worker refresh replaces the attention snapshot.
    board.apply_snapshot(snapshot);

    // Then selection follows the card and native resumption reoccupies its worker slot.
    assert_eq!(board.selected_task().unwrap().id, "T-004");
    assert_eq!(board.columns()[board.col], Status::Running);
    assert_eq!(board.column_tasks(Status::NeedsMe).len(), 2);
    assert_eq!(board.column_tasks(Status::Running).len(), 2);
    assert_eq!(
        board
            .index
            .usage(&board.tasks, &fx.ctx.store.config.limits)
            .total(),
        before + 1
    );
}

#[test]
fn missing_plan_reviewer_uses_blocked_membership_without_rewriting_status() {
    // Given a recorded review attempt with no reviewer launched.
    let fx = seeded();
    let at = fx.now.get();
    fx.ctx
        .store
        .update("T-004", |task| {
            task.runs.clear();
            task.plans.push(serde_json::from_value(json!({
                "id": "D1", "spec_revision": "fixture",
                "artifact": {
                    "run": "P1", "kind": "report", "title": "Plan", "path": "plan.md",
                    "source": "/fixture/plan.md", "sha256": "abc", "bytes": 0, "added": at
                },
                "document": {
                    "goal": "fixture", "non_goals": [], "decisions": [], "affected_paths": [],
                    "contracts": [], "dependencies": [], "acceptance": [], "verification": [],
                    "owner_decisions": []
                },
                "reviews": [{"id": "pr1", "reviewer": "fixture", "policy_digest": "fixture", "at": at}]
            })).unwrap());
            Ok(())
        })
        .unwrap();

    // When the real board loads the attention projection.
    let mut board = fx.board();
    board.selected = Some("T-004".into());
    board.restore_selection();

    // Then the index places it in Blocked while the plan action remains canonical.
    assert_eq!(board.columns()[board.col], Status::NeedsMe);
    assert_eq!(board.selected_task().unwrap().status, Status::Running);
    assert_eq!(
        board.on_key(key('r'), &fx.ctx),
        Action::OpenPlan("T-004".into())
    );
}

#[test]
fn blocked_cards_render_in_derived_column_at_every_density_and_size() {
    // Given a parked task with a known age and an ordinary worker in a fixed-clock fixture.
    let mut fx = parked();
    for density in [Density::Comfortable, Density::Rich, Density::Compact] {
        fx.ctx.store.config.board.card.density = density;
        let mut board = fx.board();
        board.selected = Some("T-004".into());
        board.restore_selection();
        for (width, height) in [(80, 24), (160, 45)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut hits = HitMap::default();

            // When the real BoardView draws through ratatui's backend.
            terminal
                .draw(|frame| {
                    board.draw(frame, frame.area(), &fx.ctx, &mut hits);
                })
                .unwrap();

            // Then card hit targets use visual membership at both terminal sizes.
            let blocked = board
                .col_rects
                .iter()
                .find(|(status, _)| *status == Status::NeedsMe)
                .unwrap()
                .1;
            let card = hits
                .regions()
                .iter()
                .find_map(|(rect, hit)| {
                    matches!(hit, Hit::Card(id) if id == "T-004").then_some(*rect)
                })
                .unwrap();
            assert_eq!(card.x, blocked.x + 1);
            assert_eq!(card.width, blocked.width - 2);
            assert!(card.bottom() < blocked.bottom());
            assert!(
                card.height >= 5,
                "blocked details must survive compact density"
            );
            println!(
                "BLOCKED BOARD {density:?} {width}x{height}\n{}",
                buffer_text(terminal.backend().buffer())
            );
            capture(
                &format!(
                    "blocked-{}-{width}x{height}",
                    density.label().to_ascii_lowercase()
                ),
                terminal.backend().buffer(),
            );
        }
    }
}
