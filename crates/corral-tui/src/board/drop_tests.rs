use corral_core::Status;
use ratatui::crossterm::event::KeyCode;
use ratatui::{Terminal, backend::TestBackend};

use super::tests::{buffer_text, code, key, seeded};
use super::{Overlay, menu};
use crate::{Action, HitMap, View};

#[test]
fn drop_menu_exists_exactly_for_unfinished_statuses() {
    // Given every possible task status.
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
        // When its card menu is built, then only unfinished cards offer drop.
        let items = menu::items_for(status, false);
        assert_eq!(
            items
                .iter()
                .filter(|(k, _, _)| *k == KeyCode::Char('X'))
                .count(),
            usize::from(!matches!(status, Status::Done | Status::Dropped)),
            "{status:?}",
        );
    }
}

#[test]
fn drop_cancel_leaves_backlog_and_running_unchanged() {
    // Given either a backlog card or a card with a live run.
    for (id, status) in [("T-002", Status::Backlog), ("T-004", Status::Running)] {
        for cancel in [code(KeyCode::Esc), key('n')] {
            let fx = seeded();
            let before = fx.ctx.store.get(id).unwrap();
            let events = fx.ctx.store.events().unwrap();
            let mut board = fx.board();
            board.selected = Some(id.into());
            board.restore_selection();
            board.on_key(key('X'), &fx.ctx);
            assert!(matches!(board.overlay, Some(Overlay::Drop(_))));
            // When confirmation is cancelled.
            assert_eq!(board.on_key(cancel, &fx.ctx), Action::None);
            // Then no task, run or event was changed.
            let after = fx.ctx.store.get(id).unwrap();
            assert_eq!(after.status, status);
            assert_eq!(
                corral_core::render_task(&before),
                corral_core::render_task(&after)
            );
            assert_eq!(fx.ctx.store.events().unwrap().len(), events.len());
            assert!(board.overlay.is_none());
        }
    }
}

#[test]
fn drop_confirm_routes_backlog_and_running_to_existing_ops() {
    // Given backlog, running and needs-me cards.
    for (id, event) in [
        ("T-002", "moved"),
        ("T-004", "discarded"),
        ("T-005", "discarded"),
    ] {
        for confirm in [code(KeyCode::Enter), key('y')] {
            let fx = seeded();
            let mut board = fx.board();
            board.selected = Some(id.into());
            board.restore_selection();
            board.on_key(key('X'), &fx.ctx);
            // When the owner supplies a reason, including confirmation shortcut letters.
            board.on_key(code(KeyCode::Tab), &fx.ctx);
            for c in "no longer necessary".chars() {
                board.on_key(key(c), &fx.ctx);
            }
            board.on_key(code(KeyCode::Tab), &fx.ctx);
            let action = board.on_key(confirm, &fx.ctx);
            // Then the operation, reason and owner/tui provenance are persisted.
            assert!(matches!(action, Action::Toast(_)), "{action:?}");
            assert!(board.overlay.is_none(), "{action:?}");
            let task = fx.ctx.store.get(id).unwrap();
            assert_eq!(task.status, Status::Dropped);
            if let Some(run) = task.current_run() {
                assert_eq!(run.outcome, Some(corral_core::RunOutcome::Discarded));
                assert!(run.ended.is_some());
            }
            let events = fx.ctx.store.events().unwrap();
            let op = events.iter().rev().find(|e| e.kind == event).unwrap();
            assert_eq!(op.detail["actor"], "owner");
            assert_eq!(op.detail["reason"], "no longer necessary");
            let drop = events.last().unwrap();
            assert_eq!(drop.kind, "dropped");
            assert_eq!(drop.detail["actor"], "owner");
            assert_eq!(drop.detail["via"], "tui");
            assert_eq!(board.on_key(key('Y'), &fx.ctx), Action::OpenHistory);
            let mut history = crate::history::HistoryView::new(&fx.ctx).unwrap();
            let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
            terminal
                .draw(|f| history.draw(f, f.area(), &fx.ctx, &mut HitMap::default()))
                .unwrap();
            let text = buffer_text(terminal.backend().buffer());
            assert!(
                text.lines()
                    .any(|line| line.contains(id) && line.contains('✗')),
                "{text}"
            );
        }
    }
}

#[test]
fn drop_owner_idea_records_explicit_popup_consent() {
    // Given an owner-origin idea without prior consent.
    let fx = seeded();
    let mut board = fx.board();
    board.on_key(key('X'), &fx.ctx);
    // When the owner confirms without an optional reason.
    board.on_key(key('y'), &fx.ctx);
    // Then the existing owner-origin protection permits the drop.
    assert_eq!(fx.ctx.store.get("T-001").unwrap().status, Status::Dropped);
    let events = fx.ctx.store.events().unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.kind == "owner_decision" && e.detail["source"] == "tui")
    );
}

#[test]
fn drop_unfinished_dispatch_shows_error_without_changing_status() {
    // Given a backlog card whose dispatch has not finished.
    let fx = seeded();
    fx.ctx
        .store
        .update("T-002", |task| {
            task.launch = Some(corral_core::LaunchReservation {
                approved_plan: None,
                socket: None,
                stage: corral_core::LaunchStage::Reserved,
                reserved: true,
                error: None,
            });
            Ok(())
        })
        .unwrap();
    let mut board = fx.board();
    board.selected = Some("T-002".into());
    board.restore_selection();
    board.on_key(key('X'), &fx.ctx);
    // When the owner confirms the drop.
    let action = board.on_key(key('y'), &fx.ctx);
    // Then the ops error is visible and nothing was dropped.
    assert!(
        matches!(action, Action::Toast(ref toast)
        if toast.tone == crate::Tone::Bad && toast.body.contains("unfinished dispatch")),
        "{action:?}"
    );
    assert_eq!(fx.ctx.store.get("T-002").unwrap().status, Status::Backlog);
    assert!(board.overlay.is_some());
    assert!(
        !fx.ctx
            .store
            .events()
            .unwrap()
            .iter()
            .any(|e| e.kind == "dropped")
    );
}

#[test]
fn drop_running_and_needs_me_closes_owned_live_panes_only_after_confirmation() {
    // Given the plugin-free corral-test server supplied by scripts/test-herdr.sh.
    let herdr = corral_herdr::Herdr::with_bin("herdr".into(), Some("corral-test".into()));
    if !herdr.available() {
        println!("SKIP: isolated corral-test server unavailable");
        return;
    }
    struct Workspace(corral_herdr::Herdr, String);
    impl Drop for Workspace {
        fn drop(&mut self) {
            if let Ok(workspaces) = self.0.workspace_list()
                && workspaces.iter().any(|(id, _)| *id == self.1)
            {
                self.0.workspace_close(&self.1).unwrap();
            }
        }
    }
    for id in ["T-004", "T-005"] {
        for confirm in [false, true] {
            let mut fx = seeded();
            let created = herdr
                .workspace_create(fx.ctx.store.root(), "T-169-drop", false)
                .unwrap();
            let _guard = Workspace(herdr.clone(), created.workspace_id.clone());
            fx.ctx.herdr = herdr.clone();
            fx.ctx
                .store
                .update(id, |task| {
                    let run = task.current_run_mut().unwrap();
                    run.worktree = fx.ctx.store.root().join("absent-worktree");
                    run.herdr = Some(corral_core::HerdrRef {
                        workspace_id: created.workspace_id.clone(),
                        pane_id: created.pane_id.clone(),
                        socket: Some(herdr.socket_path().unwrap()),
                    });
                    Ok(())
                })
                .unwrap();
            let mut board = fx.board();
            board.selected = Some(id.into());
            board.restore_selection();
            board.on_key(key('X'), &fx.ctx);
            // When the drop is cancelled or confirmed through the board's real handler.
            board.on_key(if confirm { key('y') } else { key('n') }, &fx.ctx);
            // Then only confirmation closes the owned pane, preserving other tests' panes.
            let present = herdr
                .pane_list()
                .unwrap()
                .iter()
                .any(|p| p.pane_id == created.pane_id);
            assert_eq!(present, !confirm);
            assert_eq!(
                fx.ctx.store.get(id).unwrap().status == Status::Dropped,
                confirm
            );
            println!("LIVE DROP {id}: confirm={confirm}, owned pane present={present}");
        }
    }
}
