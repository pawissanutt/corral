//! Unsaved-settings confirmation, kept separate to respect the module ceiling.

use corral_ops::Ctx;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use super::SettingsView;
use crate::Action;
use crate::hit::{self, HitMap};
use crate::overlays::{self, centered};

impl SettingsView {
    pub(super) fn confirm_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        match key.code {
            KeyCode::Char('y') => Action::Back,
            KeyCode::Char('s') => {
                self.confirm = false;
                match self.save(ctx) {
                    Ok(t) => Action::BackWithToast(t),
                    Err(t) => Action::Toast(t),
                }
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                self.confirm = false;
                Action::None
            }
            _ => Action::None,
        }
    }

    pub(super) fn draw_confirm(&self, f: &mut Frame, area: Rect, hits: &mut HitMap) {
        overlays::dim_backdrop(f.buffer_mut(), area);
        let rect = centered(area, 60, 6);
        let inner = overlays::popup(f, rect, "unsaved changes", hits);
        if inner.height < 3 {
            return;
        }
        f.render_widget(
            Paragraph::new(Line::raw("Your settings changes are not saved yet.")),
            Rect { height: 1, ..inner },
        );
        let buttons = [
            ("y", "discard"),
            ("s", "save and close"),
            ("n", "keep editing"),
        ];
        let y = inner.y + 2;
        hit::hint_hits(hits, &buttons, inner.x, y, inner.right());
        f.render_widget(
            Paragraph::new(Line::from(overlays::hint_spans(&buttons))),
            Rect {
                y,
                height: 1,
                ..inner
            },
        );
    }
}
