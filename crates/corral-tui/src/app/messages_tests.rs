use std::os::unix::fs::PermissionsExt;
use std::sync::mpsc;

use corral_herdr::Herdr;
use corral_ops::{Message, capture_messages};
use ratatui::{Terminal, backend::TestBackend};

use super::{App, WorkerMsg};
use crate::Mode;
use crate::board::tests::{buffer_text, key, seeded};

fn draw(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    buffer_text(terminal.backend().buffer())
}

#[test]
fn board_captures_sync_failures_once_without_raw_terminal_output() {
    // Given a real sync with a queued task whose project cannot be resolved.
    let mut fx = seeded();
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("herdr");
    std::fs::write(
        &bin,
        "#!/bin/sh\nprintf '%s\\n' '{\"id\":\"fake\",\"result\":{\"workspaces\":[],\"panes\":[]}}'\n",
    ).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    fx.ctx.herdr = Herdr::with_bin(bin, Some("corral-test".into()));
    fx.ctx
        .store
        .update("T-003", |task| {
            task.queued = true;
            Ok(())
        })
        .unwrap();
    let mut app = fx.app(Mode::Board);
    let baseline: Vec<_> = [(160, 45), (80, 24)]
        .into_iter()
        .map(|(width, height)| draw(&mut app, width, height))
        .collect();
    let ctx = app.ctx.clone();
    let sender = app.message_tx.clone();
    let (completed_tx, completed_rx) = mpsc::channel();
    // When the board's worker thread syncs twice, including a fresh config context.
    let worker = std::thread::spawn(move || {
        let _capture = capture_messages(sender);
        for _ in 0..2 {
            completed_tx.send(corral_ops::sync(&ctx)).unwrap();
        }
    });
    for _ in 0..2 {
        app.on_worker(WorkerMsg::Synced(
            completed_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap()
                .map_err(|error| format!("{error:#}")),
        ));
    }
    worker.join().unwrap();
    app.drain_messages();
    // Then the failure is a single toast and retained message, not a raw stderr line.
    assert_eq!(app.messages.len(), 1);
    assert_eq!(app.toasts.len(), 1);
    assert_eq!(app.toasts[0].0.task.as_deref(), Some("T-003"));
    assert!(
        app.toasts[0]
            .0
            .body
            .contains("dispatching queued T-003 failed")
    );
    for (width, height) in [(160, 45), (80, 24)] {
        let text = draw(&mut app, width, height);
        assert!(!text.contains("corral: dispatching"), "{text}");
        assert!(text.contains("dispatch"), "{text}");
        assert!(
            text.contains("M messages") && text.contains("corral"),
            "{text}"
        );
    }
    // The log remains accessible after the toast is dismissed.
    app.toasts.clear();
    for ((width, height), before) in [(160, 45), (80, 24)].into_iter().zip(baseline) {
        assert_eq!(
            draw(&mut app, width, height),
            before,
            "board buffer remains intact"
        );
    }
    app.handle_key(key('M'));
    let text = draw(&mut app, 160, 45);
    assert!(
        text.contains("Board messages") && text.contains("dispatching queued T-003 failed"),
        "{text}"
    );
}

#[test]
fn board_deduplication_is_per_task_and_resets_when_message_changes() {
    // Given two tasks reporting the same failure.
    let fx = seeded();
    let mut app = fx.app(Mode::Board);
    let message = |task: &str, text: &str| Message {
        task: Some(task.into()),
        source: "autopilot",
        text: text.into(),
    };
    // When failures repeat, change and recur after that change.
    for item in [
        message("T-003", "failed"),
        message("T-003", "failed"),
        message("T-004", "failed"),
        message("T-003", "changed"),
        message("T-003", "failed"),
    ] {
        app.on_message(item);
    }
    // Then only consecutive identical messages for the same task are suppressed.
    assert_eq!(app.messages.len(), 4);
}
