//! Full-screen live task search. The store snapshot and shared parser also serve the CLI.

use anyhow::Result;
use corral_core::Task;
use corral_core::query::Query;
use corral_ops::{Ctx, SearchData};
use jiff::Timestamp;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::artifacts::ui::{self, fit};
use crate::hit::{self, Hit};
use crate::{Action, HitMap, MouseCtx, View, shelf, theme};

#[cfg(test)]
mod tests;

const Z_LIST: u8 = 0;
const Z_ROW: u8 = 1;

pub struct SearchView {
    data: SearchData,
    input: String,
    matches: Vec<usize>,
    error: Option<String>,
    sel: usize,
    offset: usize,
}

impl SearchView {
    pub fn new(ctx: &Ctx) -> Result<Self> {
        let mut view = Self {
            data: SearchData::load(&ctx.store)?,
            input: String::new(),
            matches: Vec::new(),
            error: None,
            sel: 0,
            offset: 0,
        };
        view.refilter();
        Ok(view)
    }

    fn refilter(&mut self) {
        match Query::parse(&self.input, Timestamp::now()) {
            Ok(query) => {
                self.error = None;
                self.matches = self.data.ranked(&query);
            }
            Err(error) => {
                self.error = Some(format!("Invalid query: {error:#}"));
                self.matches.clear();
            }
        }
        self.sel = self.sel.min(self.matches.len().saturating_sub(1));
    }

    fn selected(&self) -> Option<&Task> {
        self.matches
            .get(self.sel)
            .and_then(|&i| self.data.tasks.get(i))
    }

    fn move_sel(&mut self, delta: isize) {
        self.sel = self
            .sel
            .saturating_add_signed(delta)
            .min(self.matches.len().saturating_sub(1));
    }
}

impl View for SearchView {
    fn draw(&mut self, f: &mut Frame, area: Rect, _ctx: &Ctx, hits: &mut HitMap) {
        ui::paint_bg(f, area);
        let [head, _, input, error, body] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .horizontal_margin(1)
        .areas(area);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("‹ back", Style::new().fg(theme::CYAN)),
                Span::styled("  search · all statuses", theme::bold()),
            ])),
            head,
        );
        hits.push(
            Rect {
                width: head.width.min(6),
                ..head
            },
            Hit::Key(hit::press(KeyCode::Esc)),
        );
        let mut spans = vec![Span::styled("/ ", Style::new().fg(theme::CYAN))];
        spans.extend(ui::input_spans(
            &self.input,
            "text, project:<name>, status:<status>, pinned",
        ));
        f.render_widget(Paragraph::new(Line::from(spans)), input);
        if let Some(message) = &self.error {
            f.render_widget(
                Paragraph::new(message.as_str()).style(Style::new().fg(theme::RED)),
                error,
            );
        }
        let list = ui::boxed(
            f,
            body,
            &format!("results {}", self.matches.len()),
            true,
            None,
        );
        hits.push(body, Hit::Item(Z_LIST, 0));
        let height = usize::from(list.height);
        if height == 0 {
            return;
        }
        if self.matches.is_empty() {
            if self.error.is_none() {
                f.render_widget(Paragraph::new("no tasks match").style(theme::dim()), list);
            }
            return;
        }
        self.offset = self
            .offset
            .min(self.sel)
            .max((self.sel + 1).saturating_sub(height));
        let lines: Vec<Line> = self
            .matches
            .iter()
            .enumerate()
            .skip(self.offset)
            .take(height)
            .map(|(n, &i)| {
                let task = &self.data.tasks[i];
                let prefix = format!("{}  {}  ", task.id, task.status.as_str());
                let project = task.project.as_deref().unwrap_or("—");
                let text = format!("{prefix}{}  · {project}", task.title);
                let line = Line::from(vec![
                    shelf::star(self.data.pinned.contains(&task.id)),
                    Span::raw(fit(&text, usize::from(list.width).saturating_sub(2))),
                ]);
                if n == self.sel {
                    line.style(Style::new().bg(theme::SELECTED_BG).fg(theme::FG))
                } else {
                    line
                }
            })
            .collect();
        f.render_widget(Paragraph::new(lines), list);
        hit::rows(hits, list, Z_ROW, 0, self.matches.len(), self.offset);
    }

    fn on_key(&mut self, key: KeyEvent, _ctx: &Ctx) -> Action {
        if !ui::is_press(&key) {
            return Action::None;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return Action::Back,
            KeyCode::Down => self.move_sel(1),
            KeyCode::Up => self.move_sel(-1),
            KeyCode::Char('n') if ctrl => self.move_sel(1),
            KeyCode::Char('p') if ctrl => self.move_sel(-1),
            KeyCode::PageDown => self.move_sel(10),
            KeyCode::PageUp => self.move_sel(-10),
            KeyCode::Enter => {
                return self
                    .selected()
                    .map_or(Action::None, |task| Action::OpenDetail(task.id.clone()));
            }
            _ => {
                if ui::edit_text(&mut self.input, &key) {
                    self.sel = 0;
                    self.offset = 0;
                    self.refilter();
                }
            }
        }
        Action::None
    }

    fn on_mouse(&mut self, ev: MouseEvent, m: &MouseCtx) -> Action {
        if let Some(delta) = hit::wheel(&ev) {
            if matches!(m.hit, Some(Hit::Item(Z_LIST | Z_ROW, _))) {
                self.move_sel(delta);
            }
        } else if hit::left(&ev)
            && let Some(Hit::Item(Z_ROW, n)) = m.hit
        {
            let open = n == self.sel || m.double;
            self.sel = n;
            if open {
                return self.on_key(hit::press(KeyCode::Enter), m.ctx);
            }
        }
        Action::None
    }

    fn hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("type", "search"),
            ("↑↓", "select"),
            ("⏎", "open"),
            ("esc", "back"),
        ]
    }

    fn refresh(&mut self, ctx: &Ctx) {
        let keep = self.selected().map(|task| task.id.clone());
        match SearchData::load(&ctx.store) {
            Ok(data) => self.data = data,
            Err(error) => {
                self.error = Some(format!("Could not refresh search: {error:#}"));
                return;
            }
        }
        self.refilter();
        if let Some(id) = keep
            && let Some(n) = self
                .matches
                .iter()
                .position(|&i| self.data.tasks[i].id == id)
        {
            self.sel = n;
        }
    }

    fn captures_text(&self) -> bool {
        true
    }
}
