use std::path::Path;

use corral_core::Usage;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::BoardView;
use super::tests::{Fixture, buffer_text, seeded};
use crate::app::sync_step;
use crate::{HitMap, View};

const WORKTREE: &str = "/tmp/wt";

fn render(b: &mut BoardView, fx: &Fixture) -> String {
    let mut term = Terminal::new(TestBackend::new(160, 45)).unwrap();
    let mut hits = HitMap::default();
    term.draw(|f| b.draw(f, f.area(), &fx.ctx, &mut hits))
        .unwrap();
    buffer_text(term.backend().buffer())
}

fn write_transcript(agent_dir: &Path, total_usd: f64) {
    let dir = agent_dir.join("sessions").join("--tmp-wt--");
    std::fs::create_dir_all(&dir).unwrap();
    let header = format!(
        r#"{{"type":"session","cwd":"{WORKTREE}","timestamp":"2026-01-01T12:00:00.000Z","id":"s1"}}"#
    );
    let message = format!(
        r#"{{"type":"message","timestamp":"2026-01-01T12:05:00.000Z","id":"m1","message":{{"role":"assistant","usage":{{"input":10,"output":5,"cost":{{"total":{total_usd}}}}}}}}}"#
    );
    std::fs::write(dir.join("s.jsonl"), format!("{header}\n{message}\n")).unwrap();
}

/// T-004 is Running with a cached $1.50 whose harness transcript now says `total_usd`.
fn running_with_stale_cost(fx: &mut Fixture, total_usd: f64) {
    let agent_dir = fx.ctx.store.root().join("omo-agent");
    write_transcript(&agent_dir, total_usd);
    fx.ctx.store.config.sync.omo_agent_dir = Some(agent_dir);
    fx.ctx
        .store
        .update("T-004", |t| {
            let run = t.current_run_mut().unwrap();
            run.harness = Some("omo".into());
            run.worktree = WORKTREE.into();
            run.usage = Some(Usage {
                input: 10,
                output: 5,
                reported_usd: Some(1.5),
                cost_usd: Some(1.5),
                ..Usage::default()
            });
            Ok(())
        })
        .unwrap();
}

#[test]
fn running_card_cost_follows_the_transcript_on_the_boards_background_update() {
    let mut fx = seeded();
    running_with_stale_cost(&mut fx, 4.5);
    let mut board = fx.board();
    assert!(
        render(&mut board, &fx).contains("$1.50"),
        "cached cost first"
    );

    sync_step::run(&fx.ctx).unwrap();
    board.refresh(&fx.ctx);

    let text = render(&mut board, &fx);
    assert!(text.contains("$4.50") && !text.contains("$1.50"), "{text}");
}

#[test]
fn done_card_shows_the_final_cost_including_reviews_and_is_not_rescanned() {
    let mut fx = seeded();
    running_with_stale_cost(&mut fx, 9.0);
    let template = fx
        .ctx
        .store
        .get("T-004")
        .unwrap()
        .current_run()
        .unwrap()
        .clone();
    fx.ctx
        .store
        .update("T-007", |t| {
            let mut run = template.clone();
            run.worktree = WORKTREE.into();
            run.harness = Some("omo".into());
            run.ended = Some(run.started);
            run.usage = Some(Usage {
                input: 10,
                reported_usd: Some(3.0),
                cost_usd: Some(3.0),
                ..Usage::default()
            });
            let mut review = run.clone();
            review.review_runs.clear();
            review.usage = Some(Usage {
                input: 4,
                reported_usd: Some(0.5),
                cost_usd: Some(0.5),
                ..Usage::default()
            });
            run.review_runs = vec![review];
            t.runs = vec![run];
            Ok(())
        })
        .unwrap();
    let mut board = fx.board();
    assert!(render(&mut board, &fx).contains("$3.50"), "run plus review");
    sync_step::run(&fx.ctx).unwrap();
    board.refresh(&fx.ctx);
    let text = render(&mut board, &fx);
    assert!(text.contains("$3.50"), "{text}");
    let done = fx.ctx.store.get("T-007").unwrap();
    assert_eq!(
        corral_core::cost_text(&done),
        "$3.50",
        "a rescan of the shared transcript would read $9.00"
    );
}

#[test]
fn a_failed_usage_refresh_goes_to_the_message_sink_not_the_terminal() {
    let mut fx = seeded();
    running_with_stale_cost(&mut fx, 4.5);
    let events = fx.ctx.store.dir().join("events.jsonl");
    let _ = std::fs::remove_file(&events);
    std::fs::create_dir_all(&events).unwrap();
    assert!(
        fx.ctx.store.events().is_err(),
        "an unreadable event log fails the refresh"
    );
    let (tx, rx) = std::sync::mpsc::channel();
    let _guard = corral_core::capture_messages(tx);
    corral_ops::refresh_active_usage(&fx.ctx.store);
    let message = rx.try_recv().expect("a diagnostic through the sink");
    assert_eq!(message.source, "Usage not refreshed");
    assert_eq!(message.task.as_deref(), Some("T-004"));
}
