use corral_core::RunMode;
use ratatui::crossterm::event::MouseEvent;

use super::{CAPTURE_STATUS, CaptureView, DispatchSheet, Z_AGENT, Z_MODE, Z_PROJECT, Z_STATUS};
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
    pub(super) fn mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        if !hit::left(&ev) {
            return Action::None;
        }
        match m.hit {
            Some(Hit::Item(Z_AGENT, i)) if i < self.agents.len() => self.agent_idx = i,
            Some(Hit::Item(Z_MODE, 0)) => self.mode = RunMode::Interactive,
            Some(Hit::Item(Z_MODE, 1)) => self.mode = RunMode::Headless,
            _ => {}
        }
        Action::None
    }
}
