//! Comment-thread and composer captures from an isolated CLI-created workspace.
//! Usage: capture_comments ROOT OUTPUT_PREFIX COLS TASK_ID [thread|composer|end]
//! Writes text grids and truecolor ANSI streams using the capture_obligations pattern.
use std::{fmt::Write, path::PathBuf};

use anyhow::{Context, Result};
use corral_ops::Ctx;
use corral_tui::{HitMap, View, detail::DetailView};
use ratatui::{
    Terminal,
    backend::TestBackend,
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    style::{Color, Modifier},
};
use unicode_width::UnicodeWidthStr;

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let root = PathBuf::from(args.get(1).context("missing ROOT")?);
    let output = PathBuf::from(args.get(2).context("missing OUTPUT_PREFIX")?);
    let cols = args
        .get(3)
        .context("missing COLS")?
        .parse()
        .context("parsing COLS")?;
    let id = args.get(4).context("missing TASK_ID")?;
    let state = args.get(5).map(String::as_str).unwrap_or("thread");
    let compose = match state {
        "thread" | "end" => false,
        "composer" => true,
        state => anyhow::bail!("unknown capture state {state:?}; use thread, composer or end"),
    };
    let ctx = Ctx::open(&root)?;
    let mut view = DetailView::new(&ctx, id)?;
    view.on_key(KeyEvent::new(KeyCode::Char('6'), KeyModifiers::NONE), &ctx);
    if state == "end" {
        for _ in 0..100 {
            view.on_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE), &ctx);
        }
    }
    if compose {
        view.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE), &ctx);
        for c in "Keep the cache bounded.".chars() {
            view.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE), &ctx);
        }
    }
    let mut terminal = Terminal::new(TestBackend::new(cols, 30))?;
    terminal.draw(|f| view.draw(f, f.area(), &ctx, &mut HitMap::default()))?;
    let cursor = if compose {
        Some(
            terminal
                .get_cursor_position()
                .context("reading input cursor")?,
        )
    } else {
        None
    };
    let buffer = terminal.backend().buffer();
    let mut text = String::new();
    let mut ansi = String::from("\x1b[?25l\x1b[2J\x1b[H");
    for y in 0..buffer.area.height {
        let mut skip = 0;
        for x in 0..buffer.area.width {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let cell = &buffer[(x, y)];
            let symbol = cell.symbol();
            skip = symbol.width().saturating_sub(1);
            text.push_str(symbol);
            ansi.push_str("\x1b[0m");
            if let Color::Rgb(r, g, b) = cell.fg {
                write!(&mut ansi, "\x1b[38;2;{r};{g};{b}m")?;
            }
            if let Color::Rgb(r, g, b) = cell.bg {
                write!(&mut ansi, "\x1b[48;2;{r};{g};{b}m")?;
            }
            if cell.modifier.contains(Modifier::BOLD) {
                ansi.push_str("\x1b[1m");
            }
            if cell.modifier.contains(Modifier::UNDERLINED) {
                ansi.push_str("\x1b[4m");
            }
            ansi.push_str(symbol);
        }
        text.push('\n');
        ansi.push_str("\x1b[0m");
        if y + 1 < buffer.area.height {
            ansi.push_str("\r\n");
        }
    }
    if let Some(cursor) = cursor {
        write!(
            &mut ansi,
            "\x1b[{};{}H\x1b[?25h",
            cursor.y + 1,
            cursor.x + 1
        )?;
    }
    std::fs::write(output.with_extension("txt"), text).context("writing grid")?;
    std::fs::write(output.with_extension("ansi"), ansi).context("writing ANSI")?;
    Ok(())
}
