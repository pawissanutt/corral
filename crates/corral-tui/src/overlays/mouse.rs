use corral_core::RunMode;
use ratatui::crossterm::event::MouseEvent;

use super::{CAPTURE_STATUS, CaptureView, DispatchSheet, Z_MODE, Z_PROJECT, Z_STATUS};
use crate::hit::{self, Hit};
use crate::{Action, MouseCtx};

impl CaptureView {
    pub(super) fn mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        if !hit::left(&ev) {
            return Action::None;
        }
        match m.hit {
            Some(Hit::Item(Z_PROJECT, i)) => {
                if let Some(p) = self.projects.get(i) {
                    self.project = Some(p.clone());
                }
            }
            Some(Hit::Item(Z_STATUS, i)) => {
                if let Some((_, s)) = CAPTURE_STATUS.get(i) {
                    self.status = *s;
                }
            }
            _ => {}
        }
        Action::None
    }
}

impl DispatchSheet {
    /// Profile/model/effort controls (and the model picker, modal while open) first, then the mode pills.
    pub(super) fn mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        if let Some(action) = self.choice.on_mouse(&ev, m) {
            return action;
        }
        if !hit::left(&ev) {
            return Action::None;
        }
        match m.hit {
            Some(Hit::Item(Z_MODE, 0)) => self.mode = RunMode::Interactive,
            Some(Hit::Item(Z_MODE, 1)) => self.mode = RunMode::Headless,
            _ => {}
        }
        Action::None
    }
}
