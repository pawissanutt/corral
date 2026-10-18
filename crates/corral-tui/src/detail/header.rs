use ratatui::{
    Frame,
    crossterm::event::KeyCode,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};
use unicode_width::UnicodeWidthStr;

use crate::{
    HitMap,
    hit::{self, Hit},
    theme,
};

const BACK: &str = "‹ back";

pub(super) fn draw(
    f: &mut Frame,
    area: Rect,
    task: Line<'static>,
    parent: Option<&str>,
    hits: &mut HitMap,
) {
    let mut spans = vec![
        Span::styled(BACK, Style::new().fg(theme::CYAN)),
        Span::raw("  "),
    ];
    if let Some(parent) = parent {
        let label = format!("parent {parent} [P]  ");
        let offset = u16::try_from(BACK.width() + 2)
            .unwrap_or(area.width)
            .min(area.width);
        let width = u16::try_from(label.width())
            .unwrap_or(area.width)
            .min(area.width.saturating_sub(offset));
        hits.push(
            Rect::new(area.x + offset, area.y, width, 1),
            Hit::Key(hit::press(KeyCode::Char('P'))),
        );
        spans.push(Span::styled(label, Style::new().fg(theme::CYAN)));
    }
    spans.extend(task.spans);
    f.render_widget(Paragraph::new(Line::from(spans)), area);
    hits.push(
        Rect {
            width: u16::try_from(BACK.width())
                .unwrap_or(area.width)
                .min(area.width),
            ..area
        },
        Hit::Key(hit::press(KeyCode::Esc)),
    );
}
