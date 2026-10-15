//! Noninteractive QA capture of the real board/detail from an isolated CLI-created workspace.
//! Usage: capture_obligations ROOT OUTPUT_PREFIX COLS [TASK_ID]
use std::fmt::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use corral_ops::Ctx;
use corral_tui::{HitMap, View, board::BoardView, detail::DetailView};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Modifier};
use unicode_width::UnicodeWidthStr;

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let root = PathBuf::from(args.get(1).context("missing ROOT")?);
    let output = PathBuf::from(args.get(2).context("missing OUTPUT_PREFIX")?);
    let cols: u16 = args
        .get(3)
        .context("missing COLS")?
        .parse()
        .context("parsing COLS")?;
    let ctx = Ctx::open(&root)?;
    let mut view: Box<dyn View> = if let Some(id) = args.get(4) {
        let mut detail = DetailView::new(&ctx, id)?;
        detail.on_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE), &ctx);
        Box::new(detail)
    } else {
        let mut board = BoardView::new(&ctx)?;
        board.on_key(KeyEvent::new(KeyCode::Char('D'), KeyModifiers::NONE), &ctx);
        Box::new(board)
    };
    let mut terminal =
        Terminal::new(TestBackend::new(cols, 45)).context("creating capture terminal")?;
    terminal
        .draw(|f| view.draw(f, f.area(), &ctx, &mut HitMap::default()))
        .context("drawing capture")?;
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
            if cell.modifier.contains(Modifier::ITALIC) {
                ansi.push_str("\x1b[3m");
            }
            ansi.push_str(symbol);
        }
        text.push('\n');
        ansi.push_str("\x1b[0m\r\n");
    }
    std::fs::write(output.with_extension("txt"), text).context("writing terminal grid")?;
    std::fs::write(output.with_extension("ansi"), ansi).context("writing terminal ANSI")?;
    Ok(())
}
