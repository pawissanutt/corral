use corral_core::Status;
use corral_ops::Ctx;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use super::BoardView;
use super::menu::{self, CardMenu, Pick};
use crate::{Action, Hit, MouseCtx};

/// A left press on a card; it becomes a drag once the pointer leaves that card.
pub(super) struct Drag {
    id: String,
    status: Status,
    /// The card was already selected when pressed, so releasing it without dragging opens it.
    was_selected: bool,
    pub(super) active: bool,
}

/// The board key that dropping a `from` card on column `to` stands for, when that drop is legal.
pub(super) fn drop_key(from: Status, to: Status) -> Option<KeyCode> {
    match (from, to) {
        (Status::Idea | Status::Backlog, Status::Ready) => Some(KeyCode::Char('L')),
        (Status::Ready, Status::Backlog) => Some(KeyCode::Char('H')),
        (Status::Idea | Status::Backlog | Status::Ready, Status::Running) => {
            Some(KeyCode::Char('d'))
        }
        (Status::Review, Status::Done) => Some(KeyCode::Enter),
        _ => None,
    }
}

fn plain(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

impl BoardView {
    pub(super) fn mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        if let Some(ov) = &mut self.overlay {
            let action = ov.view().on_mouse(ev, m);
            return self.overlay_outcome(action, m.ctx);
        }
        if let Some(menu) = &self.menu {
            return match (ev.kind, &m.hit) {
                (MouseEventKind::Down(MouseButton::Left), Some(Hit::Item(menu::ZONE, i))) => {
                    let pick = menu.choose(*i);
                    self.menu_pick(pick, m.ctx)
                }
                _ => Action::None,
            };
        }
        if self.reply.is_some() {
            return Action::None;
        }
        if let Some(action) = self.shelf_mouse(ev, m) {
            return action;
        }
        match ev.kind {
            MouseEventKind::Down(MouseButton::Left) => self.press(m),
            MouseEventKind::Drag(MouseButton::Left) => {
                self.drag_over(m);
                Action::None
            }
            MouseEventKind::Up(MouseButton::Left) => self.release(ev, m.ctx),
            MouseEventKind::Down(MouseButton::Right) => {
                if let Some(Hit::Card(id) | Hit::CardMenu(id)) = &m.hit {
                    self.select_id(id);
                    self.open_menu();
                }
                Action::None
            }
            MouseEventKind::ScrollDown => self.scroll(ev, true),
            MouseEventKind::ScrollUp => self.scroll(ev, false),
            _ => Action::None,
        }
    }

    /// Applies a menu outcome; choosing an item presses its key on the board with the menu's card selected.
    pub(super) fn menu_pick(&mut self, pick: Pick, ctx: &Ctx) -> Action {
        match pick {
            Pick::Stay => Action::None,
            Pick::Close => {
                self.menu = None;
                Action::None
            }
            Pick::Choose(key) => {
                let chip = self.menu.take().and_then(|menu| match menu.chip {
                    Some(i) => Some(i),
                    None => {
                        self.select_id(&menu.task_id);
                        None
                    }
                });
                match chip {
                    Some(i) => {
                        self.shelf_cursor = Some(i);
                        self.shelf_key(key, ctx)
                    }
                    None => self.board_key(key, ctx),
                }
            }
        }
    }

    pub(super) fn open_menu(&mut self) {
        let anchor = |id: &str| {
            self.card_rects
                .iter()
                .find(|(c, _)| c == id)
                .map(|(_, r)| *r)
        };
        self.menu = self
            .selected_task()
            .map(|t| CardMenu::new(t, anchor(&t.id), self.task_pinned(&t.id)));
    }

    /// During an active drag: `Some(true)` for a legal drop column, `Some(false)` for an illegal one; `None` for
    /// the card's own column or when nothing is being dragged.
    pub(super) fn drop_target(&self, col: Status) -> Option<bool> {
        let d = self.drag.as_ref().filter(|d| d.active)?;
        if self
            .tasks
            .iter()
            .find(|task| task.id == d.id)
            .and_then(|task| crate::attention::column(task, &self.attention))
            == Some(col)
        {
            return None;
        }
        Some(drop_key(d.status, col).is_some())
    }

    pub(super) fn dragging(&self) -> bool {
        self.drag.as_ref().is_some_and(|d| d.active)
    }

    /// Drops any pending or active drag; true when a drag was in progress (the key that cancelled it is spent).
    pub(super) fn cancel_drag(&mut self) -> bool {
        self.drag.take().is_some_and(|d| d.active)
    }

    fn press(&mut self, m: &MouseCtx) -> Action {
        self.drag = None;
        match m.hit.clone() {
            Some(Hit::Column(status)) => self.focus_column(status),
            Some(Hit::CardMenu(id)) => {
                self.select_id(&id);
                self.open_menu();
            }
            Some(Hit::Card(id)) => {
                let was_selected = self.selected.as_deref() == Some(id.as_str());
                self.select_id(&id);
                if m.double {
                    return self.board_key(plain(KeyCode::Enter), m.ctx);
                }
                if let Some(status) = self.selected_task().map(|t| t.status) {
                    self.drag = Some(Drag {
                        id,
                        status,
                        was_selected,
                        active: false,
                    });
                }
            }
            _ => {}
        }
        Action::None
    }

    fn drag_over(&mut self, m: &MouseCtx) {
        if let Some(d) = &mut self.drag {
            let on_origin =
                matches!(&m.hit, Some(Hit::Card(id) | Hit::CardMenu(id)) if *id == d.id);
            d.active |= !on_origin;
        }
    }

    fn release(&mut self, ev: MouseEvent, ctx: &Ctx) -> Action {
        let Some(d) = self.drag.take() else {
            return Action::None;
        };
        if !d.active {
            return if d.was_selected {
                self.board_key(plain(KeyCode::Enter), ctx)
            } else {
                Action::None
            };
        }
        let code = self
            .column_at(ev.column, ev.row)
            .and_then(|to| drop_key(d.status, to));
        match code {
            Some(code) => {
                self.select_id(&d.id);
                self.board_key(plain(code), ctx)
            }
            None => Action::None,
        }
    }

    fn scroll(&mut self, ev: MouseEvent, down: bool) -> Action {
        if let Some(col) = self.column_at(ev.column, ev.row) {
            let slot = Status::COLUMNS.iter().position(|s| *s == col).unwrap_or(0);
            let n = self.column_tasks(col).len();
            let off = &mut self.offsets[slot];
            *off = if down {
                (*off + 1).min(n.saturating_sub(1))
            } else {
                off.saturating_sub(1)
            };
            self.follow = false;
        }
        Action::None
    }

    fn column_at(&self, x: u16, y: u16) -> Option<Status> {
        self.col_rects
            .iter()
            .find(|(_, r)| r.contains((x, y).into()))
            .map(|(s, _)| *s)
    }

    fn select_id(&mut self, id: &str) {
        self.selected = Some(id.to_string());
        self.follow = true;
        self.restore_selection();
    }

    fn focus_column(&mut self, status: Status) {
        if let Some(ci) = self.columns().iter().position(|c| *c == status) {
            self.col = ci;
            self.row = 0;
            self.follow = true;
            self.select_current();
        }
    }
}
