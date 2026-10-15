use corral_ops::Ctx;
use corral_ops::watch::{GateAttempt, ProgressState};

use ratatui::Frame;

use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{StageView, Z_CHECKLIST, Z_STEP};
use crate::artifacts::ui;
use crate::render::Format;
use crate::theme;
use crate::{Hit, HitMap};

pub(super) fn state(state: ProgressState) -> (&'static str, ratatui::style::Color) {
    match state {
        ProgressState::Pending => ("pending", theme::DIM),
        ProgressState::Running => ("running", theme::BLUE),
        ProgressState::Passed => ("passed", theme::GREEN),
        ProgressState::Failed => ("failed", theme::RED),
        ProgressState::Interrupted => ("interrupted", theme::YELLOW),
    }
}

pub(super) fn duration(start: Option<jiff::Timestamp>, end: Option<jiff::Timestamp>) -> String {
    let Some(start) = start else {
        return "-".into();
    };
    let seconds = end
        .unwrap_or_else(jiff::Timestamp::now)
        .duration_since(start)
        .as_secs()
        .max(0);
    if seconds >= 60 {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

impl StageView {
    pub(super) fn page_gate(&mut self, key: KeyCode, ctx: &Ctx) {
        let Some(g) = self.selected_gate() else {
            return;
        };
        let Some(command) = g.commands.get(self.step_index()) else {
            return;
        };
        let start = self.window_start.unwrap_or(command.log_start);
        let end = if command.state == ProgressState::Running {
            std::fs::metadata(&g.log)
                .map(|m| m.len())
                .unwrap_or(command.log_end)
        } else {
            command.log_end
        };
        let page = self.viewport.max(1);
        let earlier =
            key == KeyCode::PageUp && self.scroll.offset == 0 && start > command.log_start;
        let later = key == KeyCode::PageDown
            && self.scroll.offset.saturating_add(page) >= self.text.lines().count()
            && start.saturating_add(u64::try_from(self.text.len()).unwrap_or(u64::MAX)) < end;
        let next = if key == KeyCode::Home {
            Some(command.log_start)
        } else if earlier {
            Some(start.saturating_sub(128 * 1024).max(command.log_start))
        } else if later {
            Some(start.saturating_add(128 * 1024).min(end))
        } else {
            None
        };
        self.follow = false;
        if let Some(next) = next {
            self.window_start = Some(next);
            self.load_content(ctx);
            self.scroll.offset = if earlier {
                self.text.lines().count().saturating_sub(page)
            } else {
                0
            };
        } else if key == KeyCode::PageUp {
            self.scroll.offset = self.scroll.offset.saturating_sub(page);
        } else {
            self.scroll.offset = self.scroll.offset.saturating_add(page);
        }
    }

    pub(super) fn gate_index(&self) -> usize {
        self.gate_sel
            .as_ref()
            .and_then(|key| {
                self.history
                    .gates
                    .iter()
                    .position(|g| (&g.run, &g.attempt) == (&key.0, &key.1))
            })
            .unwrap_or(self.history.gates.len().saturating_sub(1))
    }

    pub(super) fn selected_gate(&self) -> Option<&GateAttempt> {
        self.history.gates.get(self.gate_index())
    }

    pub(super) fn step_index(&self) -> usize {
        let Some(g) = self.selected_gate() else {
            return 0;
        };
        self.step
            .unwrap_or_else(|| {
                g.commands
                    .iter()
                    .position(|s| {
                        matches!(
                            s.state,
                            ProgressState::Running
                                | ProgressState::Failed
                                | ProgressState::Interrupted
                        )
                    })
                    .unwrap_or(g.commands.len().saturating_sub(1))
            })
            .min(g.commands.len().saturating_sub(1))
    }

    pub(super) fn move_step(&mut self, dir: isize, ctx: &Ctx) {
        let len = self.selected_gate().map_or(0, |g| g.commands.len());
        self.step = Some(
            self.step_index()
                .saturating_add_signed(dir)
                .min(len.saturating_sub(1)),
        );
        self.scroll = ui::Scroll::default();
        self.window_start = None;
        self.follow = false;
        self.load_content(ctx);
    }

    pub(super) fn draw_gate(&mut self, f: &mut Frame, area: Rect, hits: &mut HitMap) {
        let Some(g) = self.selected_gate() else {
            f.render_widget(
                Paragraph::new("No gate attempts yet. Commands appear when a gate starts."),
                area,
            );
            return;
        };
        let (status, color) = state(g.state);
        let title = format!(
            "{} - round {} - attempt {} - {status}",
            g.run,
            g.round + 1,
            self.gate_index() + 1
        );
        let selected = self.step_index();
        let height = u16::try_from(g.commands.len())
            .unwrap_or(u16::MAX)
            .min(area.height.saturating_sub(8).max(1) / 2)
            .saturating_add(2);
        let [checklist, log] =
            Layout::vertical([Constraint::Length(height), Constraint::Fill(1)]).areas(area);
        let inner = ui::boxed(
            f,
            checklist,
            &title,
            true,
            Some(duration(Some(g.started), g.ended)),
        );
        hits.push(checklist, Hit::Item(Z_CHECKLIST, 0));
        let offset = selected.saturating_sub(usize::from(inner.height).saturating_sub(1));
        let lines: Vec<Line<'static>> = g
            .commands
            .iter()
            .enumerate()
            .skip(offset)
            .take(usize::from(inner.height))
            .map(|(i, s)| {
                let (label, color) = state(s.state);
                let style = Style::new().fg(color);
                let style = if i == selected {
                    style.bg(theme::SELECTED_BG)
                } else {
                    style
                };
                let prefix = format!(
                    "{} {}/{} [{label}] {} ",
                    if i == selected { ">" } else { " " },
                    i + 1,
                    g.commands.len(),
                    duration(s.started, s.ended),
                );
                let command = crate::overlays::truncate(
                    &s.command,
                    usize::from(inner.width).saturating_sub(prefix.len()),
                );
                Line::from(Span::styled(format!("{prefix}{command}"), style))
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
        crate::hit::rows(hits, inner, Z_STEP, 0, g.commands.len(), offset);
        let command = g
            .commands
            .get(selected)
            .map(|s| s.command.clone())
            .unwrap_or_default();
        let title = crate::overlays::truncate(
            &format!("Command: {command}"),
            usize::from(log.width)
                .saturating_sub(Line::raw(self.source.as_str()).width().saturating_add(6)),
        );
        let inner = ui::boxed(f, log, &title, true, Some(self.source.clone()));
        if self.text.is_empty() {
            f.render_widget(
                Paragraph::new("No output for this command yet.").style(Style::new().fg(color)),
                inner,
            );
        } else {
            self.draw_text(f, inner, Format::Log, hits);
        }
    }
}
