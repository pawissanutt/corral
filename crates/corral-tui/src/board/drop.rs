use anyhow::Result;
use corral_core::{Origin, Status, Task};
use corral_ops::Ctx;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Paragraph, Wrap};

use crate::hit::{self, Hit};
use crate::overlays::{LineInput, centered, popup};
use crate::{Action, HitMap, MouseCtx, Toast, Tone, View, theme};

pub(crate) struct ConfirmDrop {
    id: String,
    has_run: bool,
    reason: LineInput,
    editing: bool,
}

impl ConfirmDrop {
    pub(super) fn new(task: &Task) -> Self {
        Self {
            id: task.id.clone(),
            has_run: task.current_run().is_some(),
            reason: LineInput::default(),
            editing: false,
        }
    }

    fn confirm(&self, ctx: &Ctx) -> Result<()> {
        let task = ctx.store.get(&self.id)?;
        let reason = self.reason.text.trim();
        let reason = (!reason.is_empty()).then_some(reason);
        if task.effective_origin() == Origin::Owner {
            ctx.store.owner_decision(
                &self.id,
                "drop",
                reason.unwrap_or("Owner confirmed drop on the board"),
                "tui",
            )?;
        }
        let dropped = if task.current_run().is_some() {
            corral_ops::discard_with_decision(ctx, &self.id, false, reason, "owner")?
        } else {
            corral_ops::move_task_with_decision(
                ctx,
                &self.id,
                Status::Dropped,
                false,
                reason,
                "owner",
                false,
            )?
        };
        ctx.store.log_event(
            Some(&self.id),
            "dropped",
            serde_json::json!({
                "actor": "owner", "via": "tui", "reason": reason,
                "from": task.status.as_str(), "task_updated": dropped.updated,
            }),
        )
    }
}

impl View for ConfirmDrop {
    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx, hits: &mut HitMap) {
        let inner = popup(
            f,
            centered(area, 74, 12),
            &format!("drop {}", self.id),
            hits,
        );
        let [question, effect, reason, controls] = ratatui::layout::Layout::vertical([
            ratatui::layout::Constraint::Length(1),
            ratatui::layout::Constraint::Fill(1),
            ratatui::layout::Constraint::Length(2),
            ratatui::layout::Constraint::Length(1),
        ])
        .areas(inner);
        f.render_widget(
            Paragraph::new(format!(" Drop {}?", self.id)).style(Style::new().fg(theme::RED)),
            question,
        );
        let text = if self.has_run {
            "Owned panes are closed. Discard removes the worktree and branch when possible; \
             artifacts stay. Cleanup warnings appear in messages."
        } else {
            "Only the task status changes to Dropped. No panes or worktree are removed."
        };
        f.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: true })
                .style(theme::dim()),
            effect,
        );
        if self.editing {
            self.reason.draw(f, reason, "reason ›", "optional");
        } else {
            f.render_widget(
                Paragraph::new(format!(
                    "reason (optional, Tab to edit): {}",
                    self.reason.text
                )),
                reason,
            );
        }
        hits.push(reason, Hit::Item(0, 0));
        hit::buttons(
            f,
            hits,
            controls.x,
            controls.y,
            controls.right(),
            &[
                ("Drop", hit::press(KeyCode::Enter)),
                ("Cancel", hit::press(KeyCode::Esc)),
            ],
        );
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        match key.code {
            KeyCode::Esc => Action::Back,
            KeyCode::Char('n') if !self.editing => Action::Back,
            KeyCode::Tab | KeyCode::BackTab => {
                self.editing = !self.editing;
                Action::None
            }
            KeyCode::Enter => match self.confirm(ctx) {
                Ok(()) => Action::BackWithToast(Toast::new(
                    Tone::Warn,
                    format!("✗ {} dropped", self.id),
                    "card moved to history; discard cleanup warnings appear in messages",
                )),
                Err(error) => Action::Toast(Toast::new(
                    Tone::Bad,
                    format!("Drop {} failed", self.id),
                    format!("{error:#}"),
                )),
            },
            KeyCode::Char('y') if !self.editing => self.on_key(hit::press(KeyCode::Enter), ctx),
            _ => {
                if self.editing {
                    self.reason.edit(&key);
                }
                Action::None
            }
        }
    }

    fn on_mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        if hit::left(&ev) && matches!(m.hit, Some(Hit::Item(0, 0))) {
            self.editing = true;
        }
        Action::None
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![("⏎", "drop"), ("⇥", "reason / confirm"), ("esc", "cancel")]
    }

    fn captures_text(&self) -> bool {
        true
    }

    fn has_unsaved_text(&self) -> bool {
        !self.reason.text.is_empty()
    }
}
