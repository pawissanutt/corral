//! Owner plan decision sheet: approve (resolving each owner decision) or request changes. Both record an explicit
//! owner verdict through `corral_ops::plan_verdict` with no review attempt, like `corral plan verdict --owner`.

use corral_core::{PlanReview, Task, Verdict};
use corral_ops::{Ctx, PlanVerdict};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::artifacts::ui;
use crate::hit::{self, Hit};
use crate::theme;
use crate::{Action, HitMap, Toast, Tone};

/// `Hit::Item` zone of the sheet's input rows.
pub(crate) const Z_FIELD: u8 = 6;
const APPROVE_NOTE: &str = "Approved from the board.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Approve,
    Changes,
}

pub(crate) struct Sheet {
    kind: Kind,
    task: String,
    plan: String,
    /// Owner decisions: approval requires all answers; changes may resolve any of them.
    decisions: Vec<String>,
    inputs: Vec<String>,
    focus: usize,
    discarding: bool,
    scroll: ui::Scroll,
    reveal_focus: bool,
    page: usize,
}

/// What a key did: keep the sheet open or close it, with the action for the view.
pub(crate) enum Outcome {
    Stay(Action),
    Close(Action),
}

impl Sheet {
    pub(crate) fn new(kind: Kind, task: &Task) -> Option<Sheet> {
        let plan = task.plans.last()?;
        let decisions: Vec<String> = plan
            .document
            .owner_decisions
            .iter()
            .chain(task.completion.iter().flat_map(|m| &m.owner_decisions))
            .cloned()
            .collect();
        let mut inputs = vec![String::new(); decisions.len()];
        inputs.push(match kind {
            Kind::Approve => APPROVE_NOTE.into(),
            Kind::Changes => String::new(),
        });
        Some(Sheet {
            kind,
            task: task.id.clone(),
            plan: plan.id.clone(),
            decisions,
            inputs,
            focus: 0,
            discarding: false,
            scroll: ui::Scroll::default(),
            reveal_focus: true,
            page: 1,
        })
    }

    pub(crate) fn has_unsaved_text(&self) -> bool {
        let note = self.inputs.last().map_or("", String::as_str);
        self.inputs[..self.decisions.len()]
            .iter()
            .any(|i| !i.is_empty())
            || (!note.is_empty() && note != APPROVE_NOTE)
    }

    pub(crate) fn focus(&mut self, i: usize) {
        self.focus = i.min(self.inputs.len() - 1);
        self.reveal_focus = true;
    }

    pub(crate) fn wheel(&mut self, dir: isize) {
        if !self.discarding {
            self.scroll.wheel(dir);
            self.reveal_focus = false;
        }
    }

    pub(crate) fn hints(&self) -> Vec<(&'static str, &'static str)> {
        if self.discarding {
            return vec![("⏎", "discard notes"), ("esc", "keep editing")];
        }
        let submit = match self.kind {
            Kind::Approve => "approve plan",
            Kind::Changes => "send changes",
        };
        vec![
            ("⇥", "next field"),
            ("pgup/pgdn", "scroll"),
            ("⏎", submit),
            ("esc", "cancel"),
        ]
    }

    pub(crate) fn on_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Outcome {
        if self.discarding {
            match key.code {
                KeyCode::Enter => return Outcome::Close(Action::None),
                KeyCode::Esc => self.discarding = false,
                _ => {}
            }
            return Outcome::Stay(Action::None);
        }
        let n = self.inputs.len();
        match key.code {
            KeyCode::Esc if self.has_unsaved_text() => self.discarding = true,
            KeyCode::Esc => return Outcome::Close(Action::None),
            KeyCode::Tab | KeyCode::Down => self.focus((self.focus + 1) % n),
            KeyCode::BackTab | KeyCode::Up => self.focus((self.focus + n - 1) % n),
            KeyCode::PageUp | KeyCode::PageDown => {
                self.scroll.on_key(key, self.page);
                self.reveal_focus = false;
            }
            KeyCode::Enter => return self.submit(ctx),
            _ => {
                ui::edit_text(&mut self.inputs[self.focus], key);
                self.reveal_focus = true;
            }
        }
        Outcome::Stay(Action::None)
    }

    fn notes(&self) -> Result<String, &'static str> {
        let note = self.inputs.last().map_or("", |s| s.trim());
        let answers = &self.inputs[..self.decisions.len()];
        if self.kind == Kind::Approve && answers.iter().any(|a| a.trim().is_empty()) {
            return Err("resolve every owner decision before approving");
        }
        let mut notes = note.to_string();
        for (question, answer) in self.decisions.iter().zip(answers) {
            if !answer.trim().is_empty() {
                notes.push_str(&format!("\nResolved: {question} => {}", answer.trim()));
            }
        }
        match notes.trim().is_empty() {
            true if self.kind == Kind::Changes => Err("type what the planner should change"),
            true => Err("add a note for the owner decision"),
            false => Ok(notes.trim().to_string()),
        }
    }

    fn submit(&mut self, ctx: &Ctx) -> Outcome {
        let notes = match self.notes() {
            Ok(notes) => notes,
            Err(why) => {
                return Outcome::Stay(Action::Toast(Toast::new(
                    Tone::Warn,
                    "Nothing recorded",
                    why,
                )));
            }
        };
        let verdict = match self.kind {
            Kind::Approve => Verdict::Approve,
            Kind::Changes => Verdict::Changes,
        };
        let input = PlanVerdict {
            plan: &self.plan,
            attempt: None,
            verdict,
            notes: &notes,
        };
        match corral_ops::plan_verdict(ctx, &self.task, &input) {
            Ok(review) => Outcome::Close(Action::Toast(self.result_toast(ctx, &review))),
            Err(e) => Outcome::Stay(Action::Toast(Toast::new(
                Tone::Bad,
                format!("plan decision for {} failed", self.task),
                format!("{e:#}"),
            ))),
        }
    }

    fn result_toast(&self, ctx: &Ctx, review: &PlanReview) -> Toast {
        let (id, plan) = (&self.task, &self.plan);
        match review.verdict {
            Some(Verdict::Approve) => {
                let n = ctx.store.get(id).map_or(0, |t| t.children.len());
                let body = match n {
                    0 => "owner decision recorded; the plan is attached to this task".to_string(),
                    n => format!("owner decision recorded; decomposed into {n} Backlog tasks"),
                };
                Toast::new(Tone::Good, format!("✓ {id} plan {plan} approved"), body)
            }
            Some(Verdict::Changes) => Toast::new(
                Tone::Info,
                format!("↩ {id} plan changes requested"),
                "owner decision recorded; the planner revises the plan",
            ),
            _ => Toast::new(
                Tone::Warn,
                format!("{id} plan {plan} escalated"),
                review
                    .notes
                    .as_deref()
                    .and_then(|n| n.lines().map(str::trim).rfind(|l| !l.is_empty()))
                    .unwrap_or("the owner verdict was escalated")
                    .to_string(),
            ),
        }
    }

    fn field(&self, i: usize, label: &str, placeholder: &str) -> Line<'static> {
        let focused = i == self.focus;
        let prompt = Style::new().fg(if focused { theme::YELLOW } else { theme::DIM });
        let mut spans = vec![Span::styled(format!(" {label} › "), prompt)];
        if focused {
            spans.extend(ui::input_spans(&self.inputs[i], placeholder));
        } else {
            spans.push(Span::styled(
                self.inputs[i].clone(),
                Style::new().fg(theme::FG),
            ));
        }
        Line::from(spans)
    }

    pub(crate) fn draw(&mut self, f: &mut Frame, area: Rect, hits: &mut HitMap) {
        if self.discarding {
            let rect = ui::centered(area, 64, 6);
            let inner = ui::popup(f, rect, "Discard owner notes?", None);
            hit::modal_popup(f, rect, hits);
            f.render_widget(
                Paragraph::new("Your edited notes and resolutions have not been recorded.")
                    .style(Style::new().fg(theme::FG)),
                inner,
            );
            hit::buttons(
                f,
                hits,
                inner.x + 1,
                inner.y + 2,
                inner.right(),
                &[
                    ("Discard", hit::press(KeyCode::Enter)),
                    ("Keep editing", hit::press(KeyCode::Esc)),
                ],
            );
            return;
        }
        let (title, intro, submit) = match self.kind {
            Kind::Approve => (
                format!("approve plan {} of {}", self.plan, self.task),
                " Records your owner decision: corral plan verdict --owner approve",
                "Approve",
            ),
            Kind::Changes => (
                format!("request plan changes {} of {}", self.plan, self.task),
                " Records your owner decision and sends these notes back to the planner",
                "Send",
            ),
        };
        let mut lines = vec![Line::styled(intro, theme::dim()), Line::default()];
        let mut rows = Vec::new();
        for (i, question) in self.decisions.iter().enumerate() {
            lines.push(Line::styled(
                format!(" decision: {question}"),
                Style::new().fg(theme::FG),
            ));
            rows.push(lines.len());
            lines.push(self.field(i, "resolution", "how you resolve it"));
        }
        let last = self.inputs.len() - 1;
        rows.push(lines.len());
        lines.push(match self.kind {
            Kind::Approve => self.field(last, "note", "why you approve"),
            Kind::Changes => self.field(last, "changes", "what should the planner change?"),
        });
        let height = u16::try_from(lines.len() + 4).unwrap_or(u16::MAX);
        let rect = ui::centered(area, 86, height);
        let inner = ui::popup(f, rect, &title, None);
        hit::modal_popup(f, rect, hits);
        let [content, footer] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(2)]).areas(inner);
        let total = lines.len();
        self.reveal_focus |= self.page != usize::from(content.height);
        self.page = usize::from(content.height);
        if self.reveal_focus {
            let row = rows[self.focus];
            if row < self.scroll.offset {
                self.scroll.offset = row.saturating_sub(1);
            } else if row >= self.scroll.offset + self.page {
                self.scroll.offset = (row + 1).saturating_sub(self.page);
            }
            self.reveal_focus = false;
        }
        let offset = self.scroll.clamp(total, self.page);
        f.render_widget(
            Paragraph::new(
                lines
                    .into_iter()
                    .skip(offset)
                    .take(self.page)
                    .collect::<Vec<_>>(),
            )
            .style(Style::new().fg(theme::FG)),
            content,
        );
        for (i, row) in rows.into_iter().enumerate() {
            if let Some(dy) = row.checked_sub(offset)
                && dy < self.page
                && let Ok(dy) = u16::try_from(dy)
            {
                let rect = Rect::new(content.x, content.y + dy, content.width, 1);
                hits.push(rect, Hit::Item(Z_FIELD, i));
            }
        }
        let mut buttons = Vec::new();
        if total > self.page {
            f.render_widget(
                Paragraph::new(format!(
                    " rows {}-{} of {total} · wheel / pgup/pgdn",
                    offset + 1,
                    (offset + self.page).min(total),
                ))
                .style(theme::dim()),
                footer,
            );
            buttons.extend([
                ("↑", hit::press(KeyCode::PageUp)),
                ("↓", hit::press(KeyCode::PageDown)),
            ]);
        }
        buttons.extend([
            (submit, hit::press(KeyCode::Enter)),
            ("Cancel", hit::press(KeyCode::Esc)),
        ]);
        hit::buttons(
            f,
            hits,
            footer.x + 1,
            footer.bottom().saturating_sub(1),
            footer.right(),
            &buttons,
        );
    }
}
