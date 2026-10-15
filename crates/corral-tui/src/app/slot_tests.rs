use corral_core::{GateRecord, GateResult, ReviewRecord};
use ratatui::{Terminal, backend::TestBackend, style::Color};

use crate::{
    Mode,
    board::tests::{buffer_text, seeded},
};

#[test]
fn shared_slot_header_renders_at_wide_and_narrow_terminal_widths() {
    let mut fx = seeded();
    fx.ctx.store.config.limits.running = 7;
    fx.ctx
        .store
        .update("T-004", |t| {
            let run = t.current_run_mut().unwrap();
            run.gate = Some(GateRecord {
                identity: None,
                commands: vec![],
                log_sha256: None,
                result: GateResult::Running,
                process_lost: false,
                failed_cmd: None,
                at: run.started,
                sha: run.base_sha.clone(),
                pane: Some("gate".into()),
            });
            Ok(())
        })
        .unwrap();
    fx.ctx
        .store
        .update("T-006", |t| {
            let run = t.current_run_mut().unwrap();
            run.review = Some(ReviewRecord {
                identity: None,
                evidence_digest: None,
                verdict: None,
                reviewer: "generic".into(),
                notes: None,
                sha: run.base_sha.clone(),
                at: run.started,
                pane: Some("review".into()),
            });
            Ok(())
        })
        .unwrap();
    let mut app = fx.app(Mode::Board);
    assert_eq!(
        (
            app.counts.used,
            app.counts.tasks,
            app.counts.reviews,
            app.counts.gates
        ),
        (4, 2, 1, 1)
    );
    for (width, height) in [(160, 45), (80, 30)] {
        let mut term = Terminal::new(TestBackend::new(width, height)).unwrap();
        term.draw(|f| app.draw(f)).unwrap();
        let buffer = term.backend().buffer();
        let text = buffer_text(buffer);
        let header = text.lines().next().unwrap();
        assert!(header.contains("4/7"), "{header}");
        assert!(header.contains("autopilot"), "{header}");
        assert!(app.hits.iter().all(|(rect, _)| rect.right() <= width));

        // Optional evidence output from the real ratatui render, without terminal input.
        if let Some(dir) = std::env::var_os("CORRAL_SLOT_EVIDENCE") {
            let dir = std::path::PathBuf::from(dir);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(format!("board-{width}.txt")), &text).unwrap();
            let mut html = String::from(
                "<!doctype html><meta charset=\"utf-8\"><style>body{margin:0;background:#10131a}pre{margin:0;padding:12px;font:14px/18px 'DejaVu Sans Mono',monospace;white-space:pre}</style><pre>",
            );
            for y in 0..height {
                let mut skip = 0;
                for x in 0..width {
                    if skip > 0 {
                        skip -= 1;
                        continue;
                    }
                    let cell = &buffer[(x, y)];
                    let sym = cell.symbol();
                    skip = unicode_width::UnicodeWidthStr::width(sym).saturating_sub(1);
                    let fg = css(cell.fg, "#d0d5df");
                    let bg = css(cell.bg, "#10131a");
                    html.push_str(&format!(
                        "<span style=\"color:{fg};background:{bg}\">{}</span>",
                        sym.replace('&', "&amp;")
                            .replace('<', "&lt;")
                            .replace('>', "&gt;")
                    ));
                }
                html.push('\n');
            }
            html.push_str("</pre>");
            std::fs::write(dir.join(format!("board-{width}.html")), html).unwrap();
        }
    }
}

fn css(color: Color, fallback: &str) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        _ => fallback.into(),
    }
}
