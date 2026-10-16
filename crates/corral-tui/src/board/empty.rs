//! Empty board guidance and its canonical mouse actions.

use corral_core::UiCfg;
use ratatui::Frame;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::{HitMap, hit, overlays, theme};

pub(super) fn draw(f: &mut Frame, area: Rect, hits: &mut HitMap, ui: &UiCfg) {
    let lines = [
        "No tasks yet.".to_string(),
        format!(
            "Press {} to capture an idea or task, or run",
            ui.bound_key('n')
        ),
        "`corral new \"title\"` in any terminal.".to_string(),
    ];
    let w = 48.min(area.width);
    let rect = overlays::centered(area, w, lines.len() as u16 + 4);
    if rect.height < 4 || w < 20 {
        return;
    }
    ratatui::widgets::Widget::render(ratatui::widgets::Clear, rect, f.buffer_mut());
    let block = overlays::focused_block("start here").style(Style::new().bg(theme::BG2));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let text: Vec<Line> = lines
        .into_iter()
        .enumerate()
        .map(|(i, l)| match i {
            0 => Line::styled(l, theme::bold()),
            _ => Line::styled(l, theme::dim()),
        })
        .collect();
    f.render_widget(Paragraph::new(text), inner);
    let buttons = [
        ("New task", hit::press(KeyCode::Char('n'))),
        ("Help", hit::press(KeyCode::Char('?'))),
    ];
    let y = inner.bottom().saturating_sub(1);
    hit::buttons(f, hits, inner.x, y, inner.right(), &buttons);
}
