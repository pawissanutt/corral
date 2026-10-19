use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use corral_core::{Status, Store};
use corral_ops::Ctx;
use ratatui::{Frame, Terminal, backend::TestBackend, crossterm::event::KeyEvent, layout::Rect};

use super::{WorkerMsg, spawn_worker};
use crate::board::tests::{buffer_text, seeded};
use crate::{Action, Hit, HitMap, Mode, View};

#[test]
fn worker_result_moves_card_when_store_changes_between_ticks() {
    // Given a displayed Backlog card and a subsequent external Review transition.
    let mut fx = seeded();
    fx.ctx.store.config.dispatch.paused = true;
    fx.ctx.store.config.review.autopilot = Some(false);
    fx.ctx.store.save_config().unwrap();
    let mut app = fx.app(Mode::Board);
    fx.ctx
        .store
        .update("T-002", |task| {
            task.status = Status::Review;
            Ok(())
        })
        .unwrap();
    let (trigger, messages) = spawn_worker(fx.ctx.clone(), app.message_tx.clone());
    let update = loop {
        let message = messages.recv_timeout(Duration::from_secs(30)).unwrap();
        match message {
            WorkerMsg::Herdr(_) => {}
            synced @ WorkerMsg::Synced(_) => break synced,
        }
    };
    assert!(matches!(&update, WorkerMsg::Synced(Ok(_))));
    drop(trigger);
    assert!(matches!(
        messages.recv_timeout(Duration::from_secs(30)),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
    ));

    // When the real worker result reaches the application.
    app.on_worker(update);

    // Then the real rendered card is in Review, without an explicit reload action.
    let mut terminal = Terminal::new(TestBackend::new(160, 45)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    let text = buffer_text(terminal.backend().buffer());
    let heading = text.lines().find(|line| line.contains("Backlog")).unwrap();
    let review_x =
        unicode_width::UnicodeWidthStr::width(&heading[..heading.find("Review").unwrap()]);
    let card_x = (0..45)
        .flat_map(|y| (0..160).map(move |x| (x, y)))
        .find(|(x, y)| matches!(app.hits.at(*x, *y), Some(Hit::Card(id)) if id == "T-002"))
        .unwrap()
        .0;
    assert!(
        (review_x - 2..review_x + 24).contains(&usize::from(card_x)),
        "{text}"
    );
    assert_eq!(app.counts.review, 2);
}

struct RefreshCounter(Rc<Cell<usize>>);

impl View for RefreshCounter {
    fn draw(&mut self, _: &mut Frame, _: Rect, _: &Ctx, _: &mut HitMap) {}
    fn on_key(&mut self, _: KeyEvent, _: &Ctx) -> Action {
        Action::None
    }
    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        Vec::new()
    }
    fn refresh(&mut self, _: &Ctx) {
        self.0.set(self.0.get() + 1);
    }
}

#[test]
fn sync_refreshes_only_top_screen_when_other_screens_are_hidden() {
    // Given two non-board screens stacked above the board.
    let fx = seeded();
    let mut app = fx.app(Mode::Board);
    let hidden = Rc::new(Cell::new(0));
    let visible = Rc::new(Cell::new(0));
    app.stack.push(Box::new(RefreshCounter(hidden.clone())));
    app.stack.push(Box::new(RefreshCounter(visible.clone())));
    let update = WorkerMsg::synced(&fx.ctx, Ok(Vec::new()));

    // When a periodic snapshot arrives.
    app.on_worker(update);

    // Then only the visible top screen re-reads its data.
    assert_eq!((hidden.get(), visible.get()), (0, 1));
}

#[test]
fn worker_snapshot_refreshes_operating_header_after_external_policy_change() {
    // Given an already-live board displaying Assist.
    let fx = seeded();
    let mut app = fx.app(Mode::Board);
    let before = app.operating_header.clone();
    // When a separate store pauses policy and the ordinary sync snapshot arrives.
    Store::open(fx.ctx.store.root())
        .unwrap()
        .pause_operating_policy(true)
        .unwrap();
    let mut fresh = fx.ctx.clone();
    fresh.reload_config().unwrap();
    let snapshot = WorkerMsg::synced(&fresh, Ok(Vec::new()));
    app.on_worker(snapshot);
    // Then the live header matches the same fresh readback without manual reload.
    assert_ne!(app.operating_header, before);
    assert_eq!(app.operating_header, super::operating::readback(&fresh));
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    assert!(buffer_text(terminal.backend().buffer()).contains("(paused)"));
}
