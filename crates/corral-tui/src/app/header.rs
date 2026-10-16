//! Board counters and the always-visible search entry point.

use corral_core::Status;
use ratatui::Frame;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::App;
use crate::hit::{self, Hit};
use crate::overlays::truncate;
use crate::theme;

impl App {
    /// Each counter focuses its board column; the switches press their ordinary board keys.
    pub(super) fn draw_header(&self, f: &mut Frame, area: Rect) -> Vec<(Rect, Hit)> {
        let search = Rect::new(
            area.right().saturating_sub(area.width.min(10)),
            area.y,
            area.width.min(10),
            area.height,
        );
        let area = Rect {
            width: area.width.saturating_sub(search.width),
            ..area
        };
        let c = self.counts;
        let root = self.ctx.store.root();
        let name = root.file_name().map_or_else(
            || root.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let narrow = area.width < 140;
        let name = if narrow { truncate(&name, 10) } else { name };
        let gap = || Span::raw(if narrow { " " } else { "   " });
        let slot_text = if c.reviews == 0 && c.gates == 0 && c.reservations == 0 {
            format!("⟳ {}/{} slots", c.used, c.limit)
        } else if narrow {
            format!(
                "⟳ {}/{} t{} r{} g{} q{}",
                c.used, c.limit, c.tasks, c.reviews, c.gates, c.reservations
            )
        } else {
            format!(
                "⟳ {}/{} slots ({} tasks · {} reviews · {} gates{})",
                c.used,
                c.limit,
                c.tasks,
                c.reviews,
                c.gates,
                if c.reservations > 0 {
                    format!(" · {} reserved", c.reservations)
                } else {
                    String::new()
                }
            )
        };
        let slots_color = if c.used >= c.limit {
            theme::ORANGE
        } else {
            theme::BLUE
        };
        let mut spans = vec![
            Span::styled(
                " corral",
                Style::new().fg(theme::CYAN).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · ", theme::dim()),
            Span::styled(name, Style::new().fg(theme::FG)),
            gap(),
            Span::styled(
                if narrow {
                    format!("▲ {}", c.needs)
                } else {
                    format!("▲ {} needs you", c.needs)
                },
                Style::new().fg(theme::YELLOW),
            ),
            gap(),
            Span::styled(slot_text, Style::new().fg(slots_color)),
            gap(),
            Span::styled(
                if narrow {
                    format!("◆ {}", c.review)
                } else {
                    format!("◆ {} review", c.review)
                },
                Style::new().fg(theme::GREEN),
            ),
            gap(),
            Span::styled(
                if narrow {
                    format!("⏳ {}", c.ready)
                } else {
                    format!("⏳ {} queued", c.ready)
                },
                theme::dim(),
            ),
        ];
        if !self.herdr_online {
            spans.push(gap());
            spans.push(Span::styled(
                "herdr offline",
                Style::new().fg(theme::RED).add_modifier(Modifier::BOLD),
            ));
        }
        spans.push(gap());
        let switch = spans.len();
        spans.push(if c.autopilot_off {
            Span::styled("⏸ autopilot off", Style::new().fg(theme::YELLOW))
        } else {
            Span::styled("● autopilot on", Style::new().fg(theme::GREEN))
        });
        let mut targets = Vec::new();
        let mut x = area.x;
        for (i, s) in spans.iter().enumerate() {
            let w = s.width() as u16;
            let hit = match i {
                4 => Some(Hit::Column(Status::NeedsMe)),
                6 => Some(Hit::Column(Status::Running)),
                8 => Some(Hit::Column(Status::Review)),
                10 => Some(Hit::Column(Status::Ready)),
                i if i == switch => Some(Hit::Key(hit::press(KeyCode::Char('O')))),
                _ => None,
            };
            if let Some(hit) = hit {
                let w = w.min(area.right().saturating_sub(x));
                targets.push((Rect::new(x, area.y, w, 1), hit));
            }
            x = x.saturating_add(w);
        }
        f.render_widget(
            Paragraph::new(Line::from(spans)).style(Style::new().bg(theme::BG2)),
            area,
        );
        f.render_widget(
            Paragraph::new(" / search ").style(Style::new().fg(theme::CYAN).bg(theme::BG2)),
            search,
        );
        targets.push((search, Hit::Key(hit::press(KeyCode::Char('/')))));
        targets
    }
}
