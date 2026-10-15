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
    // A checked-in executable cannot inherit a writable fd from a parallel fixture setup.
    let bin = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app/herdr-empty.sh");
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
        file: None,
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

#[test]
fn detail_usage_refresh_failures_use_the_board_sink_once() {
    // Given a detail screen whose backing task disappears between refreshes.
    let fx = seeded();
    let mut app = fx.app(Mode::Board);
    let _capture = capture_messages(app.message_tx.clone());
    app.apply(crate::Action::OpenDetail("T-003".into()));
    std::fs::remove_file(fx.ctx.store.get("T-003").unwrap().path).unwrap();
    // When periodic board reloads refresh the active detail screen.
    app.reload();
    app.reload();
    app.drain_messages();
    // Then the usage error is retained once, not printed over the detail screen.
    assert_eq!(app.messages.len(), 1);
    assert_eq!(app.toasts.len(), 1);
    assert_eq!(app.toasts[0].0.title, "usage");
    assert_eq!(app.toasts[0].0.task.as_deref(), Some("T-003"));
    let text = draw(&mut app, 160, 45);
    assert!(!text.contains("corral: refreshing usage"), "{text}");
}

#[test]
fn malformed_task_files_are_captured_and_deduplicated_without_garbling_the_board() {
    // Given a board and two task files that become malformed while it owns the terminal.
    let fx = seeded();
    let mut app = fx.app(Mode::Board);
    let baseline: Vec<_> = [(160, 45), (80, 24)]
        .into_iter()
        .map(|(width, height)| draw(&mut app, width, height))
        .collect();
    let _capture = capture_messages(app.message_tx.clone());
    let broken = fx.ctx.store.dir().join("tasks/broken-a.md");
    std::fs::write(&broken, "not task frontmatter").unwrap();
    std::fs::write(
        fx.ctx.store.dir().join("tasks/broken-b.md"),
        "not task frontmatter",
    )
    .unwrap();
    // When repeated refreshes skip both files.
    app.reload();
    app.reload();
    app.drain_messages();
    // Then every file has one retained diagnostic, and no repeated toast or raw stderr text.
    assert_eq!(app.messages.len(), 2, "one message per malformed file");
    assert_eq!(app.toasts.len(), 2);
    assert!(app.toasts.iter().all(|(toast, _)| toast.task.is_none()));
    for (width, height) in [(160, 45), (80, 24)] {
        let text = draw(&mut app, width, height);
        assert!(!text.contains("corral: skipping"), "{text}");
    }
    app.toasts.clear();
    app.reload();
    app.drain_messages();
    assert!(
        app.toasts.is_empty(),
        "unchanged files do not re-toast after dismissal"
    );
    for ((width, height), before) in [(160, 45), (80, 24)].into_iter().zip(baseline) {
        assert_eq!(
            draw(&mut app, width, height),
            before,
            "board buffer remains intact"
        );
    }
    // A different error for one file is a new message, not a permanent suppression.
    std::fs::write(&broken, "---\nid: [\n---\n").unwrap();
    app.reload();
    app.drain_messages();
    assert_eq!(app.messages.len(), 3);
    assert_eq!(app.toasts.len(), 1);
}
