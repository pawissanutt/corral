//! The classic frame as the board applies it: a frame computed locally by `ClassicCache` and
//! the same frame decoded from its served JSON (the daemon path) render identically.
use std::sync::Arc;

use corral_core::{Status, TaskHead};
use corral_ops::Ctx;
use corral_ops::classic::{ClassicCache, ClassicFrame, ClassicInputs};
use jiff::Timestamp;
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

use super::BoardSnapshot;
use super::tests::{Fixture, buffer_text};
use crate::{HitMap, View};

/// A full local frame at `now`, computed from scratch as `BoardSnapshot::load_at` does.
pub(super) fn local_frame(ctx: &Ctx, now: Timestamp) -> Arc<ClassicFrame> {
    ClassicCache::new()
        .refresh(
            ctx,
            ClassicInputs {
                now,
                attention: None,
                force: true,
            },
        )
        .unwrap()
        .expect("a first refresh publishes a frame")
}

/// The frame's heads as the board holds them (detached, task-number order).
pub(super) fn heads(frame: &ClassicFrame) -> Vec<TaskHead> {
    frame
        .heads
        .iter()
        .cloned()
        .map(TaskHead::from_wire)
        .collect()
}

fn render(fx: &Fixture, frame: ClassicFrame) -> Buffer {
    let mut board = fx.board();
    board.apply_snapshot(BoardSnapshot::from_frame(&fx.ctx, frame).unwrap());
    board.routes.receive();
    let mut term = Terminal::new(TestBackend::new(160, 45)).unwrap();
    term.draw(|f| board.draw(f, f.area(), &fx.ctx, &mut HitMap::default()))
        .unwrap();
    term.backend().buffer().clone()
}

#[test]
fn classic_frame_round_trip_renders_identically() {
    // Given a Review task, a tracker with an approved scope and a pinned task.
    let fx = super::tracker_tests::scoped_tracker();
    fx.ctx.store.pin("T-007", None, false).unwrap();
    let frame = local_frame(&fx.ctx, fx.now.get());
    assert!(heads(&frame).iter().any(|h| h.status == Status::Review));
    assert!(
        frame
            .plan_progress
            .get(super::tracker_tests::TRACKER)
            .is_some_and(|progress| progress.is_ok())
    );
    assert_eq!(frame.pins.len(), 1);

    // When the local frame and the same frame decoded from its served JSON are applied.
    let local = render(&fx, ClassicFrame::clone(&frame));
    let served: ClassicFrame =
        serde_json::from_slice(&serde_json::to_vec(&*frame).unwrap()).unwrap();
    let decoded = render(&fx, served);

    // Then both boards render the same buffer, cell for cell.
    assert_eq!(local, decoded, "{}", buffer_text(&local));
}
