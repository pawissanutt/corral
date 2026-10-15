use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use corral_core::{DiskSpace, ResourceSnapshot, ResourceState, Store};
use corral_ops::resources::{FixedResourceProbe, ResourceProbe};
use jiff::Timestamp;
use ratatui::{Terminal, backend::TestBackend, widgets::Paragraph};

use super::ResourceDisplay;
use crate::{
    Mode, View,
    board::tests::{buffer_text, seeded},
    theme,
};

fn snapshot(load: f64) -> ResourceSnapshot {
    ResourceSnapshot {
        load_per_core: load,
        load_one: load * 8.0,
        cores: 8,
        mem_available: 16 * 1_073_741_824,
        cpu_psi: Some(1.0),
        mem_psi: None,
        disks: vec![DiskSpace {
            path: "/workspace".into(),
            free_bytes: 100 * 1_073_741_824,
        }],
    }
}

#[derive(Debug)]
struct CountingProbe(Arc<AtomicUsize>);

impl ResourceProbe for CountingProbe {
    fn sample(&self, _: &Store) -> anyhow::Result<ResourceSnapshot> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(snapshot(0.2))
    }
}

#[test]
fn resource_gauges_render_at_compact_and_large_widths() {
    // Given a host-independent sample.
    let mut fx = seeded();
    fx.ctx.resource_probe = Arc::new(FixedResourceProbe(snapshot(0.2)));
    let mut app = fx.app(Mode::Board);
    for width in [80, 160] {
        // When the real application renders at each supported width.
        let mut terminal = Terminal::new(TestBackend::new(width, 32)).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        // Then all three gauges survive the header layout.
        let text = buffer_text(terminal.backend().buffer());
        let row = text.lines().nth(1).unwrap();
        assert!(
            row.contains("load 0.20/core")
                && row.contains("mem 16GiB")
                && row.contains("disk 100GiB"),
            "{text}"
        );
        capture(&format!("healthy-{width}"), terminal.backend().buffer());
    }
}

#[test]
fn resource_redraw_uses_cache_until_board_refresh() {
    // Given a counting probe and an already loaded board.
    let mut fx = seeded();
    let calls = Arc::new(AtomicUsize::new(0));
    fx.ctx.resource_probe = Arc::new(CountingProbe(calls.clone()));
    let mut board = fx.board();
    let before = calls.load(Ordering::SeqCst);
    let mut terminal = Terminal::new(TestBackend::new(160, 32)).unwrap();
    // When drawing repeatedly, then refreshing.
    for _ in 0..3 {
        terminal
            .draw(|f| board.draw(f, f.area(), &fx.ctx, &mut Default::default()))
            .unwrap();
    }
    assert_eq!(calls.load(Ordering::SeqCst), before);
    board.refresh(&fx.ctx);
    // Then exactly one new observation was made by refresh, not redraw.
    assert_eq!(calls.load(Ordering::SeqCst), before + 1);
}

#[test]
fn resource_held_review_without_stage_and_queue_show_numeric_reason() {
    // Given both a queue entry and a Review task with no autopilot stage.
    let mut fx = seeded();
    fx.ctx.resource_probe = Arc::new(FixedResourceProbe(snapshot(2.0)));
    fx.ctx
        .store
        .update("T-003", |task| {
            task.queued = true;
            Ok(())
        })
        .unwrap();
    let mut app = fx.app(Mode::Board);
    for width in [80, 160] {
        // When rendering under a resource hold.
        let mut terminal = Terminal::new(TestBackend::new(width, 40)).unwrap();
        terminal.draw(|f| app.draw(f)).unwrap();
        let text = buffer_text(terminal.backend().buffer());
        // Then both cards show the waiting label and the sampled numbers.
        assert_eq!(text.matches("waiting:").count(), 2, "{text}");
        assert!(text.contains("2.00") && text.contains("held"), "{text}");
        capture(&format!("held-{width}"), terminal.backend().buffer());
    }
}

#[test]
fn resource_hold_does_not_label_an_already_running_review_as_waiting() {
    // Given a manual review in flight, without an autopilot stage.
    let fx = seeded();
    let mut task = fx.ctx.store.get("T-006").unwrap();
    let run = task.current_run_mut().unwrap();
    run.review = Some(
        serde_json::from_value(serde_json::json!({
            "reviewer": "owner",
            "sha": run.base_sha,
            "at": run.started,
            "pane": "test-pane"
        }))
        .unwrap(),
    );
    // When machine pressure is displayed for that task.
    let hold = super::hold(&task, &fx.ctx.store.config, "load/core 2.00, max 1.00");
    // Then the running review is not presented as held.
    assert!(hold.is_none());
}

#[test]
fn severe_pressure_shows_unqueued_ready_and_review_work_without_running_tasks() {
    for disk_pressure in [false, true] {
        // Given authorized Ready and pending Review work, but no Running task or prior enqueue.
        let mut fx = seeded();
        let ready = fx.ctx.store.get("T-003").unwrap();
        fx.ctx.store.config.dispatch.ready_projects = vec![ready.project.unwrap()];
        for task in fx.ctx.store.list().unwrap() {
            fx.ctx
                .store
                .update(&task.id, |current| {
                    if !matches!(current.id.as_str(), "T-003" | "T-006") {
                        current.status = corral_core::Status::Dropped;
                    }
                    current.queued = false;
                    Ok(())
                })
                .unwrap();
        }
        let mut sample = snapshot(0.2);
        if disk_pressure {
            sample.disks[0].free_bytes = 1;
        } else {
            sample.mem_available = 1;
        }
        fx.ctx.resource_probe = Arc::new(FixedResourceProbe(sample));
        let mut app = fx.app(Mode::Board);
        for width in [80, 160] {
            // When the real application renders severe host pressure.
            let mut terminal = Terminal::new(TestBackend::new(width, 40)).unwrap();
            terminal.draw(|f| app.draw(f)).unwrap();
            let text = buffer_text(terminal.backend().buffer());
            // Then both admission identities retain a visible per-task waiting explanation.
            assert_eq!(text.matches("waiting:").count(), 2, "{text}");
            assert!(text.lines().nth(1).unwrap().contains("needs you"), "{text}");
            assert_eq!(
                fx.ctx.store.get("T-003").unwrap().status,
                corral_core::Status::Ready
            );
            assert_eq!(
                fx.ctx.store.get("T-006").unwrap().status,
                corral_core::Status::Review
            );
            capture(
                &format!(
                    "severe-{}-{width}",
                    if disk_pressure { "disk" } else { "memory" }
                ),
                terminal.backend().buffer(),
            );
        }
    }
}

#[test]
fn ready_waiting_label_requires_project_authorization_and_no_owner_hold() {
    // Given unqueued Ready work outside the project's admission allowlist.
    let fx = seeded();
    let mut task = fx.ctx.store.get("T-003").unwrap();
    task.queued = false;
    let mut config = fx.ctx.store.config.clone();
    config.dispatch.ready_projects.clear();
    // When the shared board/autopilot status formatter checks its admission identity.
    assert!(super::hold(&task, &config, "memory available 1 bytes").is_none());
    config.dispatch.ready_projects = vec![task.project.clone().unwrap()];
    assert!(super::hold(&task, &config, "memory available 1 bytes").is_some());
    task.dispatch_hold = true;
    // Then explicit dequeue remains an owner hold, not resource-authorized work.
    assert!(super::hold(&task, &config, "memory available 1 bytes").is_none());
    task.dispatch_hold = false;
    task.status = corral_core::Status::NeedsMe;
    assert!(super::hold(&task, &config, "memory available 1 bytes").is_none());
}

#[test]
fn resource_gauges_color_near_and_exceeded_thresholds() {
    // Given configurable thresholds and values near their limits.
    let fx = seeded();
    let mut sample = snapshot(0.9);
    sample.mem_available = 8_500_000_000;
    sample.disks[0].free_bytes = 51_000_000_000;
    let state = ResourceState {
        snapshot: sample,
        reason: Some("load/core 0.90, max 1.00".into()),
        severe: false,
        held: true,
        observed_at: Timestamp::UNIX_EPOCH,
    };
    let display = ResourceDisplay::from_sample(Ok(state), &fx.ctx.store.config);
    // When rendering the gauge's semantic colors.
    let colors: Vec<_> = display
        .line()
        .spans
        .iter()
        .filter(|span| !span.content.trim().is_empty())
        .filter_map(|span| span.style.fg)
        .collect();
    // Then near-limit values warn and the hold has its own non-color label.
    assert!(colors.iter().all(|color| *color == theme::YELLOW));
    let display = ResourceDisplay::from_sample(
        Ok(ResourceState {
            snapshot: snapshot(2.0),
            reason: None,
            severe: false,
            held: false,
            observed_at: Timestamp::UNIX_EPOCH,
        }),
        &fx.ctx.store.config,
    );
    assert_eq!(display.line().spans[1].style.fg, Some(theme::RED));
}

#[test]
fn resource_probe_error_keeps_the_board_visible() {
    // Given an unavailable resource observation.
    let fx = seeded();
    let display = ResourceDisplay::from_sample(
        Err(anyhow::anyhow!("fixture probe unavailable")),
        &fx.ctx.store.config,
    );
    // When rendering its diagnostic in a terminal.
    let mut terminal = Terminal::new(TestBackend::new(80, 1)).unwrap();
    terminal
        .draw(|f| f.render_widget(Paragraph::new(display.line()), f.area()))
        .unwrap();
    let text = buffer_text(terminal.backend().buffer());
    // Then the unavailable cause is visible rather than failing the display.
    assert!(
        text.contains("resources unavailable: fixture probe unavailable"),
        "{text}"
    );
    capture("unavailable-80", terminal.backend().buffer());
}

fn capture(name: &str, buffer: &ratatui::buffer::Buffer) {
    let Some(dir) = std::env::var_os("CORRAL_RESOURCE_EVIDENCE") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{name}.txt")), buffer_text(buffer)).unwrap();
    let mut ansi = String::from("\x1b[2J\x1b[H");
    for y in 0..buffer.area.height {
        let mut skip = 0;
        for x in 0..buffer.area.width {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let cell = &buffer[(x, y)];
            skip = unicode_width::UnicodeWidthStr::width(cell.symbol()).saturating_sub(1);
            for (color, code) in [(cell.fg, 38), (cell.bg, 48)] {
                match color {
                    ratatui::style::Color::Rgb(r, g, b) => {
                        ansi.push_str(&format!("\x1b[{code};2;{r};{g};{b}m"))
                    }
                    _ => ansi.push_str(if code == 38 { "\x1b[39m" } else { "\x1b[49m" }),
                }
            }
            ansi.push_str(cell.symbol());
        }
        ansi.push_str("\x1b[0m\r\n");
    }
    std::fs::write(dir.join(format!("{name}.ansi")), ansi).unwrap();
}
