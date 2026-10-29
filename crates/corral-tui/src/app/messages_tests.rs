#[cfg(unix)]
use std::sync::mpsc;

#[cfg(unix)]
use corral_herdr::Herdr;
use corral_ops::{Message, capture_messages};
use ratatui::{Terminal, backend::TestBackend};

use super::App;
#[cfg(unix)]
use super::WorkerMsg;
use crate::Mode;
#[cfg(unix)]
use crate::board::tests::key;
use crate::board::tests::{buffer_text, seeded};

fn draw(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| app.draw(frame)).unwrap();
    buffer_text(terminal.backend().buffer())
}

#[test]
#[cfg(unix)]
fn board_captures_sync_failures_once_without_raw_terminal_output() {
    // Given a real sync with admitted Ready work whose route cannot be resolved.
    let mut fx = seeded();
    // A checked-in executable cannot inherit a writable fd from a parallel fixture setup.
    let bin = fx.ctx.store.root().join("herdr-empty");
    std::os::unix::fs::symlink(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app/herdr-empty.sh"),
        &bin,
    )
    .unwrap();
    fx.ctx.herdr = Herdr::with_bin(bin, Some("corral-test".into()));
    let root = fx.ctx.store.root().to_str().unwrap().to_string();
    crate::board::card_tests::configure_project(&mut fx, "mitigraph", &root);
    fx.ctx
        .store
        .update("T-003", |task| {
            task.profile = Some("missing-profile".into());
            Ok(())
        })
        .unwrap();
    fx.grant(&["T-003"]);
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
        app.on_worker(WorkerMsg::synced(
            &app.ctx,
            completed_rx
                // Sync completion is the signal, not how fast its subprocesses run.
                .recv_timeout(std::time::Duration::from_secs(120))
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
    // The shared operating header now inspects review evidence. Keep this
    // unrelated synthetic review held so only the disappearance below fails.
    fx.ctx
        .store
        .update("T-006", |task| {
            task.dispatch_hold = true;
            Ok(())
        })
        .unwrap();
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
    fn capture(
        app: &mut App,
        width: u16,
        height: u16,
    ) -> (String, std::collections::BTreeMap<String, Vec<String>>) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let cards = app
            .hits
            .regions()
            .iter()
            .filter_map(|(rect, hit)| {
                let crate::Hit::Card(id) = hit else {
                    return None;
                };
                let mut card = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(
                    0,
                    0,
                    rect.width,
                    rect.height,
                ));
                for y in 0..rect.height {
                    for x in 0..rect.width {
                        card[(x, y)] = buffer[(rect.x + x, rect.y + y)].clone();
                    }
                }
                Some((
                    id.clone(),
                    buffer_text(&card).lines().map(str::to_owned).collect(),
                ))
            })
            .collect();
        (buffer_text(buffer), cards)
    }

    // Given a board and two task files that become malformed while it owns the terminal.
    let fx = seeded();
    // Keep unavailable-projection column membership stable and avoid the synthetic worktree.
    fx.ctx
        .store
        .update("T-006", |task| {
            task.status = corral_core::Status::NeedsMe;
            task.dispatch_hold = true;
            Ok(())
        })
        .unwrap();
    let attention = crate::attention::load(&fx.ctx, fx.now.get()).unwrap();
    let mut app = fx.app(Mode::Board);
    // Keep complete cards visible for cell equality; the small viewport is checked below.
    let baseline: Vec<_> = [(160, 45), (80, 45)]
        .into_iter()
        .map(|(width, height)| capture(&mut app, width, height))
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
        assert!(text.contains("attention unavailable:"), "{text}");
        assert!(!text.contains("overdue 0"), "{text}");
        assert!(!text.contains("No waits."), "{text}");
    }
    app.toasts.clear();
    app.reload();
    app.drain_messages();
    assert!(
        app.toasts.is_empty(),
        "unchanged files do not re-toast after dismissal"
    );
    for ((width, height), (before, mut expected_cards)) in
        [(160, 45), (80, 45)].into_iter().zip(baseline)
    {
        let (after, cards) = capture(&mut app, width, height);
        // A failed projection deliberately replaces these two cards' attention rows
        // with the unavailable header. Every other cell in every card must survive.
        for id in ["T-005", "T-006"] {
            let task = fx.ctx.store.get(id).unwrap();
            let condition =
                corral_core::attention::blocked(&task, attention.get(id).unwrap()).unwrap();
            let rows = expected_cards.get_mut(id).unwrap();
            let width = unicode_width::UnicodeWidthStr::width(rows[0].as_str()).saturating_sub(2);
            let count = crate::attention::blocked_lines(condition, width, fx.now.get()).len();
            let removed = rows.drain(2..2 + count).collect::<Vec<_>>();
            let row = removed.last().unwrap();
            assert!(
                row.contains("By Owner"),
                "expected attention row for {id}: {row}"
            );
        }
        assert_eq!(cards, expected_cards, "unaffected cards remain intact");
        let before: Vec<_> = before.lines().collect();
        let after: Vec<_> = after.lines().collect();
        assert_eq!(&after[..4], &before[..4], "app header remains intact");
        assert_eq!(
            &after[after.len() - 2..],
            &before[before.len() - 2..],
            "footer remains intact"
        );
        for border in ['╭', '╰'] {
            assert_eq!(
                after.iter().find(|line| line.starts_with(border)),
                before.iter().find(|line| line.starts_with(border)),
                "column border remains intact"
            );
        }
    }
    // A different error for one file is a new message, not a permanent suppression.
    std::fs::write(&broken, "---\nid: [\n---\n").unwrap();
    app.reload();
    app.drain_messages();
    assert_eq!(app.messages.len(), 3);
    assert_eq!(app.toasts.len(), 1);
}
