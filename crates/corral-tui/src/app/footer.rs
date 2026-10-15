//! The footer: the top view's hints wrapped into rows (every hint stays visible and clickable) and, on the board
//! itself, the back-to-board key at the right end of the last row, or on its own row above the hints when it does
//! not fit. That label names a terminal-engine key, so it is not a click target.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use unicode_width::UnicodeWidthStr;

use crate::hit::{self, HitMap};
use crate::nav::NavKeys;
use crate::{overlays, theme};

const BRAND: &str = " corral  ";

type Hint = (&'static str, &'static str);

pub(super) struct Footer<'a> {
    rows: Vec<&'a [Hint]>,
    back: Option<Line<'static>>,
    back_row: bool,
}

impl<'a> Footer<'a> {
    /// Lays `hints` out in rows of `width` columns; `back` is the back-to-board label when it is shown.
    pub(super) fn new(hints: &'a [Hint], width: u16, back: Option<Line<'static>>) -> Footer<'a> {
        let brand = BRAND.width() as u16;
        let mut rows = Vec::new();
        let mut start = 0;
        let mut used = brand;
        for (i, (key, label)) in hints.iter().enumerate() {
            let w = u16::try_from(format!("{key} {label}  ").width()).unwrap_or(u16::MAX);
            if i > start && used.saturating_add(w) > width {
                rows.push(&hints[start..i]);
                start = i;
                used = brand;
            }
            used = used.saturating_add(w);
        }
        rows.push(&hints[start..]);
        let back_w = back.as_ref().map_or(0, |line| line.width() as u16);
        let back_row = back.is_some() && used.saturating_add(back_w) > width;
        Footer {
            rows,
            back,
            back_row,
        }
    }

    pub(super) fn height(&self) -> u16 {
        u16::try_from(self.rows.len() + usize::from(self.back_row)).unwrap_or(u16::MAX)
    }

    pub(super) fn draw(self, f: &mut Frame, footer: Rect, hits: &mut HitMap) {
        let brand = BRAND.width() as u16;
        for (dy, hints) in self
            .rows
            .iter()
            .take(usize::from(footer.height))
            .enumerate()
        {
            let y = footer.y + u16::from(self.back_row) + dy as u16;
            let row = Rect::new(footer.x, y, footer.width, 1);
            hit::hint_hits(hits, hints, row.x + brand, row.y, row.right());
            let prefix = if dy == 0 {
                BRAND.to_string()
            } else {
                " ".repeat(usize::from(brand))
            };
            let mut spans = vec![Span::styled(prefix, theme::dim())];
            spans.extend(overlays::hint_spans(hints));
            f.render_widget(
                Paragraph::new(Line::from(spans)).style(Style::new().bg(theme::BG2)),
                row,
            );
        }
        if let Some(line) = self.back {
            let w = (line.width() as u16).min(footer.width);
            let y = if self.back_row {
                footer.y
            } else {
                footer.bottom().saturating_sub(1)
            };
            if self.back_row {
                f.render_widget(
                    Block::new().style(Style::new().bg(theme::BG2)),
                    Rect {
                        y,
                        height: 1,
                        ..footer
                    },
                );
            }
            f.render_widget(
                Paragraph::new(line).style(Style::new().bg(theme::BG2)),
                Rect::new(footer.right() - w, y, w, 1),
            );
        }
    }
}

/// The board's back-to-board label ("Ctrl-b k back to board"), so the key is learned before it is needed.
pub(super) fn back_label(nav: &NavKeys) -> Line<'static> {
    let label = match nav.board {
        Some(_) => " back to board ",
        None => " reopens the board ",
    };
    Line::from(vec![
        Span::styled(nav.board_short(), Style::new().fg(theme::CYAN)),
        Span::styled(label, theme::dim()),
    ])
}
