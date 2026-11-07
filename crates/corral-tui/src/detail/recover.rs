//! Hosts the owner recovery sheet over the detail view (`F`, or the Recover… button on Plan and Acceptance).
use corral_ops::Ctx;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::Rect;

use super::{ACCEPTANCE, DetailView, PLAN};
use crate::recovery::RecoverySheet;
use crate::{Action, HitMap, MouseCtx, View};

pub(super) fn load(ctx: &Ctx, id: &str) -> Result<Vec<corral_core::RecoveryRecord>, String> {
    corral_ops::recovery::recover_history(ctx, id).map_err(|e| format!("{e:#}"))
}

impl DetailView {
    pub(super) fn with_recoveries(mut self, ctx: &Ctx) -> DetailView {
        self.recoveries = Some(load(ctx, &self.task.id));
        self
    }

    /// Header text badges (`[owner recovery]`, `[review waived]`, …); words, never color alone.
    pub(super) fn recovery_badges(&self) -> Vec<ratatui::text::Span<'static>> {
        let Some(Ok(records)) = &self.recoveries else {
            return Vec::new();
        };
        crate::recovery::labels::badges(records)
            .into_iter()
            .map(|b| {
                ratatui::text::Span::styled(
                    format!("  [{b}]"),
                    ratatui::style::Style::new().fg(crate::theme::YELLOW),
                )
            })
            .collect()
    }

    /// Recovery is offered from the Plan and Acceptance tabs of a nonterminal task.
    pub(super) fn recover_available(&self) -> bool {
        matches!(self.tab, PLAN | ACCEPTANCE)
            && !matches!(
                self.task.status,
                corral_core::Status::Done | corral_core::Status::Dropped
            )
    }

    pub(super) fn open_recover(&mut self, ctx: &Ctx) -> Action {
        self.recover = Some(Box::new(RecoverySheet::new(ctx, &self.task)));
        Action::None
    }

    /// Back closes the sheet without effect; a completed operation reloads the task.
    fn recover_outcome(&mut self, action: Action, ctx: &Ctx) -> Action {
        match action {
            Action::Back => {
                self.recover = None;
                Action::None
            }
            Action::BackWithToast(t) => {
                self.recover = None;
                View::refresh(self, ctx);
                Action::Toast(t)
            }
            other => other,
        }
    }

    pub(super) fn recover_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Option<Action> {
        let sheet = self.recover.as_mut()?;
        let action = sheet.on_key(key, ctx);
        Some(self.recover_outcome(action, ctx))
    }

    pub(super) fn recover_mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Option<Action> {
        let sheet = self.recover.as_mut()?;
        let action = View::on_mouse(sheet.as_mut(), ev, m);
        Some(self.recover_outcome(action, m.ctx))
    }

    pub(super) fn draw_recover(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx, hits: &mut HitMap) {
        if let Some(sheet) = &mut self.recover {
            crate::overlays::dim_backdrop(f.buffer_mut(), area);
            sheet.draw(f, area, ctx, hits);
        }
    }

    /// Acceptance-tab section: each operation with reason, attestation and original checks.
    pub(super) fn recovery_lines(&self) -> Vec<ratatui::text::Line<'static>> {
        match &self.recoveries {
            None => vec![],
            Some(Ok(records)) => crate::recovery::labels::history_lines(records),
            Some(Err(e)) => vec![ratatui::text::Line::raw(format!(
                "Owner recovery history unavailable: {e}"
            ))],
        }
    }

    /// The visible Recover… button; clicking it presses `F`, the same path as the key.
    pub(super) fn recover_button(&self, f: &mut Frame, area: Rect, hits: &mut HitMap) {
        if self.recover.is_none() && self.recover_available() && area.height > 0 {
            let at = area.right().saturating_sub(13).max(area.x);
            crate::hit::buttons(
                f,
                hits,
                at,
                area.y,
                area.right(),
                &[("Recover…", crate::hit::press(KeyCode::Char('F')))],
            );
        }
    }
}
