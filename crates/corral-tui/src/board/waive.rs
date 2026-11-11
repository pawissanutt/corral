//! Owner confirmation for landing one co-work finish after its gate without independent review.
use corral_core::Task;
use corral_ops::Ctx;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Paragraph, Wrap};

use crate::hit::{self, Hit};
use crate::overlays::{LineInput, centered, popup};
use crate::{Action, HitMap, MouseCtx, Toast, Tone, View, theme};

pub(crate) struct ConfirmWaive {
    id: String,
    reason: LineInput,
}

impl ConfirmWaive {
    pub(super) fn new(task: &Task) -> Self {
        Self {
            id: task.id.clone(),
            reason: LineInput::default(),
        }
    }
}

impl View for ConfirmWaive {
    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx, hits: &mut HitMap) {
        let inner = popup(
            f,
            centered(area, 74, 12),
            &format!("land {} without review", self.id),
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
            Paragraph::new(format!(" Land {} without independent review?", self.id))
                .style(Style::new().fg(theme::RED)),
            question,
        );
        f.render_widget(
            Paragraph::new(
                "The gate still runs and must pass. No reviewer starts for this run; the card, \
                 history and landing report record that it landed without review.",
            )
            .wrap(Wrap { trim: true })
            .style(theme::dim()),
            effect,
        );
        self.reason.draw(f, reason, "reason ›", "required");
        hits.push(reason, Hit::Item(0, 0));
        hit::buttons(
            f,
            hits,
            controls.x,
            controls.y,
            controls.right(),
            &[
                ("Land without review", hit::press(KeyCode::Enter)),
                ("Cancel", hit::press(KeyCode::Esc)),
            ],
        );
    }

    fn on_key(&mut self, key: KeyEvent, ctx: &Ctx) -> Action {
        match key.code {
            KeyCode::Esc => Action::Back,
            KeyCode::Enter => {
                match corral_ops::review_waiver::waive(ctx, &self.id, &self.reason.text) {
                    Ok(()) => Action::BackWithToast(Toast::new(
                        Tone::Warn,
                        format!("{} review waived", self.id),
                        "lands once its gate passes, marked landed without review",
                    )),
                    Err(error) => Action::Toast(Toast::new(
                        Tone::Bad,
                        format!("Waive review for {} failed", self.id),
                        format!("{error:#}"),
                    )),
                }
            }
            _ => {
                self.reason.edit(&key);
                Action::None
            }
        }
    }

    fn on_mouse(&mut self, _ev: MouseEvent, _m: &MouseCtx) -> Action {
        Action::None
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![("⏎", "land without review"), ("esc", "cancel")]
    }

    fn captures_text(&self) -> bool {
        true
    }

    fn has_unsaved_text(&self) -> bool {
        !self.reason.text.is_empty()
    }
}
