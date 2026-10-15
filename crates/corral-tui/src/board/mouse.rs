use corral_core::Status;
use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use super::BoardView;
use crate::{Action, Hit, MouseCtx};

impl BoardView {
    pub(super) fn mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        if let Some(ov) = &mut self.overlay {
            let action = ov.view().on_mouse(ev, m);
            return self.overlay_outcome(action, m.ctx);
        }
        match (ev.kind, &m.hit) {
            (MouseEventKind::Down(MouseButton::Left), Some(Hit::Column(status))) => {
                self.focus_column(*status);
                Action::None
            }
            _ => Action::None,
        }
    }

    fn focus_column(&mut self, status: Status) {
        if let Some(ci) = self.columns().iter().position(|c| *c == status) {
            self.col = ci;
            self.row = 0;
            self.select_current();
        }
    }
}
