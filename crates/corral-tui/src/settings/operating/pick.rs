//! Checklist of projects and unfinished tasks for an Autopilot grant.
use std::collections::BTreeSet;

use anyhow::Result;
use corral_core::{OperatingSelection, Status};
use corral_ops::Ctx;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::hit::{self, Hit, HitMap};
use crate::{overlays, theme};

pub(crate) enum Outcome {
    Stay,
    Cancel,
    Apply {
        projects: BTreeSet<String>,
        tasks: BTreeSet<String>,
    },
}

enum Entry {
    Project(String),
    Task { id: String, line: String },
}

pub(crate) struct ScopePicker {
    entries: Vec<Entry>,
    projects: BTreeSet<String>,
    tasks: BTreeSet<String>,
    sel: usize,
    scroll: usize,
}

impl ScopePicker {
    pub(crate) fn new(ctx: &Ctx, selection: &OperatingSelection, at_tasks: bool) -> Result<Self> {
        let mut entries = ctx
            .store
            .config
            .projects
            .keys()
            .map(|project| Entry::Project(project.clone()))
            .collect::<Vec<_>>();
        let first_task = entries.len();
        let mut tasks = ctx.store.list()?;
        tasks
            .retain(|task| !task.tracker && !matches!(task.status, Status::Done | Status::Dropped));
        tasks.sort_by(|a, b| b.created.cmp(&a.created));
        entries.extend(tasks.into_iter().map(|task| Entry::Task {
            line: format!(
                "{}  {}  {}  {}",
                task.id,
                task.project.as_deref().unwrap_or("-"),
                task.status.as_str(),
                task.title
            ),
            id: task.id,
        }));
        let sel = if at_tasks && first_task < entries.len() {
            first_task
        } else {
            0
        };
        Ok(Self {
            entries,
            projects: selection.projects.clone(),
            tasks: selection.tasks.clone(),
            sel,
            scroll: 0,
        })
    }

    fn toggle(&mut self) {
        let (set, key) = match self.entries.get(self.sel) {
            Some(Entry::Project(project)) => (&mut self.projects, project),
            Some(Entry::Task { id, .. }) => (&mut self.tasks, id),
            None => return,
        };
        if !set.remove(key) {
            set.insert(key.clone());
        }
    }

    pub(crate) fn on_key(&mut self, key: KeyEvent) -> Outcome {
        let last = self.entries.len().saturating_sub(1);
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => self.sel = (self.sel + 1).min(last),
            KeyCode::PageUp => self.sel = self.sel.saturating_sub(10),
            KeyCode::PageDown => self.sel = (self.sel + 10).min(last),
            KeyCode::Char(' ') => self.toggle(),
            KeyCode::Enter => {
                return Outcome::Apply {
                    projects: self.projects.clone(),
                    tasks: self.tasks.clone(),
                };
            }
            KeyCode::Esc => return Outcome::Cancel,
            _ => {}
        }
        Outcome::Stay
    }

    pub(crate) fn draw(&mut self, f: &mut Frame, area: Rect, hits: &mut HitMap) {
        overlays::dim_backdrop(f.buffer_mut(), area);
        let rect = overlays::centered(area, 96, area.height.saturating_sub(2));
        let inner = overlays::popup(f, rect, "what may Autopilot work on?", hits);
        let height = usize::from(inner.height.saturating_sub(2));
        if self.sel < self.scroll {
            self.scroll = self.sel;
        } else if height > 0 && self.sel >= self.scroll + height {
            self.scroll = self.sel + 1 - height;
        }
        let lines = self
            .entries
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(height)
            .map(|(i, entry)| {
                let (checked, text) = match entry {
                    Entry::Project(project) => (
                        self.projects.contains(project),
                        format!("project {project}"),
                    ),
                    Entry::Task { id, line } => (self.tasks.contains(id), line.clone()),
                };
                let text = overlays::truncate(
                    &format!(
                        "{} {} {text}",
                        if i == self.sel { "›" } else { " " },
                        if checked { "[x]" } else { "[ ]" }
                    ),
                    usize::from(inner.width),
                );
                let style = if i == self.sel {
                    theme::bold().add_modifier(Modifier::REVERSED)
                } else if matches!(entry, Entry::Project(_)) {
                    theme::bold()
                } else {
                    ratatui::style::Style::new()
                };
                Line::styled(text, style)
            })
            .collect::<Vec<_>>();
        f.render_widget(
            Paragraph::new(lines),
            Rect {
                height: inner.height.saturating_sub(2),
                ..inner
            },
        );
        let y = inner.bottom().saturating_sub(1);
        let mut x = inner.x;
        for (label, code) in [("[Apply]", KeyCode::Enter), ("[Cancel]", KeyCode::Esc)] {
            let width = u16::try_from(label.len()).unwrap_or(0);
            let rect = Rect::new(x, y, width, 1);
            f.render_widget(
                Paragraph::new(label).style(theme::bold().fg(theme::CYAN)),
                rect,
            );
            hits.push(rect, Hit::Key(hit::press(code)));
            x += width + 2;
        }
        f.render_widget(
            Paragraph::new("space select · ⏎ apply · esc cancel").style(theme::dim()),
            Rect::new(x, y, inner.right().saturating_sub(x), 1),
        );
    }
}
