//! Owner plan decision sheet: approve (resolving each owner decision) or request changes. Both record an explicit
//! owner verdict through `corral_ops::plan_verdict` with no review attempt, like `corral plan verdict --owner`.

use corral_core::{PlanReview, Task, Verdict};
use corral_ops::{Ctx, PlanVerdict};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
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
    /// Owner decisions an approval must resolve; one input each, then the note.
    decisions: Vec<String>,
    inputs: Vec<String>,
    focus: usize,
}

/// What a key did: keep the sheet open or close it, with the action for the view.
pub(crate) enum Outcome {
    Stay(Action),
    Close(Action),
}

impl Sheet {
    pub(crate) fn new(kind: Kind, task: &Task) -> Option<Sheet> {
        let plan = task.plans.last()?;
        let decisions: Vec<String> = match kind {
            Kind::Approve => plan
                .document
                .owner_decisions
                .iter()
                .chain(task.completion.iter().flat_map(|m| &m.owner_decisions))
                .cloned()
                .collect(),
            Kind::Changes => Vec::new(),
        };
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
    }

    pub(crate) fn hints(&self) -> Vec<(&'static str, &'static str)> {
        let submit = match self.kind {
            Kind::Approve => "approve plan",
            Kind::Changes => "send changes",
        };
        vec![("⇥", "next field"), ("⏎", submit), ("esc", "cancel")]
    }

    pub(crate) fn on_key(&mut self, key: &KeyEvent, ctx: &Ctx) -> Outcome {
        let n = self.inputs.len();
        match key.code {
            KeyCode::Esc => return Outcome::Close(Action::None),
            KeyCode::Tab | KeyCode::Down => self.focus = (self.focus + 1) % n,
            KeyCode::BackTab | KeyCode::Up => self.focus = (self.focus + n - 1) % n,
            KeyCode::Enter => return self.submit(ctx),
            _ => {
                ui::edit_text(&mut self.inputs[self.focus], key);
            }
        }
        Outcome::Stay(Action::None)
    }

    fn notes(&self) -> Result<String, &'static str> {
        let note = self.inputs.last().map_or("", |s| s.trim());
        let answers = &self.inputs[..self.decisions.len()];
        if answers.iter().any(|a| a.trim().is_empty()) {
            return Err("resolve every owner decision before approving");
        }
        let mut notes = note.to_string();
        for (question, answer) in self.decisions.iter().zip(answers) {
            notes.push_str(&format!("\nResolved: {question} => {}", answer.trim()));
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

    pub(crate) fn draw(&self, f: &mut Frame, area: Rect, hits: &mut HitMap) {
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
        lines.push(Line::default());
        let buttons_row = lines.len();
        lines.push(Line::default());
        let height = u16::try_from(lines.len() + 2).unwrap_or(u16::MAX);
        let rect = ui::centered(area, 86, height);
        let inner = ui::popup(f, rect, &title, None);
        hit::modal_popup(f, rect, hits);
        f.render_widget(
            Paragraph::new(lines).style(Style::new().fg(theme::FG)),
            inner,
        );
        for (i, row) in rows.into_iter().enumerate() {
            if let Ok(dy) = u16::try_from(row)
                && dy < inner.height
            {
                let rect = Rect::new(inner.x, inner.y + dy, inner.width, 1);
                hits.push(rect, Hit::Item(Z_FIELD, i));
            }
        }
        if let Ok(dy) = u16::try_from(buttons_row)
            && dy < inner.height
        {
            let buttons = [
                (submit, hit::press(KeyCode::Enter)),
                ("Cancel", hit::press(KeyCode::Esc)),
            ];
            hit::buttons(f, hits, inner.x + 1, inner.y + dy, inner.right(), &buttons);
        }
    }
}
