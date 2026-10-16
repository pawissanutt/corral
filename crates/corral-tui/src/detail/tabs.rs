//! Wrapped detail tabs keep every tab clickable on smaller terminals.
use ratatui::crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::Span,
};
use unicode_width::UnicodeWidthStr;

use crate::{
    HitMap,
    hit::{self, Hit},
    theme,
};

fn labels(width: u16, names: &[&str]) -> impl Iterator<Item = (usize, String, u16, u16)> {
    let (mut x, mut y) = (0_u16, 0_u16);
    names.iter().enumerate().map(move |(i, name)| {
        let label = format!(" {} {name} ", i + 1);
        let w = u16::try_from(label.width()).unwrap_or(width).min(width);
        if x > 0 && x.saturating_add(w) > width {
            x = 0;
            y = y.saturating_add(1);
        }
        let result = (i, label, x, y);
        x = x.saturating_add(w).saturating_add(1);
        result
    })
}

pub(super) fn height(width: u16, names: &[&str]) -> u16 {
    labels(width, names).last().map_or(1, |(_, _, _, y)| y + 1)
}

pub(super) fn draw(f: &mut Frame, area: Rect, names: &[&str], selected: usize, hits: &mut HitMap) {
    for (i, label, x, y) in labels(area.width, names) {
        if y >= area.height {
            break;
        }
        let w = u16::try_from(label.width())
            .unwrap_or(area.width)
            .min(area.width.saturating_sub(x));
        let rect = Rect::new(area.x + x, area.y + y, w, 1);
        let style = if i == selected {
            Style::new()
                .fg(theme::CYAN)
                .add_modifier(Modifier::UNDERLINED | Modifier::BOLD)
        } else {
            theme::dim()
        };
        f.render_widget(Span::styled(label, style), rect);
        if let Ok(number) = u32::try_from(i + 1)
            && let Some(c) = char::from_digit(number, 10)
        {
            hits.push(rect, Hit::Key(hit::press(KeyCode::Char(c))));
        }
    }
}
