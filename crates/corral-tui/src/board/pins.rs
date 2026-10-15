//! The board's side of the pinned shelf: `*` pins the selected card, `1`..`9` open pins, `g` focuses the strip,
//! `G` opens the shelf panel; chips open on click, carry a right-click menu, and the wheel scrolls the strip.

use corral_ops::Ctx;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use super::menu::CardMenu;
use super::{BoardView, Overlay};
use crate::shelf::{self, Z_CHIP, Z_NEXT, Z_PREV, Z_STRIP, panel::ShelfPanel};
use crate::{Action, Hit, MouseCtx, Toast, Tone, hit};

/// Footer hints while the shelf strip has focus.
pub(super) const SHELF_HINTS: [(&str, &str); 6] = [
    ("←→", "chip"),
    ("⏎", "open"),
    ("H/L", "move"),
    ("*", "unpin"),
    ("G", "shelf"),
    ("esc", "board"),
];

impl BoardView {
    /// Board keys `*`, `g`, `G` and `1`..`9`.
    pub(super) fn pin_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        match key.code {
            KeyCode::Char('*') => match self.selected_task() {
                Some(t) => {
                    let id = t.id.clone();
                    shelf::toggle(ctx, &id)
                }
                None => Action::Toast(Toast::new(
                    Tone::Info,
                    "Nothing to pin",
                    "* pins the selected card",
                )),
            },
            KeyCode::Char('g') if self.pins.is_empty() => Action::Toast(Toast::new(
                Tone::Info,
                "Nothing pinned",
                "* pins the selected card or artifact",
            )),
            KeyCode::Char('g') => {
                let first = self.shelf_offset.min(self.pins.len() - 1);
                self.shelf_cursor = Some(first);
                Action::None
            }
            KeyCode::Char('G') => {
                let sel = self.shelf_cursor.take().unwrap_or(0);
                self.overlay = Some(Overlay::Shelf(ShelfPanel::new(ctx, sel)));
                Action::None
            }
            KeyCode::Char(c @ '1'..='9') => {
                let i = usize::from(c as u8 - b'1');
                if i < self.pins.len() {
                    Action::OpenPin(i)
                } else {
                    Action::Toast(Toast::new(
                        Tone::Info,
                        format!("No pin #{}", i + 1),
                        format!("{} pinned; * pins the selected card", self.pins.len()),
                    ))
                }
            }
            _ => Action::None,
        }
    }

    /// Keys while the strip has focus: ←→/h l pick a chip, ⏎ or g opens it, H/L move it, * unpins it, G opens the
    /// panel on it, esc/j/↓ leave. Any other key leaves the strip and acts on the board.
    pub(super) fn shelf_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        let Some(c) = self.shelf_cursor else {
            return self.board_key(key, ctx);
        };
        let last = self.pins.len().saturating_sub(1);
        match key.code {
            KeyCode::Left | KeyCode::Char('h') => self.shelf_cursor = Some(c.saturating_sub(1)),
            KeyCode::Right | KeyCode::Char('l') => self.shelf_cursor = Some((c + 1).min(last)),
            KeyCode::Enter | KeyCode::Char('g') => return Action::OpenPin(c),
            KeyCode::Char(d @ '1'..='9') => {
                return self.pin_key(KeyEvent::new(KeyCode::Char(d), key.modifiers), ctx);
            }
            KeyCode::Char('*') | KeyCode::Char('x') => {
                return match self.pins.get(c) {
                    Some(e) => {
                        let target = e.pin.target.clone();
                        shelf::toggle(ctx, &target)
                    }
                    None => Action::None,
                };
            }
            KeyCode::Char('H') | KeyCode::Char('L') => {
                let to = if key.code == KeyCode::Char('H') {
                    c.checked_sub(1)
                } else {
                    Some(c + 1).filter(|t| *t <= last)
                };
                let Some(to) = to else {
                    return Action::None;
                };
                return match ctx.store.move_pin(c, to) {
                    Ok(at) => {
                        self.shelf_cursor = Some(at);
                        Action::Reload
                    }
                    Err(e) => Action::Toast(Toast::new(
                        Tone::Bad,
                        "Could not move the pin",
                        format!("{e:#}"),
                    )),
                };
            }
            KeyCode::Char('G') => return self.pin_key(key, ctx),
            KeyCode::Esc | KeyCode::Char('j') | KeyCode::Down => self.shelf_cursor = None,
            _ => {
                self.shelf_cursor = None;
                return self.board_key(key, ctx);
            }
        }
        Action::None
    }

    /// Mouse on the strip: a chip click opens it, a right-click opens its menu, `‹`/`›` and the wheel scroll the
    /// strip. A left press anywhere else takes focus off the strip. None when the board should handle the event.
    pub(super) fn shelf_mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Option<Action> {
        let (zone, i) = match m.hit {
            Some(Hit::Item(z, i)) if (Z_STRIP..=Z_NEXT).contains(&z) => (z, i),
            _ => {
                if hit::left(&ev) {
                    self.shelf_cursor = None;
                }
                return None;
            }
        };
        let last = self.pins.len().saturating_sub(1);
        if let Some(dir) = hit::wheel(&ev) {
            self.shelf_cursor = None;
            self.shelf_offset = self.shelf_offset.saturating_add_signed(dir).min(last);
            return Some(Action::None);
        }
        match (ev.kind, zone) {
            (MouseEventKind::Down(MouseButton::Left), Z_CHIP) => {
                self.shelf_cursor = None;
                return Some(Action::OpenPin(i));
            }
            (MouseEventKind::Down(MouseButton::Right), Z_CHIP) => {
                let anchor = Rect::new(ev.column, ev.row, 30, 1);
                self.menu = Some(CardMenu::chip(i, Some(anchor)));
            }
            (MouseEventKind::Down(MouseButton::Left), Z_PREV) => {
                self.shelf_cursor = None;
                self.shelf_offset = self.shelf_offset.saturating_sub(1);
            }
            (MouseEventKind::Down(MouseButton::Left), Z_NEXT) => {
                self.shelf_cursor = None;
                self.shelf_offset = (self.shelf_offset + 1).min(last);
            }
            _ => {}
        }
        Some(Action::None)
    }
}
