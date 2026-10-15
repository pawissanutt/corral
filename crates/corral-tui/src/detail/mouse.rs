use ratatui::crossterm::event::{KeyCode, MouseEvent};

use super::{DetailView, Z_ACC, Z_ART, Z_CONTENT, Z_LIVE, Z_RUN};
use crate::artifacts::list::Row;
use crate::hit::{self, Hit, WHEEL_LINES};
use crate::{Action, MouseCtx, View};

impl DetailView {
    pub(super) fn scroll_live(&mut self, dir: isize) {
        self.live_scroll = self.live_scroll.saturating_add_signed(-dir * WHEEL_LINES);
    }

    /// Wheel scrolls the box under the pointer; a click selects a run, artifact or acceptance record row, and a
    /// click on the selected artifact or record (or a double-click) previews it like ⏎. Tabs and `‹ back` are key hits.
    pub(super) fn mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        if let Some(p) = &mut self.preview {
            if let Some(dir) = hit::wheel(&ev) {
                p.wheel(dir);
            }
            return Action::None;
        }
        if self.confirm.is_some() || self.comment.is_some() {
            return Action::None;
        }
        if let Some(dir) = hit::wheel(&ev) {
            match m.hit {
                Some(Hit::Item(Z_LIVE, _)) => {
                    self.scroll_live(dir);
                }
                Some(Hit::Item(Z_CONTENT | Z_RUN | Z_ART | Z_ACC, _)) => self.scroll.wheel(dir),
                _ => {}
            }
            return Action::None;
        }
        if !hit::left(&ev) {
            return Action::None;
        }
        match m.hit {
            Some(Hit::Item(Z_RUN, i)) => self.run_sel = Some(i),
            Some(Hit::Item(Z_ART, r)) => match self.list.rows(&self.task.artifacts).get(r) {
                Some(Row::Fold) => self
                    .list
                    .toggle_fold(&self.task.artifacts, &mut self.art_sel),
                Some(Row::Item(i)) => {
                    let (i, open) = (*i, m.double || *i == self.art_sel);
                    self.art_sel = i;
                    if open {
                        return self.on_key(hit::press(KeyCode::Enter), m.ctx);
                    }
                }
                None => {}
            },
            Some(Hit::Item(Z_ACC, i)) if m.double || i == self.acc_sel => {
                self.acc_sel = i;
                return self.on_key(hit::press(KeyCode::Enter), m.ctx);
            }
            Some(Hit::Item(Z_ACC, i)) => self.acc_sel = i,
            _ => {}
        }
        Action::None
    }
}
